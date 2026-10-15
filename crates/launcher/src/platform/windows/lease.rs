// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The routing lease: one game lifetime, one root thread, one local server.
//!
//! This is a direct port of the C# `EndpointLease`. It pins the game and
//! server processes by handle, creation time, image path and digest, finds
//! the game's startup object through a thread-local slot on its root
//! thread, verifies three regions of the game's code by hash so the layout
//! assumptions hold, checks that the server still owns the loopback
//! listener, and then drives the [`EndpointTransaction`] over bounded
//! process-memory reads and writes.
//!
//! Every number here (offsets, hashes, sizes) describes the one supported
//! game build and is reproduced from the C# original, not derived. A
//! mismatch fails closed: the lease refuses to write, and the supervisor
//! reports the failure. No arbitrary-address write is exposed.

use crate::{
    digest,
    endpoint::{EndpointTransaction, FIELD_SIZES, Memory},
    exit::{Failure, Result},
};
use nfs_afterhours_sys::windows as sys;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

/// SHA-256 of the supported `NFS16.exe`.
pub const GAME_BUILD_SHA256: &str =
    "92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df";
/// SHA-256 of the supported `Core/ActivationUI.exe`.
pub const ACTIVATION_HELPER_SHA256: &str =
    "5901b0ad269e12deecc2bcd2eaa56f8964538a8d6b1a265651d56dbd636daefd";

/// Field offsets from the startup root: host, flag, port.
const OFFSETS: [u64; 3] = [0x546, 0x646, 0x648];
/// Code regions (module offset, length, SHA-256) that must match before any write.
const CODE: [(u64, usize, &str); 3] = [
    (
        0x03ec_ca30,
        37,
        "9814166c5e8c200613a6239d40707f02ca649953fcb5abd749457e949d902a12",
    ),
    (
        0x0436_3fd0,
        33,
        "cb794f9ae8dc1b3074eb76b8a56f67fef316e315a7ddff9343d997b6e2c0394f",
    ),
    (
        0x04c4_3ab0,
        24,
        "dfe377bcb9eac63d66e12ef5a02dce32e24847de0bae004706aebbf6cccd49eb",
    ),
];
/// Module offset of the TLS index used to reach the startup root.
const TLS_INDEX_OFFSET: u64 = 0x02c9_e2b4;
/// Module offsets the root and provider objects must point at.
const ROOT_VTABLE: u64 = 0x0239_4fb8;
const ROOT_VTABLE_2: u64 = 0x0239_4fd8;
const PROVIDER_VTABLE: u64 = 0x023d_b740;
/// Lowest and highest user-mode addresses we will read.
const ADDRESS_MIN: u64 = 0x10000;
const ADDRESS_MAX: u64 = 0x0000_7fff_ffff_0000;
/// Per-guard read budget.
const MAX_READS: u32 = 1500;
const MAX_READ_BYTES: u32 = 32_768;
const MAX_READ_SECONDS: u64 = 5;
/// `STILL_ACTIVE`.
const STILL_ACTIVE: i32 = 259;
/// Message the supervisor treats as "not yet", retrying preparation.
pub const NOT_UNPACKED_YET: &str = "Unsupported live code guard";

fn refuse(message: impl Into<String>) -> Failure {
    Failure::session(message)
}

/// A process held by handle and verified against the identity it had when
/// the launcher started it.
pub struct ProcessPin {
    process: sys::Process,
    created: u64,
    path: PathBuf,
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

impl ProcessPin {
    /// Open and verify a process, checking its executable's digest on disk.
    pub fn new(pid: u32, created: u64, path: &Path, sha256: &str, write: bool) -> Result<Self> {
        if pid == 0 || created == 0 {
            return Err(refuse("Invalid process identity"));
        }
        let process = sys::Process::open(pid, write)?;
        let pin = ProcessPin {
            process,
            created,
            path: std::path::absolute(path)?,
        };
        pin.verify()?;
        if !digest::sha256_file(&pin.path)?.eq_ignore_ascii_case(sha256) {
            return Err(refuse("Executable digest changed"));
        }
        Ok(pin)
    }

    pub fn id(&self) -> u32 {
        self.process.id()
    }

    pub fn exited(&self) -> Result<bool> {
        Ok(self.process.wait(0)? == sys::Wait::Exited)
    }

    /// The process is alive and still has the lifetime and path we pinned.
    pub fn verify(&self) -> Result<()> {
        if self.exited()? {
            return Err(refuse("Owned process exited"));
        }
        if self.process.creation_time()? != self.created
            || !same_path(&self.process.image_path()?, &self.path)
        {
            return Err(refuse("Process lifetime/path changed"));
        }
        Ok(())
    }

    /// Base and size of the main module, which must be the pinned image.
    pub fn module(&self) -> Result<(u64, u32)> {
        let (base, size, path) = self.process.main_module()?;
        if !same_path(&path, &self.path) {
            return Err(refuse("Main module path mismatch"));
        }
        Ok((base, size))
    }

    /// Wait up to `milliseconds` for the process to exit.
    pub fn wait_exit(&self, milliseconds: u32) -> Result<bool> {
        Ok(self.process.wait(milliseconds)? == sys::Wait::Exited)
    }
}

/// The lease over the routed game.
pub struct EndpointLease {
    game: ProcessPin,
    server: ProcessPin,
    port: u16,
    thread: Option<sys::Thread>,
    teb: u64,
    tid: u64,
    root: u64,
    provider: u64,
    module_base: u64,
    module_size: u32,
    tls_index: u32,
    transaction: Option<EndpointTransaction>,
    touched: bool,
    applying: bool,
    budget_started: Instant,
    reads: u32,
    bytes_read: u32,
}

impl EndpointLease {
    /// Pin both processes. `game_created` and `server_created` are the
    /// creation times observed when the launcher started them.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        game_pid: u32,
        game_created: u64,
        game_path: &Path,
        server_pid: u32,
        server_created: u64,
        server_path: &Path,
        server_sha256: &str,
        port: u16,
    ) -> Result<Self> {
        if port < 1024 {
            return Err(refuse("Unexpected local port"));
        }
        let game = ProcessPin::new(game_pid, game_created, game_path, GAME_BUILD_SHA256, true)?;
        let server = ProcessPin::new(
            server_pid,
            server_created,
            server_path,
            server_sha256,
            false,
        )?;
        Ok(Self {
            game,
            server,
            port,
            thread: None,
            teb: 0,
            tid: 0,
            root: 0,
            provider: 0,
            module_base: 0,
            module_size: 0,
            tls_index: 0,
            transaction: None,
            touched: false,
            applying: false,
            budget_started: Instant::now(),
            reads: 0,
            bytes_read: 0,
        })
    }

    fn reset_budget(&mut self) {
        self.budget_started = Instant::now();
        self.reads = 0;
        self.bytes_read = 0;
    }

    fn read(&mut self, address: u64, count: usize) -> Result<Vec<u8>> {
        self.reads += 1;
        self.bytes_read += count as u32;
        let in_range = (ADDRESS_MIN..ADDRESS_MAX).contains(&address)
            && (1..=256).contains(&count)
            && address + count as u64 <= ADDRESS_MAX;
        if !in_range
            || self.reads >= MAX_READS
            || self.bytes_read > MAX_READ_BYTES
            || self.budget_started.elapsed().as_secs() >= MAX_READ_SECONDS
        {
            return Err(refuse("Memory observation exceeds bounds"));
        }
        let mut bytes = vec![0u8; count];
        self.game.process.read_memory(address, &mut bytes)?;
        Ok(bytes)
    }

    fn q(&mut self, address: u64) -> Result<u64> {
        let bytes = self.read(address, 8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }

    fn u(&mut self, address: u64) -> Result<u32> {
        let bytes = self.read(address, 4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }

    fn thread_info(&self, thread: &sys::Thread) -> Result<sys::ThreadInfo> {
        let info = thread.info()?;
        if info.exit_status != STILL_ACTIVE || info.pid != u64::from(self.game.id()) {
            return Err(refuse("Root thread exited or changed"));
        }
        Ok(info)
    }

    fn check_code(&mut self) -> Result<()> {
        for (offset, length, expected) in CODE {
            let bytes = self.read(self.module_base + offset, length)?;
            if !digest::sha256_bytes(&bytes).eq_ignore_ascii_case(expected) {
                return Err(refuse(NOT_UNPACKED_YET));
            }
        }
        Ok(())
    }

    /// Startup root reached from a thread's TLS slot, or 0 when that thread
    /// has none.
    fn thread_root(&mut self, info: sys::ThreadInfo) -> Result<u64> {
        let teb = info.teb;
        if self.q(teb + 0x30)? != teb
            || self.q(teb + 0x40)? != u64::from(self.game.id())
            || self.q(teb + 0x48)? != info.tid
        {
            return Err(refuse("Thread environment identity mismatch"));
        }
        let array = self.q(teb + 0x58)?;
        if array == 0 {
            return Ok(0);
        }
        let block = self.q(array + u64::from(self.tls_index) * 8)?;
        if block == 0 {
            return Ok(0);
        }
        self.q(block + 0x290)
    }

    /// Call during startup, before the first backend request. `Ok(false)`
    /// means the guarded startup root is not ready yet. An error whose
    /// message is [`NOT_UNPACKED_YET`] may be retried; any other error is
    /// final.
    pub fn prepare(&mut self) -> Result<bool> {
        if self.transaction.is_some() || self.thread.is_some() {
            return Err(refuse("Lease already prepared"));
        }
        self.game.verify()?;
        self.server.verify()?;
        self.reset_budget();
        let (base, size) = self.game.module()?;
        self.module_base = base;
        self.module_size = size;
        let last_code = CODE[2].0 + CODE[2].1 as u64;
        if base < ADDRESS_MIN || u64::from(size) <= last_code || size > 0x1000_0000 {
            return Err(refuse("Unsupported module extent"));
        }
        self.check_code()?;
        self.tls_index = self.u(self.module_base + TLS_INDEX_OFFSET)?;
        if self.tls_index >= 64 {
            return Err(refuse("Unsupported TLS index"));
        }
        if self.game.process.creation_time()? != self.game.created {
            return Err(refuse("Thread inventory lifetime/bound"));
        }
        let threads = sys::thread_ids(self.game.id(), 128)
            .map_err(|_| refuse("Thread inventory lifetime/bound"))?;
        let mut selected: Option<(sys::Thread, sys::ThreadInfo)> = None;
        let mut found = 0u64;
        for tid in threads {
            let thread = sys::Thread::open(tid)?;
            let info = self.thread_info(&thread)?;
            let value = self.thread_root(info)?;
            if value == 0 {
                continue;
            }
            if found != 0 && found != value {
                return Err(refuse("Conflicting startup roots"));
            }
            found = value;
            if selected.is_none() {
                selected = Some((thread, info));
            }
        }
        let Some((thread, info)) = selected else {
            return Ok(false);
        };
        self.root = found;
        self.provider = self.q(self.root + 0x750)?;
        self.thread = Some(thread);
        self.teb = info.teb;
        self.tid = info.tid;
        let result = (|| -> Result<bool> {
            self.guard_state()?;
            let map = self.q(self.provider + 0xd48)?;
            let begin = self.q(map + 0x18)?;
            let end = self.q(map + 0x20)?;
            let capacity = self.q(map + 0x28)?;
            let shape_ok = begin == end
                && (begin != 0 || capacity == 0)
                && capacity >= end
                && (capacity - begin) % 48 == 0
                && (capacity - begin) / 48 <= 4096
                && capacity < ADDRESS_MAX;
            if !shape_ok {
                return Err(refuse(
                    "Backend startup has already begun or configuration is invalid",
                ));
            }
            let port = self.port;
            let transaction = EndpointTransaction::new(self, port)?;
            self.transaction = Some(transaction);
            self.check_listener()?;
            Ok(true)
        })();
        if result.is_err() {
            self.thread = None;
            self.transaction = None;
        }
        result
    }

    /// Every identity the lease relies on is unchanged.
    fn guard_state(&mut self) -> Result<()> {
        self.game.verify()?;
        self.reset_budget();
        if self.game.module()? != (self.module_base, self.module_size) {
            return Err(refuse("Main module changed"));
        }
        self.check_code()?;
        let thread = self
            .thread
            .as_ref()
            .ok_or_else(|| refuse("Lease not prepared"))?;
        let info = thread.info()?;
        if info.exit_status != STILL_ACTIVE || info.pid != u64::from(self.game.id()) {
            return Err(refuse("Root thread exited or changed"));
        }
        if info.teb != self.teb
            || info.tid != self.tid
            || self.u(self.module_base + TLS_INDEX_OFFSET)? != self.tls_index
            || self.thread_root(info)? != self.root
        {
            return Err(refuse("Startup root changed"));
        }
        let base = self.module_base;
        let identity_ok = self.q(self.root)? == base + ROOT_VTABLE
            && self.q(self.root + 8)? == base + ROOT_VTABLE_2
            && self.q(self.root + 0x750)? == self.provider
            && self.q(self.provider)? == base + PROVIDER_VTABLE
            && self.q(self.provider + 8)? == self.root
            && self.u(self.root + 0x528)? == 0
            && self.u(self.root + 0x530)? == 3;
        if !identity_ok {
            return Err(refuse("Startup object identity changed"));
        }
        Ok(())
    }

    /// The pinned server still owns the loopback listener on our port.
    fn check_listener(&self) -> Result<()> {
        self.server.verify()?;
        match sys::loopback_listener_owner(self.port)? {
            Some(owner) if owner == self.server.id() => Ok(()),
            _ => Err(refuse(
                "Expected server no longer owns the loopback listener",
            )),
        }
    }

    /// Write the endpoint. Single use; a failure rolls back before returning.
    pub fn apply(&mut self) -> Result<()> {
        let transaction = match (&self.transaction, self.touched) {
            (Some(transaction), false) => transaction.clone(),
            _ => return Err(refuse("Lease not prepared or already applied")),
        };
        self.check_listener()?;
        self.touched = true;
        self.applying = true;
        let result = transaction.apply(self);
        self.applying = false;
        if result.is_err() {
            let _ = self.restore();
        }
        result
    }

    /// Restore the original bytes if we changed them and the game is alive.
    pub fn restore(&mut self) -> Result<()> {
        if !self.touched {
            return Ok(());
        }
        let Some(transaction) = self.transaction.clone() else {
            return Ok(());
        };
        if !self.game.exited()? {
            transaction.restore(self)?;
            if !transaction.is_original(self)? {
                return Err(refuse("Independent rollback check failed"));
            }
        }
        self.touched = false;
        Ok(())
    }

    /// After the game exits, its address space and our change are gone.
    /// Shutdown can retire the root thread before the process handle
    /// signals, so only the handle proves the lifetime ended.
    pub fn complete_after_exit(&mut self) -> Result<()> {
        if !self.game.wait_exit(10_000)? {
            return Err(refuse("Game lifetime has not ended"));
        }
        self.touched = false;
        Ok(())
    }
}

impl Memory for EndpointLease {
    fn read(&mut self, field: usize) -> Result<Vec<u8>> {
        let address = self.root + OFFSETS[field];
        EndpointLease::read(self, address, FIELD_SIZES[field])
    }

    fn write(&mut self, field: usize, value: &[u8]) -> Result<()> {
        if field >= 3 || value.len() != FIELD_SIZES[field] {
            return Err(refuse("Endpoint write extent mismatch"));
        }
        if self.applying {
            self.check_listener()?;
        }
        self.game
            .process
            .write_memory(self.root + OFFSETS[field], value)?;
        Ok(())
    }

    fn guard(&mut self) -> Result<()> {
        self.guard_state()
    }
}

impl Drop for EndpointLease {
    fn drop(&mut self) {
        // Best effort: a failed restore here has already been reported by
        // the supervisor's own restore call.
        let _ = self.restore();
    }
}
