// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unix calls: privilege check, process liveness and signals.

use std::sync::atomic::Ordering;

/// Whether the effective user is root.
pub fn is_root() -> bool {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// Whether a process exists. A process we may not signal still exists.
pub fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill with signal 0 performs only the existence and permission
    // checks; it sends nothing.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Ask a process to terminate with `SIGTERM`.
pub fn terminate(pid: u32) -> std::io::Result<()> {
    let pid = libc::pid_t::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| std::io::Error::other("invalid process identifier"))?;
    // SAFETY: kill with a valid positive pid and a standard signal number.
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

extern "C" fn on_signal(_signal: libc::c_int) {
    // Only an atomic store: async-signal-safe.
    super::STOP_REQUESTED.store(true, Ordering::SeqCst);
}

/// Install handlers for `SIGINT` and `SIGTERM` that set [`super::STOP_REQUESTED`].
pub fn capture_stop_requests() -> std::io::Result<()> {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: zeroed sigaction is a valid starting value; the handler is a
        // plain extern "C" fn that only performs an atomic store.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        let handler: extern "C" fn(libc::c_int) = on_signal;
        action.sa_sigaction = handler as usize;
        action.sa_flags = libc::SA_RESTART;
        // SAFETY: sigemptyset on a valid, writable sigset_t.
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        // SAFETY: sigaction with a valid signal number and a fully initialized
        // struct; the old action pointer is allowed to be null.
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_process_exists_and_an_impossible_pid_does_not() {
        assert!(process_exists(std::process::id()));
        assert!(!process_exists(0));
        assert!(!process_exists(u32::MAX));
    }

    #[test]
    fn terminate_rejects_invalid_identifiers_without_signalling() {
        assert!(terminate(0).is_err());
        assert!(terminate(u32::MAX).is_err());
    }
}
