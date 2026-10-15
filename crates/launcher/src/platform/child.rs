// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A child process with bounded log pumps, shared by every platform.
//!
//! Standard output and standard error are redirected to pipes and drained
//! by two threads so a slow console can never block the child. Each stream
//! is written to its own log file up to [`MAX_LOG_BYTES`]; after that the
//! pump keeps draining and discards, and marks the output truncated. The
//! first line of standard output can be captured as a readiness record, and
//! standard error is also queued, bounded, for the supervisor to echo.

use super::{ChildProcess, SpawnSpec};
use crate::exit::{Failure, Result};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

/// Bytes retained per log file.
pub const MAX_LOG_BYTES: usize = 4 * 1024 * 1024;
/// Longest readiness line accepted.
pub const MAX_FIRST_LINE_BYTES: usize = 65_536;
/// Diagnostic chunks queued for the console before dropping.
const MAX_QUEUED_CHUNKS: usize = 128;

#[derive(Default)]
struct Shared {
    first_line: Option<std::result::Result<String, String>>,
    diagnostics: VecDeque<String>,
    dropped: bool,
    output_truncated: bool,
    fault: Option<String>,
}

/// The standard child used on every platform.
pub struct StdChild {
    child: std::process::Child,
    shared: Arc<Mutex<Shared>>,
    pumps: Vec<JoinHandle<()>>,
    status: Option<std::process::ExitStatus>,
}

impl StdChild {
    /// Start the process described by `spec`.
    pub fn spawn(spec: &SpawnSpec) -> Result<StdChild> {
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.arguments)
            .current_dir(&spec.working_directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &spec.environment {
            command.env(key, value);
        }
        let stdout_log = open_log(&spec.stdout_log)?;
        let stderr_log = open_log(&spec.stderr_log)?;
        let mut child = command
            .spawn()
            .map_err(|e| Failure::session(format!("Process failed to start: {e}")))?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Failure::internal("no stdout pipe"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Failure::internal("no stderr pipe"))?;
        let pumps = vec![
            pump(
                stdout,
                stdout_log,
                Arc::clone(&shared),
                spec.capture_first_line,
                false,
            ),
            pump(stderr, stderr_log, Arc::clone(&shared), false, true),
        ];
        Ok(StdChild {
            child,
            shared,
            pumps,
            status: None,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn poll_status(&mut self) -> Option<std::process::ExitStatus> {
        if self.status.is_none() {
            self.status = self.child.try_wait().ok().flatten();
        }
        self.status
    }
}

fn open_log(path: &std::path::Path) -> Result<std::fs::File> {
    Ok(std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?)
}

fn pump(
    mut reader: impl Read + Send + 'static,
    mut log: std::fs::File,
    shared: Arc<Mutex<Shared>>,
    capture_first: bool,
    queue_diagnostics: bool,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        let mut kept = 0usize;
        let mut line: Vec<u8> = Vec::new();
        let mut found = !capture_first;
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(e) => {
                    let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                    s.fault = Some(format!("Could not drain child diagnostics: {e}"));
                    break;
                }
            };
            let chunk = &buffer[..count];
            let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
            if queue_diagnostics {
                if s.diagnostics.len() < MAX_QUEUED_CHUNKS {
                    s.diagnostics
                        .push_back(String::from_utf8_lossy(chunk).into_owned());
                } else {
                    s.dropped = true;
                }
            }
            if !found {
                for &byte in chunk {
                    if byte == b'\n' {
                        let text = String::from_utf8_lossy(&line)
                            .trim_end_matches('\r')
                            .to_owned();
                        s.first_line = Some(Ok(text));
                        found = true;
                        break;
                    }
                    if line.len() >= MAX_FIRST_LINE_BYTES {
                        s.first_line = Some(Err("Readiness line exceeded its bound".into()));
                        found = true;
                        break;
                    }
                    line.push(byte);
                }
            }
            let write = count.min(MAX_LOG_BYTES.saturating_sub(kept));
            if write > 0 {
                if let Err(e) = log.write_all(&chunk[..write]).and_then(|()| log.flush()) {
                    s.fault = Some(format!("Could not write child log: {e}"));
                    break;
                }
                kept += write;
            }
            if write < count && capture_first {
                s.output_truncated = true;
            }
        }
        let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
        if !found && s.first_line.is_none() {
            s.first_line = Some(Err("Process exited without readiness output".into()));
        }
    })
}

impl ChildProcess for StdChild {
    fn id(&self) -> u32 {
        self.child.id()
    }
    fn exited(&mut self) -> bool {
        self.poll_status().is_some()
    }
    fn exit_code(&mut self) -> Option<i32> {
        self.poll_status().and_then(|status| status.code())
    }
    fn first_line(&mut self) -> Option<Result<String>> {
        self.lock()
            .first_line
            .clone()
            .map(|r| r.map_err(Failure::session))
    }
    fn drain_diagnostics(&mut self) -> String {
        let mut s = self.lock();
        let mut text = String::new();
        if std::mem::take(&mut s.dropped) {
            text.push_str(
                "[nfs-afterhours: console output skipped while busy; inspect the bounded session log.]\n",
            );
        }
        for _ in 0..16 {
            match s.diagnostics.pop_front() {
                Some(chunk) => text.push_str(&chunk),
                None => break,
            }
        }
        text
    }
    fn logs_finished(&self) -> bool {
        self.pumps.iter().all(JoinHandle::is_finished)
    }
    fn output_truncated(&self) -> bool {
        self.lock().output_truncated
    }
    fn check_logs(&self) -> Result<()> {
        match &self.lock().fault {
            Some(fault) => Err(Failure::session(fault.clone())),
            None => Ok(()),
        }
    }
    fn request_close(&mut self) -> bool {
        #[cfg(windows)]
        {
            nfs_afterhours_sys::windows::close_windows(self.child.id()).unwrap_or(false)
        }
        #[cfg(unix)]
        {
            nfs_afterhours_sys::unix::terminate(self.child.id()).is_ok()
        }
        #[cfg(not(any(windows, unix)))]
        {
            false
        }
    }
    fn kill(&mut self) {
        if self.poll_status().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small script run by the system shell: `sh` on Unix, `cmd` on
    /// Windows. The script stands in for a server that prints a readiness
    /// line, writes diagnostics and exits with a code.
    fn spec(dir: &std::path::Path, mode: &str, script: &str) -> SpawnSpec {
        let (executable, mut arguments, path) = if cfg!(windows) {
            let path = dir.join(format!("{mode}.cmd"));
            ("cmd".to_owned(), vec!["/C".to_owned()], path)
        } else {
            let path = dir.join(format!("{mode}.sh"));
            ("sh".to_owned(), Vec::new(), path)
        };
        std::fs::write(&path, script).unwrap();
        arguments.push(path.to_string_lossy().into_owned());
        SpawnSpec {
            executable: executable.into(),
            arguments,
            working_directory: dir.to_path_buf(),
            stdout_log: dir.join(format!("{mode}.stdout.log")),
            stderr_log: dir.join(format!("{mode}.stderr.log")),
            environment: Vec::new(),
            capture_first_line: true,
        }
    }

    fn ready_script() -> &'static str {
        if cfg!(windows) {
            "@echo off\r\necho {\"ready\":true}\r\necho second line\r\necho diag 1>&2\r\nexit 0\r\n"
        } else {
            "printf '{\"ready\":true}\\nsecond line\\n'\necho diag >&2\nexit 0\n"
        }
    }

    fn silent_script() -> &'static str {
        if cfg!(windows) {
            "@echo off\r\nexit 3\r\n"
        } else {
            "exit 3\n"
        }
    }

    /// Prints a file of `MAX_LOG_BYTES + 1` bytes and no newline until the end.
    fn loud_script(dir: &std::path::Path) -> String {
        let big = dir.join("big.txt");
        let mut bytes = vec![b'x'; MAX_LOG_BYTES + 1];
        bytes.push(b'\n');
        std::fs::write(&big, bytes).unwrap();
        if cfg!(windows) {
            format!("@echo off\r\ntype \"{}\"\r\nexit 0\r\n", big.display())
        } else {
            format!("cat '{}'\nexit 0\n", big.display())
        }
    }

    fn wait_exit(child: &mut StdChild) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !child.exited() || !child.logs_finished() {
            assert!(std::time::Instant::now() < deadline, "child did not finish");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn readiness_line_diagnostics_and_logs_are_captured() {
        let dir = crate::testing::scratch("child");
        let mut child = StdChild::spawn(&spec(&dir, "ready", ready_script())).unwrap();
        wait_exit(&mut child);
        assert_eq!(child.exit_code(), Some(0));
        assert_eq!(child.first_line().unwrap().unwrap(), "{\"ready\":true}");
        child.check_logs().unwrap();
        assert!(!child.output_truncated());
        let out = std::fs::read_to_string(dir.join("ready.stdout.log")).unwrap();
        assert!(out.contains("second line"));
        let mut diagnostics = String::new();
        for _ in 0..50 {
            diagnostics.push_str(&child.drain_diagnostics());
            if diagnostics.contains("diag") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(diagnostics.contains("diag"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn exit_without_output_is_a_readiness_error_with_the_exit_code() {
        let dir = crate::testing::scratch("child");
        let mut child = StdChild::spawn(&spec(&dir, "silent", silent_script())).unwrap();
        wait_exit(&mut child);
        assert_eq!(child.exit_code(), Some(3));
        assert!(child.first_line().unwrap().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn output_beyond_the_bound_is_truncated_but_fully_drained() {
        let dir = crate::testing::scratch("child");
        let mut child = StdChild::spawn(&spec(&dir, "loud", &loud_script(&dir))).unwrap();
        wait_exit(&mut child);
        assert_eq!(child.exit_code(), Some(0));
        assert!(child.output_truncated());
        assert!(
            child.first_line().unwrap().is_err(),
            "the line exceeded its bound"
        );
        let len = std::fs::metadata(dir.join("loud.stdout.log"))
            .unwrap()
            .len();
        assert_eq!(len as usize, MAX_LOG_BYTES);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_executables_fail_as_session_errors_and_kill_is_idempotent() {
        let dir = crate::testing::scratch("child");
        let mut bad = spec(&dir, "none", silent_script());
        bad.executable = dir.join("does-not-exist");
        let error = match StdChild::spawn(&bad) {
            Err(error) => error,
            Ok(_) => panic!("a missing executable must not spawn"),
        };
        assert_eq!(error.exit, crate::exit::Exit::SessionFailed);
        let mut child = StdChild::spawn(&spec(&dir, "ready", ready_script())).unwrap();
        child.kill();
        child.kill();
        assert!(child.exited());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
