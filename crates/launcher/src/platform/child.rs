// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A child process with bounded log pumps, shared by every platform.
//!
//! Standard output and standard error are redirected to pipes and drained
//! by two threads so a slow console can never block the child. Each stream
//! is written to its own log file of at most [`MAX_LOG_BYTES`]. A log that
//! keeps its newest output ([`Keep::End`], every log) is trimmed in place
//! when full: its oldest part is removed and one line at the top says how
//! much, so the end, where a crash shows, is always on disk. A log kept
//! from its start ([`Keep::Start`], JSON results) stops growing instead:
//! the pump keeps draining and discards, and marks the output truncated.
//! The first line of standard output can be captured as a readiness record,
//! and standard error is also queued, bounded, for the supervisor to echo.

use super::{ChildProcess, Keep, SpawnSpec};
use crate::exit::{Failure, Result};
use std::{
    collections::VecDeque,
    io::{Read, Seek, SeekFrom, Write},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

/// Largest size of a log file.
pub const MAX_LOG_BYTES: usize = 4 * 1024 * 1024;
/// Newest output a full [`Keep::End`] log keeps when it is trimmed; the rest
/// of the bound is room for new output, so a log is trimmed about once per
/// MiB written.
pub const RETAINED_LOG_BYTES: usize = 3 * 1024 * 1024;
/// How far into the kept part a trim looks for a line start, so it does not
/// begin in the middle of a line.
const LINE_SEARCH_BYTES: usize = 64 * 1024;
/// Longest readiness line accepted.
pub const MAX_FIRST_LINE_BYTES: usize = 65_536;
/// Diagnostic chunks queued for the console before dropping.
const MAX_QUEUED_CHUNKS: usize = 128;

#[derive(Default)]
struct Shared {
    first_line: Option<std::result::Result<String, String>>,
    /// The newest map frame line after the readiness line.
    map_feed: crate::map_feed::Latch,
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
                spec.stdout_keeps,
                Arc::clone(&shared),
                spec.capture_first_line,
                spec.map_feed,
                false,
            ),
            pump(
                stderr,
                stderr_log,
                Keep::End,
                Arc::clone(&shared),
                false,
                false,
                true,
            ),
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
    // Read access lets a full log be trimmed in place.
    Ok(std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?)
}

/// A full log trimmed so far: output bytes removed and the length of the
/// line at its top that says so.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Trimmed {
    removed: u64,
    notice: u64,
}

/// Remove the oldest part of a full log in place. Keeps its newest
/// [`RETAINED_LOG_BYTES`], from the first line start among them when one is
/// near, behind one line saying how many bytes of output have been removed
/// in all. Returns the new length.
fn trim_to_newest(log: &mut std::fs::File, trimmed: &mut Trimmed) -> std::io::Result<usize> {
    log.flush()?;
    let len = log.seek(SeekFrom::End(0))?;
    let start = len.saturating_sub(RETAINED_LOG_BYTES as u64);
    log.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::with_capacity(RETAINED_LOG_BYTES);
    Read::by_ref(log)
        .take(RETAINED_LOG_BYTES as u64)
        .read_to_end(&mut tail)?;
    let cut = tail
        .iter()
        .take(LINE_SEARCH_BYTES)
        .position(|&byte| byte == b'\n')
        .map_or(0, |at| at + 1);
    // The previous notice sits at the top and is removed with the rest.
    let dropped = start + cut as u64;
    trimmed.removed += dropped.saturating_sub(trimmed.notice);
    let notice = format!(
        "[nfs-afterhours: {} earlier bytes removed to keep this log within {} MiB]\n",
        trimmed.removed,
        MAX_LOG_BYTES / (1024 * 1024)
    );
    trimmed.notice = notice.len() as u64;
    log.set_len(0)?;
    log.seek(SeekFrom::Start(0))?;
    log.write_all(notice.as_bytes())?;
    log.write_all(&tail[cut..])?;
    log.flush()?;
    Ok(notice.len() + tail.len() - cut)
}

fn pump(
    mut reader: impl Read + Send + 'static,
    mut log: std::fs::File,
    keep: Keep,
    shared: Arc<Mutex<Shared>>,
    capture_first: bool,
    map_feed: bool,
    queue_diagnostics: bool,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        let mut kept = 0usize;
        let mut trimmed = Trimmed::default();
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
            // What goes to the log: with a map feed, only up to the end of
            // the readiness line; the frames after it are kept in memory.
            let mut logged = chunk;
            if !found {
                for (at, &byte) in chunk.iter().enumerate() {
                    if byte == b'\n' {
                        let text = String::from_utf8_lossy(&line)
                            .trim_end_matches('\r')
                            .to_owned();
                        s.first_line = Some(Ok(text));
                        found = true;
                        if map_feed {
                            logged = &chunk[..=at];
                            s.map_feed.push(&chunk[at + 1..]);
                        }
                        break;
                    }
                    if line.len() >= MAX_FIRST_LINE_BYTES {
                        s.first_line = Some(Err("Readiness line exceeded its bound".into()));
                        found = true;
                        break;
                    }
                    line.push(byte);
                }
            } else if map_feed {
                s.map_feed.push(chunk);
                logged = &[];
            }
            let (chunk, count) = (logged, logged.len());
            if count == 0 {
                continue;
            }
            if keep == Keep::End && kept + count > MAX_LOG_BYTES {
                match trim_to_newest(&mut log, &mut trimmed) {
                    Ok(len) => kept = len,
                    Err(e) => {
                        s.fault = Some(format!("Could not trim child log: {e}"));
                        break;
                    }
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
            if write < count {
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
    fn take_map_feed(&mut self) -> Option<String> {
        self.lock().map_feed.take()
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
        spec_keeping(dir, mode, script, Keep::Start)
    }

    fn spec_keeping(dir: &std::path::Path, mode: &str, script: &str, keep: Keep) -> SpawnSpec {
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
            stdout_keeps: keep,
            stderr_log: dir.join(format!("{mode}.stderr.log")),
            environment: Vec::new(),
            capture_first_line: true,
            map_feed: false,
        }
    }

    /// `count` numbered lines, `line 0000000\n` and so on, 13 bytes each.
    fn numbered(from: usize, count: usize) -> Vec<u8> {
        (from..from + count)
            .flat_map(|n| format!("line {n:07}\n").into_bytes())
            .collect()
    }

    /// The lines of a trimmed log after its notice: whole, consecutive, and
    /// ending with `last`. Returns the first kept line's number.
    fn assert_newest_lines(text: &str, last: usize) -> usize {
        let mut lines = text.lines();
        let notice = lines.next().unwrap();
        assert!(
            notice.starts_with("[nfs-afterhours: ") && notice.ends_with(" MiB]"),
            "{notice}"
        );
        let numbers: Vec<usize> = lines
            .map(|line| {
                line.strip_prefix("line ")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or_else(|| panic!("not a whole line: {line:?}"))
            })
            .collect();
        assert_eq!(numbers.last(), Some(&last), "the newest line is kept");
        assert!(numbers.windows(2).all(|w| w[1] == w[0] + 1), "consecutive");
        numbers[0]
    }

    #[test]
    fn a_full_log_keeps_its_newest_lines_behind_one_notice() {
        let dir = crate::testing::scratch("trim");
        let path = dir.join("server.stderr.log");
        let mut log = open_log(&path).unwrap();
        let mut trimmed = Trimmed::default();
        // 5 MiB of lines, then a trim.
        let first = 5 * 1024 * 1024 / 13;
        log.write_all(&numbered(0, first)).unwrap();
        let len = trim_to_newest(&mut log, &mut trimmed).unwrap();
        assert_eq!(len as u64, std::fs::metadata(&path).unwrap().len());
        assert!(len <= RETAINED_LOG_BYTES + 128);
        let text = std::fs::read_to_string(&path).unwrap();
        let kept_from = assert_newest_lines(&text, first - 1);
        assert_eq!(trimmed.removed, kept_from as u64 * 13, "counts output only");
        assert!(text.starts_with(&format!(
            "[nfs-afterhours: {} earlier bytes removed",
            trimmed.removed
        )));
        // More output and a second trim: still one notice, with the total.
        log.write_all(&numbered(first, 100_000)).unwrap();
        trim_to_newest(&mut log, &mut trimmed).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let kept_from = assert_newest_lines(&text, first + 100_000 - 1);
        assert_eq!(trimmed.removed, kept_from as u64 * 13);
        assert_eq!(text.matches("[nfs-afterhours: ").count(), 1);
        drop(log);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn logs_keep_the_newest_output_and_results_keep_their_start() {
        let dir = crate::testing::scratch("child");
        // About 9 MiB of lines on both streams: two trims or more.
        let total = 9 * 1024 * 1024 / 13;
        let big = dir.join("lines.txt");
        std::fs::write(&big, numbered(0, total)).unwrap();
        let script = if cfg!(windows) {
            format!(
                "@echo off\r\ntype \"{0}\"\r\ntype \"{0}\" 1>&2\r\nexit 0\r\n",
                big.display()
            )
        } else {
            format!("cat '{0}'\ncat '{0}' >&2\nexit 0\n", big.display())
        };
        let mut child = StdChild::spawn(&spec_keeping(&dir, "long", &script, Keep::End)).unwrap();
        wait_exit(&mut child);
        assert_eq!(child.exit_code(), Some(0));
        child.check_logs().unwrap();
        assert!(!child.output_truncated(), "nothing new was discarded");
        for name in ["long.stdout.log", "long.stderr.log"] {
            let path = dir.join(name);
            assert!(std::fs::metadata(&path).unwrap().len() as usize <= MAX_LOG_BYTES);
            let text = std::fs::read_to_string(&path).unwrap();
            assert_newest_lines(&text, total - 1);
        }
        // The same output kept from its start stops at the bound instead.
        let mut child =
            StdChild::spawn(&spec_keeping(&dir, "result", &script, Keep::Start)).unwrap();
        wait_exit(&mut child);
        assert!(child.output_truncated());
        let text = std::fs::read_to_string(dir.join("result.stdout.log")).unwrap();
        assert_eq!(text.len(), MAX_LOG_BYTES);
        assert!(text.starts_with("line 0000000\n"));
        std::fs::remove_dir_all(dir).unwrap();
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
    fn map_frames_after_readiness_stay_in_memory_and_only_the_newest_is_kept() {
        let dir = crate::testing::scratch("child");
        let script = if cfg!(windows) {
            "@echo off\r\necho {\"ready\":true}\r\necho {\"frame\":1}\r\necho {\"frame\":2}\r\necho {\"frame\":3}\r\necho diag 1>&2\r\nexit 0\r\n"
        } else {
            "printf '{\"ready\":true}\\n{\"frame\":1}\\n{\"frame\":2}\\n'\nprintf '{\"frame\":3}\\n'\necho diag >&2\nexit 0\n"
        };
        let mut feed = spec_keeping(&dir, "feed", script, Keep::End);
        feed.map_feed = true;
        let mut child = StdChild::spawn(&feed).unwrap();
        wait_exit(&mut child);
        child.check_logs().unwrap();
        assert_eq!(child.first_line().unwrap().unwrap(), "{\"ready\":true}");
        assert_eq!(child.take_map_feed().as_deref(), Some("{\"frame\":3}"));
        assert_eq!(child.take_map_feed(), None, "each line is taken once");
        assert!(child.drain_diagnostics().contains("diag"));
        let log = std::fs::read_to_string(dir.join("feed.stdout.log")).unwrap();
        assert_eq!(log.trim_end(), "{\"ready\":true}", "frames are not logged");
        // Without a map feed the same output is logged and nothing is kept.
        let mut child = StdChild::spawn(&spec_keeping(&dir, "plain", script, Keep::End)).unwrap();
        wait_exit(&mut child);
        assert_eq!(child.take_map_feed(), None);
        let log = std::fs::read_to_string(dir.join("plain.stdout.log")).unwrap();
        assert_eq!(log.lines().count(), 4);
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
