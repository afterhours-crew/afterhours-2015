// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windows calls behind safe wrappers.
//!
//! Handles are owned by the wrapper types and closed on drop. Buffers are
//! fixed-size and bounds are checked before any call. Numeric constants that
//! `windows-sys` spreads across feature-gated modules are written out with
//! their names so the values can be checked against the SDK headers.

use std::{
    ffi::{OsString, c_void},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    sync::atomic::Ordering,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, FILETIME, HANDLE, HWND, INVALID_HANDLE_VALUE,
        LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    NetworkManagement::IpHelper::GetExtendedTcpTable,
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::{
        Console::SetConsoleCtrlHandler,
        Diagnostics::{
            Debug::{ReadProcessMemory, WriteProcessMemory},
            ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
            },
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject,
        },
        ProcessStatus::{
            K32EnumProcessModulesEx, K32GetModuleFileNameExW, K32GetModuleInformation,
            LIST_MODULES_ALL, MODULEINFO,
        },
        Registry::{
            HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, REG_SZ, RegCloseKey,
            RegOpenKeyExW, RegQueryValueExW,
        },
        Threading::{
            GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken, OpenThread,
            QueryFullProcessImageNameW, WaitForSingleObject,
        },
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
        WM_CLOSE,
    },
};

/// `PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | SYNCHRONIZE`: what a pin
/// needs to observe a process. Matches the launcher's C# predecessor.
const PIN_READ_ACCESS: u32 = 0x0400 | 0x0010 | 0x0010_0000;
/// `PROCESS_VM_WRITE | PROCESS_VM_OPERATION`: added for the routed game only.
const PIN_WRITE_ACCESS: u32 = 0x0020 | 0x0008;
/// `THREAD_QUERY_INFORMATION`.
const THREAD_QUERY_INFORMATION: u32 = 0x0040;
/// `TCP_TABLE_OWNER_PID_LISTENER`.
const TCP_TABLE_OWNER_PID_LISTENER: i32 = 3;
/// `AF_INET`.
const AF_INET: u32 = 2;
/// `MIB_TCP_STATE_LISTEN`.
const TCP_STATE_LISTEN: u32 = 2;
/// Largest path we read back from the system, in UTF-16 units.
const PATH_UNITS: usize = 32_768;

fn last_error(operation: &str) -> std::io::Error {
    let error = std::io::Error::last_os_error();
    std::io::Error::new(error.kind(), format!("{operation}: {error}"))
}

fn utf16z(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn filetime(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

/// Owned kernel handle, closed on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was returned to us by a Win32 open/create call
        // and is closed exactly once here.
        unsafe { CloseHandle(self.0) };
    }
}

/// Whether the current process token is elevated.
pub fn is_elevated() -> std::io::Result<bool> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: returns a pseudo handle that needs no close.
    let current = unsafe { GetCurrentProcess() };
    // SAFETY: the out pointer is a valid HANDLE slot.
    if unsafe { OpenProcessToken(current, TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error("Open process token"));
    }
    let token = Owned(token);
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0u32;
    let size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
    // SAFETY: the buffer is a correctly sized TOKEN_ELEVATION and the length
    // passed matches it.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast::<c_void>(),
            size,
            &mut returned,
        )
    };
    if ok == 0 {
        return Err(last_error("Read token elevation"));
    }
    Ok(elevation.TokenIsElevated != 0)
}

/// Whether the current process is already inside a job object.
pub fn in_job() -> std::io::Result<bool> {
    let mut result = 0i32;
    // SAFETY: returns a pseudo handle that needs no close.
    let current = unsafe { GetCurrentProcess() };
    // SAFETY: a null job asks whether the process belongs to any job; the out
    // pointer is valid.
    if unsafe { IsProcessInJob(current, std::ptr::null_mut(), &mut result) } == 0 {
        return Err(last_error("Query process job"));
    }
    Ok(result != 0)
}

/// Whether a process with this identifier exists.
pub fn process_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: OpenProcess with query-limited access on a plain identifier.
    let handle = unsafe { OpenProcess(0x1000, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let handle = Owned(handle);
    // SAFETY: a zero wait on an owned process handle reports exit without blocking.
    unsafe { WaitForSingleObject(handle.0, 0) != WAIT_OBJECT_0 }
}

/// Session job: the supervisor and every process it starts.
///
/// Children inherit membership when Windows creates them, and
/// kill-on-close means Windows terminates the tree when the last handle to
/// the job closes, including when the supervisor is force-terminated.
pub struct Job(Owned);

impl Job {
    /// Create an anonymous job with kill-on-close and place the current
    /// process in it. The caller must have checked [`in_job`] first.
    pub fn attach_current_process() -> std::io::Result<Job> {
        // SAFETY: null security attributes and name create an anonymous job.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(last_error("Create session job"));
        }
        let job = Job(Owned(handle));
        job.set_kill_on_close(true)?;
        // SAFETY: returns a pseudo handle that needs no close.
        let current = unsafe { GetCurrentProcess() };
        // SAFETY: both handles are valid for the duration of the call.
        if unsafe { AssignProcessToJobObject(job.0.0, current) } == 0 {
            return Err(last_error("Own supervisor process tree"));
        }
        Ok(job)
    }

    fn set_kill_on_close(&self, enabled: bool) -> std::io::Result<()> {
        const _: () = assert!(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() == 144);
        // SAFETY: the struct is plain data; all-zero is a valid value.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        if enabled {
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        }
        // SAFETY: the information pointer and length describe one complete
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
        let ok = unsafe {
            SetInformationJobObject(
                self.0.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(last_error("Set session job limits"));
        }
        Ok(())
    }

    /// Identifiers of every process currently in the job, bounded to 128.
    pub fn process_ids(&self) -> std::io::Result<Vec<u32>> {
        // JOBOBJECT_BASIC_PROCESS_ID_LIST: two u32 counters then usize ids.
        const CAPACITY: usize = 128;
        let mut buffer = vec![0u8; 8 + CAPACITY * std::mem::size_of::<usize>()];
        let mut returned = 0u32;
        // SAFETY: the buffer length passed equals the allocation.
        let ok = unsafe {
            QueryInformationJobObject(
                self.0.0,
                JobObjectBasicProcessIdList,
                buffer.as_mut_ptr().cast::<c_void>(),
                buffer.len() as u32,
                &mut returned,
            )
        };
        if ok == 0 {
            return Err(last_error("Verify session children"));
        }
        let listed = u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;
        let count = listed.min(CAPACITY);
        Ok((0..count)
            .map(|i| {
                let start = 8 + i * std::mem::size_of::<usize>();
                const WIDTH: usize = std::mem::size_of::<usize>();
                let mut raw = [0u8; WIDTH];
                raw.copy_from_slice(&buffer[start..start + WIDTH]);
                usize::from_le_bytes(raw) as u32
            })
            .collect())
    }

    /// Whether a process handle belongs to this job.
    pub fn contains(&self, process: &Process) -> std::io::Result<bool> {
        let mut result = 0i32;
        // SAFETY: both handles are owned and valid; the out pointer is valid.
        if unsafe { IsProcessInJob(process.handle.0, self.0.0, &mut result) } == 0 {
            return Err(last_error("Query owned helper"));
        }
        Ok(result != 0)
    }

    /// Clear kill-on-close. Call only after every child has exited.
    pub fn disarm(&self) -> std::io::Result<()> {
        self.set_kill_on_close(false)
    }
}

/// An opened process.
pub struct Process {
    handle: Owned,
    pid: u32,
}

/// Result of waiting on a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// The process has exited.
    Exited,
    /// Still running when the timeout elapsed.
    Running,
}

impl Process {
    /// Open a process for observation, optionally with memory write access.
    pub fn open(pid: u32, write: bool) -> std::io::Result<Process> {
        if pid == 0 {
            return Err(std::io::Error::other("invalid process identifier"));
        }
        let access = PIN_READ_ACCESS | if write { PIN_WRITE_ACCESS } else { 0 };
        // SAFETY: plain identifier and access mask; the handle is checked.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            return Err(last_error("Open owned process"));
        }
        Ok(Process {
            handle: Owned(handle),
            pid,
        })
    }

    /// Process identifier this handle was opened with.
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// Wait up to `milliseconds` for exit.
    pub fn wait(&self, milliseconds: u32) -> std::io::Result<Wait> {
        // SAFETY: owned handle, bounded wait.
        match unsafe { WaitForSingleObject(self.handle.0, milliseconds) } {
            WAIT_OBJECT_0 => Ok(Wait::Exited),
            WAIT_TIMEOUT => Ok(Wait::Running),
            _ => Err(last_error("Process wait failed")),
        }
    }

    /// Creation time as a FILETIME value, which identifies a process lifetime
    /// together with its identifier.
    pub fn creation_time(&self) -> std::io::Result<u64> {
        let mut created = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exited = created;
        let mut kernel = created;
        let mut user = created;
        // SAFETY: four valid FILETIME out pointers.
        let ok = unsafe {
            GetProcessTimes(
                self.handle.0,
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            return Err(last_error("Read process creation time"));
        }
        Ok(filetime(created))
    }

    /// Full path of the process image.
    pub fn image_path(&self) -> std::io::Result<PathBuf> {
        let mut buffer = vec![0u16; PATH_UNITS];
        let mut size = PATH_UNITS as u32;
        // SAFETY: buffer and size describe the same allocation.
        let ok =
            unsafe { QueryFullProcessImageNameW(self.handle.0, 0, buffer.as_mut_ptr(), &mut size) };
        if ok == 0 {
            return Err(last_error("Read process image path"));
        }
        Ok(PathBuf::from(OsString::from_wide(&buffer[..size as usize])))
    }

    /// Base address, size and path of the main module.
    pub fn main_module(&self) -> std::io::Result<(u64, u32, PathBuf)> {
        let mut module: *mut c_void = std::ptr::null_mut();
        let mut needed = 0u32;
        // SAFETY: room for exactly one module handle, and the byte count says so.
        let ok = unsafe {
            K32EnumProcessModulesEx(
                self.handle.0,
                &mut module,
                std::mem::size_of::<*mut c_void>() as u32,
                &mut needed,
                LIST_MODULES_ALL,
            )
        };
        if ok == 0 {
            return Err(last_error("Read main module"));
        }
        if needed < std::mem::size_of::<*mut c_void>() as u32 || module.is_null() {
            return Err(std::io::Error::other("Main module missing"));
        }
        let mut info = MODULEINFO {
            lpBaseOfDll: std::ptr::null_mut(),
            SizeOfImage: 0,
            EntryPoint: std::ptr::null_mut(),
        };
        // SAFETY: a valid MODULEINFO out pointer with its exact size.
        let ok = unsafe {
            K32GetModuleInformation(
                self.handle.0,
                module,
                &mut info,
                std::mem::size_of::<MODULEINFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(last_error("Read module extent"));
        }
        let mut buffer = vec![0u16; PATH_UNITS];
        // SAFETY: buffer and capacity describe the same allocation.
        let length = unsafe {
            K32GetModuleFileNameExW(
                self.handle.0,
                module,
                buffer.as_mut_ptr(),
                PATH_UNITS as u32,
            )
        };
        if !(1..PATH_UNITS).contains(&(length as usize)) {
            return Err(last_error("Read module path"));
        }
        Ok((
            info.lpBaseOfDll as u64,
            info.SizeOfImage,
            PathBuf::from(OsString::from_wide(&buffer[..length as usize])),
        ))
    }

    /// Read `buffer.len()` bytes at `address`. Fails unless every byte was read.
    pub fn read_memory(&self, address: u64, buffer: &mut [u8]) -> std::io::Result<()> {
        let mut read = 0usize;
        // SAFETY: the destination is a live slice of the stated length; the
        // remote address is only dereferenced by the kernel on our behalf.
        let ok = unsafe {
            ReadProcessMemory(
                self.handle.0,
                address as usize as *const c_void,
                buffer.as_mut_ptr().cast::<c_void>(),
                buffer.len(),
                &mut read,
            )
        };
        if ok == 0 {
            return Err(last_error("Read startup data"));
        }
        if read != buffer.len() {
            return Err(std::io::Error::other("Partial startup read"));
        }
        Ok(())
    }

    /// Write `bytes` at `address`. Fails unless every byte was written.
    pub fn write_memory(&self, address: u64, bytes: &[u8]) -> std::io::Result<()> {
        let mut written = 0usize;
        // SAFETY: the source is a live slice of the stated length; the handle
        // was opened with write access or the call fails cleanly.
        let ok = unsafe {
            WriteProcessMemory(
                self.handle.0,
                address as usize as *const c_void,
                bytes.as_ptr().cast::<c_void>(),
                bytes.len(),
                &mut written,
            )
        };
        if ok == 0 {
            return Err(last_error("Write local endpoint"));
        }
        if written != bytes.len() {
            return Err(std::io::Error::other("Partial endpoint write"));
        }
        Ok(())
    }
}

/// Thread identifiers belonging to a process, bounded to `limit`.
pub fn thread_ids(pid: u32, limit: usize) -> std::io::Result<Vec<u32>> {
    // SAFETY: snapshot flags and a process id; the handle is checked.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(last_error("Snapshot threads"));
    }
    let snapshot = Owned(snapshot);
    // SAFETY: plain data struct; zero is valid before setting dwSize.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut ids = Vec::new();
    // SAFETY: valid snapshot handle and entry with dwSize set.
    let mut more = unsafe { Thread32First(snapshot.0, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            ids.push(entry.th32ThreadID);
            if ids.len() > limit {
                return Err(std::io::Error::other("Thread inventory bound exceeded"));
            }
        }
        // SAFETY: same handle and entry as above.
        more = unsafe { Thread32Next(snapshot.0, &mut entry) } != 0;
    }
    Ok(ids)
}

/// Basic information about a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadInfo {
    /// `STILL_ACTIVE` (259) while running.
    pub exit_status: i32,
    /// Thread environment block address.
    pub teb: u64,
    /// Owning process identifier.
    pub pid: u64,
    /// Thread identifier.
    pub tid: u64,
}

/// `THREAD_BASIC_INFORMATION` from `ntdll`, 48 bytes on x64.
#[repr(C)]
struct ThreadBasicInformation {
    exit_status: i32,
    teb: *mut c_void,
    unique_process: usize,
    unique_thread: usize,
    affinity_mask: usize,
    priority: i32,
    base_priority: i32,
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryInformationThread(
        thread: HANDLE,
        class: i32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

/// An opened thread.
pub struct Thread(Owned);

impl Thread {
    /// Open a thread for information queries only.
    pub fn open(tid: u32) -> std::io::Result<Thread> {
        // SAFETY: plain identifier and access mask; the handle is checked.
        let handle = unsafe { OpenThread(THREAD_QUERY_INFORMATION, 0, tid) };
        if handle.is_null() {
            return Err(last_error("Open game thread"));
        }
        Ok(Thread(Owned(handle)))
    }

    /// Query `ThreadBasicInformation`.
    pub fn info(&self) -> std::io::Result<ThreadInfo> {
        const _: () = assert!(std::mem::size_of::<ThreadBasicInformation>() == 48);
        // SAFETY: plain data; zero is a valid initial value.
        let mut value: ThreadBasicInformation = unsafe { std::mem::zeroed() };
        let mut returned = 0u32;
        // SAFETY: the buffer is exactly one ThreadBasicInformation and the
        // length says so; class 0 is ThreadBasicInformation.
        let status = unsafe {
            NtQueryInformationThread(
                self.0.0,
                0,
                (&mut value as *mut ThreadBasicInformation).cast::<c_void>(),
                48,
                &mut returned,
            )
        };
        if status != 0 || returned != 48 {
            return Err(std::io::Error::other(format!(
                "Thread information query failed (status {status:#x})"
            )));
        }
        Ok(ThreadInfo {
            exit_status: value.exit_status,
            teb: value.teb as u64,
            pid: value.unique_process as u64,
            tid: value.unique_thread as u64,
        })
    }
}

/// Running processes whose executable name, without extension, matches one of
/// `names` case-insensitively. Returns `(pid, matched name)`.
pub fn processes_named(names: &[&str]) -> std::io::Result<Vec<(u32, String)>> {
    // SAFETY: snapshot flags; the handle is checked.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(last_error("Snapshot processes"));
    }
    let snapshot = Owned(snapshot);
    // SAFETY: plain data struct; zero is valid before setting dwSize.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = Vec::new();
    // SAFETY: valid snapshot handle and entry with dwSize set.
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while more {
        let end = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let file = String::from_utf16_lossy(&entry.szExeFile[..end]);
        let stem = file
            .rsplit_once('.')
            .map_or(file.as_str(), |(stem, _)| stem)
            .to_owned();
        if let Some(name) = names.iter().find(|n| n.eq_ignore_ascii_case(&stem)) {
            found.push((entry.th32ProcessID, (*name).to_owned()));
        }
        // SAFETY: same handle and entry as above.
        more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    Ok(found)
}

struct WindowScan {
    pid: u32,
    visible: bool,
    close: bool,
}

unsafe extern "system" fn on_window(window: HWND, lparam: LPARAM) -> i32 {
    // SAFETY: lparam is the pointer to the WindowScan this enumeration was
    // started with, which outlives the EnumWindows call.
    let scan = unsafe { &mut *(lparam as *mut WindowScan) };
    let mut owner = 0u32;
    // SAFETY: window handle supplied by the enumeration; valid out pointer.
    unsafe { GetWindowThreadProcessId(window, &mut owner) };
    if owner != scan.pid {
        return 1;
    }
    // SAFETY: window handle supplied by the enumeration.
    let top_level = unsafe { GetWindow(window, GW_OWNER) }.is_null();
    // SAFETY: window handle supplied by the enumeration.
    let visible = unsafe { IsWindowVisible(window) } != 0;
    if top_level && visible {
        scan.visible = true;
        if scan.close {
            // SAFETY: posting a standard message to a window of the target
            // process; failures are ignored because the window may be closing.
            unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
        }
    }
    1
}

fn scan_windows(pid: u32, close: bool) -> std::io::Result<bool> {
    let mut scan = WindowScan {
        pid,
        visible: false,
        close,
    };
    // SAFETY: the callback only dereferences the scan pointer we pass, which
    // stays alive for the whole synchronous enumeration.
    let ok = unsafe { EnumWindows(Some(on_window), (&mut scan as *mut WindowScan) as LPARAM) };
    if ok == 0 {
        let error = std::io::Error::last_os_error();
        // EnumWindows reports failure when the callback stops early or when
        // there are no windows at all; neither happens here, but a desktop
        // without windows is not an error for our purpose.
        if error.raw_os_error().unwrap_or(0) != 0 {
            return Err(last_error("Enumerate windows"));
        }
    }
    Ok(scan.visible)
}

/// Whether the process owns a visible, top-level window.
pub fn has_visible_window(pid: u32) -> std::io::Result<bool> {
    scan_windows(pid, false)
}

/// Post `WM_CLOSE` to the process's visible top-level windows, as a normal
/// close request. Returns whether any window received it.
pub fn close_windows(pid: u32) -> std::io::Result<bool> {
    scan_windows(pid, true)
}

/// Owning process of the IPv4 loopback TCP listener on `port`, if any.
pub fn loopback_listener_owner(port: u16) -> std::io::Result<Option<u32>> {
    const ROW: usize = 24;
    const MAX_BYTES: u32 = 1 << 20;
    let mut size = 0u32;
    // SAFETY: a null table with zero size asks for the required size.
    let code = unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            AF_INET,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if code != ERROR_INSUFFICIENT_BUFFER || !(4..=MAX_BYTES).contains(&size) {
        return Err(std::io::Error::other("TCP ownership table unavailable"));
    }
    for _ in 0..3 {
        let mut table = vec![0u8; size as usize];
        // SAFETY: table and size describe the same allocation.
        let code = unsafe {
            GetExtendedTcpTable(
                table.as_mut_ptr().cast::<c_void>(),
                &mut size,
                0,
                AF_INET,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if code == ERROR_INSUFFICIENT_BUFFER {
            if size > MAX_BYTES {
                return Err(std::io::Error::other("TCP table bound"));
            }
            continue;
        }
        if code != 0 {
            return Err(std::io::Error::other("TCP ownership query failed"));
        }
        let count = u32::from_le_bytes([table[0], table[1], table[2], table[3]]) as usize;
        if count > 10_000 || 4 + count * ROW > table.len() {
            return Err(std::io::Error::other("TCP table length mismatch"));
        }
        for i in 0..count {
            let p = 4 + i * ROW;
            let state = u32::from_le_bytes([table[p], table[p + 1], table[p + 2], table[p + 3]]);
            let loopback = table[p + 4..p + 8] == [127, 0, 0, 1];
            // dwLocalPort holds the port in network byte order in its low bytes.
            let local_port = u16::from(table[p + 8]) << 8 | u16::from(table[p + 9]);
            let owner =
                u32::from_le_bytes([table[p + 20], table[p + 21], table[p + 22], table[p + 23]]);
            if state == TCP_STATE_LISTEN && loopback && local_port == port {
                return Ok(Some(owner));
            }
        }
        return Ok(None);
    }
    Err(std::io::Error::other("TCP table changed repeatedly"))
}

/// Read a `REG_SZ` value from the 32-bit view of `HKEY_LOCAL_MACHINE`.
/// Returns `None` when the key or value is absent; any other type is an error.
pub fn local_machine_string_32(subkey: &str, value: &str) -> std::io::Result<Option<String>> {
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const ERROR_MORE_DATA: u32 = 234;
    let subkey = utf16z(subkey);
    let value = utf16z(value);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: valid NUL-terminated UTF-16 subkey and a valid HKEY out pointer.
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_32KEY,
            &mut key,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if code != 0 {
        return Err(std::io::Error::from_raw_os_error(code as i32));
    }
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the key was opened by RegOpenKeyExW and is closed once.
            unsafe { RegCloseKey(self.0) };
        }
    }
    let key = Key(key);
    let mut kind = 0u32;
    let mut size = 0u32;
    // SAFETY: null data with zero size asks for the type and required size.
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            value.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if code != 0 && code != ERROR_MORE_DATA {
        return Err(std::io::Error::from_raw_os_error(code as i32));
    }
    if kind != REG_SZ {
        return Err(std::io::Error::other("registry value is not a string"));
    }
    if size as usize > PATH_UNITS * 2 {
        return Err(std::io::Error::other("registry value exceeds its bound"));
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: data and size describe the same allocation.
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            value.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        )
    };
    if code != 0 {
        return Err(std::io::Error::from_raw_os_error(code as i32));
    }
    let units: Vec<u16> = data[..size as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&c| c != 0)
        .collect();
    Ok(Some(String::from_utf16_lossy(&units)))
}

unsafe extern "system" fn on_console_event(_event: u32) -> i32 {
    super::STOP_REQUESTED.store(true, Ordering::SeqCst);
    1
}

/// Route Ctrl+C, Ctrl+Break and console close into [`super::STOP_REQUESTED`].
pub fn capture_stop_requests() -> std::io::Result<()> {
    // SAFETY: registering a plain extern "system" callback that only performs
    // an atomic store.
    if unsafe { SetConsoleCtrlHandler(Some(on_console_event), 1) } == 0 {
        return Err(last_error("Set console control handler"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_process_is_observable() {
        let me = Process::open(std::process::id(), false).unwrap();
        assert_eq!(me.wait(0).unwrap(), Wait::Running);
        assert!(me.creation_time().unwrap() > 0);
        assert_eq!(me.image_path().unwrap(), std::env::current_exe().unwrap());
        let (base, size, path) = me.main_module().unwrap();
        assert!(base > 0 && size > 0);
        assert_eq!(path, std::env::current_exe().unwrap());
        assert!(!thread_ids(std::process::id(), 1024).unwrap().is_empty());
        assert!(process_exists(std::process::id()));
        assert!(!process_exists(0));
    }

    #[test]
    fn thread_information_identifies_our_own_thread() {
        let tid = thread_ids(std::process::id(), 1024).unwrap()[0];
        let info = Thread::open(tid).unwrap().info().unwrap();
        assert_eq!(info.pid, u64::from(std::process::id()));
        assert_eq!(info.tid, u64::from(tid));
        assert!(info.teb > 0);
    }

    #[test]
    fn memory_reads_are_exact_and_writes_need_write_access() {
        let me = Process::open(std::process::id(), false).unwrap();
        let data = [7u8, 8, 9, 10];
        let mut out = [0u8; 4];
        me.read_memory(data.as_ptr() as u64, &mut out).unwrap();
        assert_eq!(out, data);
        assert!(me.write_memory(data.as_ptr() as u64, &[1]).is_err());
    }

    #[test]
    fn listener_ownership_is_reported_for_our_own_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(
            loopback_listener_owner(port).unwrap(),
            Some(std::process::id())
        );
        drop(listener);
        assert_eq!(loopback_listener_owner(port).unwrap(), None);
    }

    #[test]
    fn absent_registry_values_are_none_not_errors() {
        assert_eq!(
            local_machine_string_32("SOFTWARE\\Afterhours\\DoesNotExist", "Nothing").unwrap(),
            None
        );
    }

    #[test]
    fn job_lists_its_members_and_disarms() {
        if in_job().unwrap() {
            // Hosted runners and IDEs already place us in a job; the job test
            // needs a clean process and is covered by the launcher's own tests.
            return;
        }
        let job = Job::attach_current_process().unwrap();
        assert_eq!(job.process_ids().unwrap(), vec![std::process::id()]);
        let me = Process::open(std::process::id(), false).unwrap();
        assert!(job.contains(&me).unwrap());
        job.disarm().unwrap();
    }
}
