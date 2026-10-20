// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the supervisor needs from the operating system.
//!
//! The supervisor never calls the OS directly. It asks a [`Platform`] for
//! processes, privilege checks and conflict checks, and drives the children
//! it gets back through [`ChildProcess`]. This keeps the decision logic
//! portable and testable with fakes, and keeps every OS call in one place
//! per platform.
//!
//! Windows additionally places the supervisor and its children in a job
//! object with kill-on-close, because `start` leaves a routed game process
//! and a renamed server running and those must not outlive the supervisor.
//! Unix has no such guarantee by design: `serve` has no game and no memory
//! lease, so a supervisor crash leaving the server running is ordinary
//! daemon behavior that `status` reports.

use crate::exit::Result;
use std::path::{Path, PathBuf};

/// What a bounded log file keeps once it reaches its bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// Its beginning: later output is drained and discarded, and the child
    /// reports its output truncated. For a result such as a JSON document,
    /// which is useless without its start.
    Start,
    /// Its newest output: the oldest part is removed, so the end of a long
    /// session, where a crash shows, is always there. For logs.
    End,
}

/// How to start a child.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    /// Bounded log file for standard output.
    pub stdout_log: PathBuf,
    /// What the standard output log keeps when full. The standard error log
    /// always keeps its newest output.
    pub stdout_keeps: Keep,
    /// Bounded log file for standard error.
    pub stderr_log: PathBuf,
    /// Extra environment variables, such as the launch marker.
    pub environment: Vec<(String, String)>,
    /// Whether the first line of standard output is a readiness record.
    pub capture_first_line: bool,
}

/// A running child the supervisor owns.
pub trait ChildProcess {
    /// Process identifier.
    fn id(&self) -> u32;
    /// Whether the process has exited.
    fn exited(&mut self) -> bool;
    /// Exit code once exited. `None` while running or when killed by a signal.
    fn exit_code(&mut self) -> Option<i32>;
    /// First line of standard output, once seen. `Err` if the process exited
    /// first or the line exceeded its bound.
    fn first_line(&mut self) -> Option<Result<String>>;
    /// Diagnostics (standard error) captured since the last call, bounded.
    fn drain_diagnostics(&mut self) -> String;
    /// Whether both log pumps have finished after exit.
    fn logs_finished(&self) -> bool;
    /// Whether standard output exceeded the log bound and, kept from its
    /// start ([`Keep::Start`]), lost its end.
    fn output_truncated(&self) -> bool;
    /// Any fault in the log pumps.
    fn check_logs(&self) -> Result<()>;
    /// Ask the process to close normally: close its main window on Windows,
    /// `SIGTERM` elsewhere. Returns whether the request was delivered.
    fn request_close(&mut self) -> bool;
    /// Terminate the process and its descendants.
    fn kill(&mut self);
}

/// A guard around the whole session's process tree.
pub trait SessionGuard {
    /// Disarm the guard once every child has exited. On error the guard
    /// stays armed and process exit performs the cleanup.
    fn finish(&mut self) -> Result<()>;
}

/// Operating-system operations, one implementation per platform.
pub trait Platform {
    /// Refuse to run with elevated or root privileges.
    fn refuse_privileged(&self) -> Result<()>;
    /// The installed client registration, if this platform has one.
    fn registered_client_path(&self) -> Result<Option<String>>;
    /// Names of running processes among `names` (without extension).
    fn running_processes(&self, names: &[&str]) -> Result<Vec<String>>;
    /// Whether a process with this identifier is alive.
    fn process_alive(&self, pid: u32) -> bool;
    /// Attach the current process to a session guard. Call before spawning.
    fn attach_session_guard(&self) -> Result<Box<dyn SessionGuard>>;
    /// Start a child with redirected, bounded output.
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn ChildProcess>>;
}

/// Probe whether the loopback TCP port is free by binding it briefly.
pub fn loopback_port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// Default data directory for this user on this platform.
pub fn default_data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("Afterhours2015"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support/Afterhours2015"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .map(|base| base.join("afterhours2015"))
    }
}

/// The game's own registration: its installation directory and the locale
/// the game runs in, from the 32-bit `SOFTWARE\EA Games\Need for Speed`
/// key. Read only; both are `None` where there is no such registry or value.
pub fn game_registration() -> (Option<PathBuf>, Option<String>) {
    #[cfg(windows)]
    {
        const KEY: &str = "SOFTWARE\\EA Games\\Need for Speed";
        let read = |value| {
            nfs_afterhours_sys::windows::local_machine_string_32(KEY, value)
                .ok()
                .flatten()
        };
        (read("Install Dir").map(PathBuf::from), read("Locale"))
    }
    #[cfg(not(windows))]
    {
        (None, None)
    }
}

/// The folder where the game writes `CrashDump_*.dmp`:
/// `Documents\Need For Speed\Temp`. The Documents folder is looked up from
/// the environment (the profile's `Documents`, then a OneDrive one), so a
/// Documents folder redirected elsewhere is not found. `None` where the
/// game does not run.
pub fn game_crash_dump_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let candidates: Vec<PathBuf> = ["USERPROFILE", "OneDrive", "OneDriveConsumer"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|base| {
                PathBuf::from(base)
                    .join("Documents")
                    .join("Need For Speed")
                    .join("Temp")
            })
            .collect();
        candidates
            .iter()
            .find(|dir| dir.is_dir())
            .or(candidates.first())
            .cloned()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The EA Desktop log folder, `%LOCALAPPDATA%\Electronic Arts\EA Desktop\Logs`.
/// `None` where the game does not run.
pub fn ea_desktop_logs_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|base| {
            PathBuf::from(base)
                .join("Electronic Arts")
                .join("EA Desktop")
                .join("Logs")
        })
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Default server executable: `nfs-server` next to this executable, since
/// both are built by the same `cargo build`.
pub fn default_server_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = format!("nfs-server{}", std::env::consts::EXE_SUFFIX);
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

/// Executable name of the installed game.
pub const GAME_EXECUTABLE: &str = "NFS16.exe";
/// Relative path of the installed activation helper.
pub const ACTIVATION_HELPER: &str = "Core/ActivationUI.exe";
/// Process names that must not be running when a session starts.
pub const CONFLICTING_PROCESSES: &[&str] = &[
    "NFS16",
    "NFS16_trial",
    "EADesktop",
    "Origin",
    "EADesktopDebug",
    "OriginDebug",
];
/// Processes that keep `play` from starting the game: the game itself and the
/// EA applications. The `Debug` names of [`CONFLICTING_PROCESSES`] are left
/// out because a serving session's own server runs under one of them.
pub const PLAY_CONFLICTS: &[&str] = &["NFS16", "NFS16_trial", "EADesktop", "Origin"];
/// Loopback port the game expects the launcher services on.
pub const LSX_PORT: u16 = 3216;

/// Whether this build can start and route the installed game.
pub fn supports_game_launch() -> bool {
    cfg!(windows)
}

/// Shared helper: resolve a path that must exist to an absolute path.
pub fn existing_absolute(path: &Path, what: &str) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if !absolute.exists() {
        return Err(crate::exit::Failure::refused(format!(
            "{what} not found: {}",
            absolute.display()
        )));
    }
    Ok(absolute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_conflicts_never_include_the_compatibility_server_name() {
        for (_, name) in [
            (
                "eadesktop",
                crate::compat::compatibility_name(r"C:\EA\EADesktop.exe"),
            ),
            (
                "origin",
                crate::compat::compatibility_name(r"C:\Origin\Origin.exe"),
            ),
        ] {
            let stem = name.unwrap().trim_end_matches(".exe");
            assert!(CONFLICTING_PROCESSES.contains(&stem), "{stem}");
            assert!(!PLAY_CONFLICTS.contains(&stem), "{stem}");
        }
        for name in PLAY_CONFLICTS {
            assert!(CONFLICTING_PROCESSES.contains(name), "{name}");
        }
    }

    #[test]
    fn a_bound_port_is_reported_busy_and_a_free_one_free() {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!loopback_port_is_free(port));
        drop(listener);
        assert!(loopback_port_is_free(port));
    }

    #[test]
    fn default_server_path_only_reports_an_existing_sibling() {
        // The test binary's directory has no nfs-server next to it.
        if let Some(path) = default_server_path() {
            assert!(path.is_file());
        }
    }
}

pub mod child;

#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

/// The platform this binary runs on.
#[cfg(windows)]
pub fn native() -> impl Platform {
    windows::WindowsPlatform
}

/// The platform this binary runs on.
#[cfg(unix)]
pub fn native() -> impl Platform {
    unix::UnixPlatform
}
