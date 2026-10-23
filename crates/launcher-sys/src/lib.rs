// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Operating-system calls for `nfs-afterhours`, behind safe wrappers.
//!
//! This is the only first-party crate in the workspace allowed to contain
//! `unsafe`. It exposes small, safe functions and types; the launcher never
//! sees a raw handle or pointer. Every operation is bounded: buffers have
//! fixed maximum sizes, memory reads and writes take explicit lengths, and
//! nothing here attaches a debugger, changes memory protection, writes the
//! registry or alters a service, firewall or hosts setting.
//!
//! The Windows half mirrors the C# helpers the PowerShell launcher compiled
//! at runtime: job objects, process pins, thread lookup, bounded process
//! memory access, the TCP ownership table, a 32-bit registry read, window
//! detection and console control events. The Unix half is much smaller:
//! root detection, process liveness, `SIGTERM`, and `SIGINT`/`SIGTERM`
//! capture into a flag.

#![cfg_attr(not(any(windows, unix)), allow(unused))]

use std::sync::atomic::AtomicBool;

#[cfg(windows)]
pub mod windows;

#[cfg(unix)]
pub mod unix;

/// Set once the user asks the process to stop (Ctrl+C, console close or
/// `SIGTERM`). Polled by the supervisor loop.
pub static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Route Ctrl+C and termination requests into [`STOP_REQUESTED`] instead of
/// killing the process, so the supervisor can shut children down in order.
pub fn capture_stop_requests() -> std::io::Result<()> {
    #[cfg(windows)]
    {
        windows::capture_stop_requests()
    }
    #[cfg(unix)]
    {
        unix::capture_stop_requests()
    }
    #[cfg(not(any(windows, unix)))]
    {
        Ok(())
    }
}

/// Whether the process runs with privileges it should not have: an elevated
/// token on Windows, effective user ID zero on Unix.
pub fn is_privileged() -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        windows::is_elevated()
    }
    #[cfg(unix)]
    {
        Ok(unix::is_root())
    }
    #[cfg(not(any(windows, unix)))]
    {
        Ok(false)
    }
}

/// Fill `buffer` from the operating system's cryptographic random source:
/// `BCryptGenRandom` with the system-preferred generator on Windows,
/// `/dev/urandom` elsewhere. For secrets such as an API token.
pub fn random_bytes(buffer: &mut [u8]) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        windows::random_bytes(buffer)
    }
    #[cfg(unix)]
    {
        unix::random_bytes(buffer)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = buffer;
        Err(std::io::Error::other("No random source on this platform."))
    }
}

/// Whether a process with this identifier currently exists.
pub fn process_exists(pid: u32) -> bool {
    #[cfg(windows)]
    {
        windows::process_exists(pid)
    }
    #[cfg(unix)]
    {
        unix::process_exists(pid)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        false
    }
}
