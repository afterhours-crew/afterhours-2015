// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only summaries of the session folders in a data directory, for a
//! desktop logs view: `artifacts/launcher/<id>/`, newest first.
//!
//! For each session this reports the action from `request.json`, the
//! latest state and message from `status.json` (a live state whose
//! supervisor is gone is reported as `interrupted`, as `status` does), each
//! file with its size and line count, the server's log levels and its
//! `unsupported operation received` warnings grouped by service and route,
//! whether the server's recording folder `artifacts/runs/<id>/` exists, and
//! the game's crash dumps written during the session.
//!
//! Nothing is written or locked, and every read is bounded: the number of
//! folders, files and dumps examined, the bytes scanned per file and per
//! call, the length of a parsed log line and the number of warning groups.
//! A file or record that cannot be read becomes a warning on its session;
//! only an unusable data directory fails the command.

use crate::{
    clock,
    exit::Result,
    paths,
    platform::child::MAX_LOG_BYTES,
    records::{self, Action, Request, SessionPointer, State, Status},
    session,
};
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// Version of [`SessionList`].
pub const FORMAT_VERSION: u32 = 1;
/// Sessions listed when no limit is given.
pub const DEFAULT_LIMIT: usize = 20;
/// Largest accepted limit.
pub const MAX_LIMIT: usize = 200;
/// Entries of `artifacts/launcher` examined.
pub const MAX_SESSION_ENTRIES: usize = 4096;
/// Entries of one session folder examined.
pub const MAX_FILE_ENTRIES: usize = 256;
/// Files listed per session.
pub const MAX_FILES: usize = 64;
/// Larger files are listed without a line count. The log pump keeps at
/// most 4 MiB per file, so every log a session writes is scanned.
pub const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024;
/// Bytes scanned per call across all files; later files get no line count.
pub const MAX_TOTAL_SCAN_BYTES: u64 = 256 * 1024 * 1024;
/// Server log bytes parsed per line; longer lines count as `other`.
pub const MAX_PARSED_LINE: usize = 16 * 1024;
/// Distinct (service, route) warning groups kept per log.
pub const MAX_UNSUPPORTED_GROUPS: usize = 256;
/// Characters kept of a service or route name.
pub const MAX_FIELD_CHARS: usize = 200;
/// Entries of the crash dump folder examined.
pub const MAX_DUMP_ENTRIES: usize = 4096;
/// Crash dumps listed per session.
pub const MAX_DUMPS: usize = 32;
/// Dumps written this long after a session's last status still belong to it.
pub const DUMP_GRACE: Duration = Duration::from_secs(120);

const SERVER_LOG: &str = "server.stderr.log";
const UNSUPPORTED: &str = "unsupported operation received";
const CHUNK: usize = 64 * 1024;

/// What to list and where the optional platform folders are.
#[derive(Debug, Clone)]
pub struct Options {
    /// Sessions listed, newest first; clamped to `1..=MAX_LIMIT`.
    pub limit: usize,
    /// Only this session.
    pub id: Option<String>,
    /// The game's `Documents\Need For Speed\Temp` folder, if known.
    pub crash_dump_directory: Option<PathBuf>,
    /// The EA Desktop log folder, if known.
    pub ea_desktop_logs: Option<PathBuf>,
    /// Bytes scanned per call; [`MAX_TOTAL_SCAN_BYTES`] outside tests.
    pub scan_budget: u64,
    /// The current time, which ends a live session's crash dump window.
    pub now: SystemTime,
}

impl Options {
    pub fn new(limit: usize, id: Option<String>) -> Self {
        Self {
            limit,
            id,
            crash_dump_directory: None,
            ea_desktop_logs: None,
            scan_budget: MAX_TOTAL_SCAN_BYTES,
            now: SystemTime::now(),
        }
    }
}

/// `sessions --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SessionList {
    pub version: u32,
    pub data_directory: PathBuf,
    /// The session `current-session.json` names, if any.
    pub current: Option<String>,
    /// Newest first, by the time the session was requested.
    pub sessions: Vec<SessionSummary>,
    /// More sessions exist than are listed.
    pub truncated: bool,
    /// The folder searched for game crash dumps; `null` where the game
    /// does not run or the folder is unknown.
    pub crash_dump_directory: Option<Location>,
    /// The EA Desktop log folder; `null` where it does not apply.
    pub ea_desktop_logs: Option<Location>,
    pub warnings: Vec<String>,
}

/// A folder outside the data directory and whether it exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Location {
    pub path: PathBuf,
    pub exists: bool,
}

/// One session folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SessionSummary {
    /// Session ID: 32 lowercase hexadecimal characters.
    pub id: String,
    /// Whether `current-session.json` names this session.
    pub current: bool,
    pub directory: PathBuf,
    /// From `request.json`; `null` when it is missing or unreadable.
    pub action: Option<Action>,
    /// When the session was requested (the modification time of
    /// `request.json`), UTC.
    pub started_utc: Option<String>,
    /// When the latest status was recorded, as `status.json` says.
    pub updated_utc: Option<String>,
    /// When the session finished or failed; `null` while it runs or when
    /// it was interrupted.
    pub ended_utc: Option<String>,
    /// `null` when `status.json` is missing or unreadable, for example
    /// just before a new supervisor records its first state.
    pub state: Option<State>,
    pub message: Option<String>,
    /// Ordinary files in the session folder, by name.
    pub files: Vec<SessionFile>,
    /// More files exist than are listed.
    pub files_truncated: bool,
    /// Summary of `server.stderr.log`; `null` when there is none or it
    /// was not scanned (see `warnings`).
    pub server_log: Option<ServerLog>,
    /// The server's recording folder, `artifacts/runs/<id>/`.
    pub runs_directory: PathBuf,
    pub runs_directory_exists: bool,
    /// Game crash dumps written between the session's request and its
    /// last status (or now, while it runs) plus two minutes. `null` when
    /// the crash dump folder is unknown or the session has no start time.
    pub crash_dumps: Option<Vec<CrashDump>>,
    pub warnings: Vec<String>,
}

/// One file in a session folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SessionFile {
    pub name: String,
    pub bytes: u64,
    /// Newline-separated lines, counting an unterminated last line; `null`
    /// when the file was too large or the scan budget was spent.
    pub lines: Option<u64>,
    /// The file reached the 4 MiB log bound; output after it was discarded.
    pub truncated: bool,
}

/// What `server.stderr.log` contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ServerLog {
    /// All lines, including an unterminated last one.
    pub lines: u64,
    /// The file ends without a newline (still being written, or cut at the
    /// log bound). That line is counted but not parsed.
    pub partial_last_line: bool,
    /// Complete lines by tracing level.
    pub levels: Levels,
    /// Every `unsupported operation received` line.
    pub unsupported_total: u64,
    /// Those lines grouped by service and route, most frequent first.
    pub unsupported: Vec<UnsupportedRoute>,
    /// More distinct groups existed than are listed; their lines are
    /// still in `unsupported_total`.
    pub unsupported_truncated: bool,
}

/// Complete lines by level. `other` counts continuation lines, panics,
/// overlong lines and anything without a timestamp and level.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Levels {
    pub error: u64,
    pub warn: u64,
    pub info: u64,
    pub debug: u64,
    pub trace: u64,
    pub other: u64,
}

/// One group of `unsupported operation received` warnings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct UnsupportedRoute {
    /// The `service` field, such as `lsx` or `blaze`.
    pub service: Option<String>,
    /// The `route` field as the server printed it, without `Some(...)`,
    /// quotes and a Blaze route's per-request `correlation`; `null` when
    /// absent or `None`.
    pub route: Option<String>,
    /// Warning lines in this group. The server rate-limits repeats, so
    /// this counts warnings, not requests.
    pub warnings: u64,
}

/// A game crash dump.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CrashDump {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub modified_utc: String,
}

/// List the sessions in `data`.
pub fn list(data: &Path, options: &Options) -> Result<SessionList> {
    let data = paths::checked_data_path(data, None)?;
    let mut warnings = Vec::new();
    let current = current_pointer(&data, &mut warnings);
    let root = data.join("artifacts").join("launcher");
    paths::checked_data_path(&root, None)?;
    let (mut found, mut truncated) = discover(&root, &mut warnings);
    if let Some(id) = &options.id {
        found.retain(|candidate| &candidate.id == id);
    }
    found.sort_by(|a, b| b.sort_key().cmp(&a.sort_key()).then(a.id.cmp(&b.id)));
    let limit = options.limit.clamp(1, MAX_LIMIT);
    if found.len() > limit {
        found.truncate(limit);
        truncated = true;
    }
    let dumps = options
        .crash_dump_directory
        .as_deref()
        .map(|dir| crash_dumps(dir, &mut warnings));
    let mut context = Context {
        data: &data,
        current: current.as_deref(),
        lock: None,
        dumps: dumps.as_deref(),
        now: options.now,
        budget: options.scan_budget,
        budget_spent: false,
    };
    let mut sessions = Vec::with_capacity(found.len());
    for candidate in found {
        sessions.push(context.summarize(candidate)?);
    }
    if context.budget_spent {
        warnings.push(format!(
            "Line counting stopped after {} MiB in this call; list fewer sessions for the rest.",
            options.scan_budget / (1024 * 1024)
        ));
    }
    Ok(SessionList {
        version: FORMAT_VERSION,
        data_directory: data,
        current,
        sessions,
        truncated,
        crash_dump_directory: options.crash_dump_directory.as_deref().map(location),
        ea_desktop_logs: options.ea_desktop_logs.as_deref().map(location),
        warnings,
    })
}

fn location(path: &Path) -> Location {
    Location {
        path: path.to_owned(),
        exists: path.is_dir(),
    }
}

/// Metadata of an ordinary file, never following a link.
fn regular_file(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|meta| !paths::is_link(meta) && meta.is_file())
}

fn current_pointer(data: &Path, warnings: &mut Vec<String>) -> Option<String> {
    let path = data.join(session::POINTER_FILE);
    regular_file(&path)?;
    match records::read_json::<SessionPointer>(&path) {
        Ok(pointer) if pointer.version == records::VERSION && records::is_id(&pointer.id) => {
            Some(pointer.id)
        }
        Ok(_) => {
            warnings.push("current-session.json is not a valid session pointer.".into());
            None
        }
        Err(error) => {
            warnings.push(error.message);
            None
        }
    }
}

struct Candidate {
    id: String,
    requested: Option<SystemTime>,
    modified: Option<SystemTime>,
}

impl Candidate {
    fn sort_key(&self) -> SystemTime {
        self.requested
            .or(self.modified)
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }
}

fn discover(root: &Path, warnings: &mut Vec<String>) -> (Vec<Candidate>, bool) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), false);
        }
        Err(error) => {
            warnings.push(format!("The session folders could not be read: {error}"));
            return (Vec::new(), false);
        }
    };
    let mut found = Vec::new();
    for (examined, entry) in entries.enumerate() {
        if examined == MAX_SESSION_ENTRIES {
            warnings.push(format!(
                "Only the first {MAX_SESSION_ENTRIES} entries of artifacts/launcher were examined."
            ));
            return (found, true);
        }
        let Ok(entry) = entry else { continue };
        let Ok(id) = entry.file_name().into_string() else {
            continue;
        };
        if !records::is_id(&id) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if paths::is_link(&meta) || !meta.is_dir() {
            continue;
        }
        let requested =
            regular_file(&entry.path().join("request.json")).and_then(|m| m.modified().ok());
        found.push(Candidate {
            id,
            requested,
            modified: meta.modified().ok(),
        });
    }
    (found, false)
}

struct Dump {
    name: String,
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

fn crash_dumps(directory: &Path, warnings: &mut Vec<String>) -> Vec<Dump> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warnings.push(format!("The crash dump folder could not be read: {error}"));
            return Vec::new();
        }
    };
    let mut dumps = Vec::new();
    for (examined, entry) in entries.enumerate() {
        if examined == MAX_DUMP_ENTRIES {
            warnings.push(format!(
                "Only the first {MAX_DUMP_ENTRIES} entries of the crash dump folder were examined."
            ));
            break;
        }
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let is_dump = name.starts_with("CrashDump_")
            && name.len() > 4
            && name
                .get(name.len() - 4..)
                .is_some_and(|ext| ext.eq_ignore_ascii_case(".dmp"));
        if !is_dump {
            continue;
        }
        let Some(meta) = regular_file(&entry.path()) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        dumps.push(Dump {
            name,
            path: entry.path(),
            bytes: meta.len(),
            modified,
        });
    }
    dumps.sort_by(|a, b| a.modified.cmp(&b.modified).then(a.name.cmp(&b.name)));
    dumps
}

struct Context<'a> {
    data: &'a Path,
    current: Option<&'a str>,
    /// `session::lock_is_held`, read once and only when needed.
    lock: Option<Option<bool>>,
    dumps: Option<&'a [Dump]>,
    now: SystemTime,
    budget: u64,
    budget_spent: bool,
}

impl Context<'_> {
    /// Reserve `bytes` of the scan budget.
    fn take(&mut self, bytes: u64) -> bool {
        if bytes <= self.budget {
            self.budget -= bytes;
            true
        } else {
            self.budget_spent = true;
            false
        }
    }

    fn summarize(&mut self, candidate: Candidate) -> Result<SessionSummary> {
        let id = candidate.id;
        let directory = paths::session_dir(self.data, &id);
        let mut warnings = Vec::new();
        let current = self.current == Some(id.as_str());

        let request_path = directory.join("request.json");
        let action = if candidate.requested.is_none() {
            warnings.push("request.json is missing.".into());
            None
        } else {
            match records::read_json::<Request>(&request_path) {
                Ok(request) if request.id == id => Some(request.action),
                Ok(_) => {
                    warnings.push("request.json names another session.".into());
                    None
                }
                Err(error) => {
                    warnings.push(error.message);
                    None
                }
            }
        };

        let status_path = directory.join("status.json");
        let status_meta = regular_file(&status_path);
        let mut status = match status_meta {
            None => None,
            Some(_) => match records::read_json::<Status>(&status_path) {
                Ok(status) if status.id == id => Some(status),
                Ok(_) => {
                    warnings.push("status.json names another session.".into());
                    None
                }
                Err(error) => {
                    warnings.push(error.message);
                    None
                }
            },
        };
        if let Some(status) = status.as_mut()
            && status.state.is_live()
        {
            // Only the current session can have a running supervisor.
            let lock = if current {
                match self.lock {
                    Some(lock) => lock,
                    None => {
                        let lock = session::lock_is_held(self.data)?;
                        self.lock = Some(lock);
                        lock
                    }
                }
            } else {
                Some(false)
            };
            session::derive_interrupted(status, lock);
        }
        let live = status.as_ref().is_some_and(|s| s.state.is_live());

        let (mut files, files_truncated) = list_files(&directory, &mut warnings);
        let server_log = self.server_log(&directory, &mut files, &mut warnings);
        for file in files.iter_mut().filter(|f| f.lines.is_none()) {
            if file.name != SERVER_LOG {
                file.lines = self.count_lines(&directory.join(&file.name), file.bytes);
            }
        }

        let runs_directory = self.data.join("artifacts").join("runs").join(&id);
        let runs_directory_exists = std::fs::symlink_metadata(&runs_directory)
            .is_ok_and(|meta| !paths::is_link(&meta) && meta.is_dir());

        let crash_dumps = match (self.dumps, candidate.requested) {
            (Some(dumps), Some(start)) => {
                let end = if live {
                    self.now
                } else {
                    status_meta
                        .and_then(|meta| meta.modified().ok())
                        .unwrap_or(start)
                };
                let end = end.checked_add(DUMP_GRACE).unwrap_or(end);
                let mut inside: Vec<CrashDump> = dumps
                    .iter()
                    .filter(|dump| dump.modified >= start && dump.modified <= end)
                    .map(|dump| CrashDump {
                        name: dump.name.clone(),
                        path: dump.path.clone(),
                        bytes: dump.bytes,
                        modified_utc: clock::iso_utc(dump.modified),
                    })
                    .collect();
                if inside.len() > MAX_DUMPS {
                    warnings.push(format!(
                        "{} crash dumps fall in this session; the first {MAX_DUMPS} are listed.",
                        inside.len()
                    ));
                    inside.truncate(MAX_DUMPS);
                }
                Some(inside)
            }
            _ => None,
        };

        let ended_utc = status
            .as_ref()
            .filter(|s| matches!(s.state, State::Finished | State::Failed))
            .map(|s| s.utc.clone());
        Ok(SessionSummary {
            current,
            directory,
            action,
            started_utc: candidate.requested.map(clock::iso_utc),
            updated_utc: status.as_ref().map(|s| s.utc.clone()),
            ended_utc,
            state: status.as_ref().map(|s| s.state),
            message: status.map(|s| s.message),
            files,
            files_truncated,
            server_log,
            runs_directory,
            runs_directory_exists,
            crash_dumps,
            warnings,
            id,
        })
    }

    /// Lines in a file of `bytes` bytes, reading at most that many.
    fn count_lines(&mut self, path: &Path, bytes: u64) -> Option<u64> {
        if bytes > MAX_SCAN_BYTES || !self.take(bytes) {
            return None;
        }
        let mut lines = 0u64;
        let mut last = b'\n';
        read_chunks(path, bytes, |chunk| {
            lines += chunk.iter().filter(|&&b| b == b'\n').count() as u64;
            if let Some(&byte) = chunk.last() {
                last = byte;
            }
        })
        .ok()?;
        Some(lines + u64::from(last != b'\n'))
    }

    fn server_log(
        &mut self,
        directory: &Path,
        files: &mut [SessionFile],
        warnings: &mut Vec<String>,
    ) -> Option<ServerLog> {
        let path = directory.join(SERVER_LOG);
        let bytes = regular_file(&path)?.len();
        if bytes > MAX_SCAN_BYTES || !self.take(bytes) {
            warnings.push(format!(
                "{SERVER_LOG} was not scanned: it exceeds the scan bound."
            ));
            return None;
        }
        let mut scanner = LogScanner::default();
        if let Err(error) = read_chunks(&path, bytes, |chunk| scanner.push(chunk)) {
            warnings.push(format!("{SERVER_LOG} could not be read: {error}"));
            return None;
        }
        let log = scanner.finish();
        if let Some(file) = files.iter_mut().find(|f| f.name == SERVER_LOG) {
            file.lines = Some(log.lines);
        }
        Some(log)
    }
}

/// Feed at most `bytes` bytes of a file to `each` in bounded chunks.
fn read_chunks(path: &Path, bytes: u64, mut each: impl FnMut(&[u8])) -> std::io::Result<()> {
    let mut reader = std::fs::File::open(path)?.take(bytes);
    let mut buffer = vec![0u8; CHUNK];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => each(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn list_files(directory: &Path, warnings: &mut Vec<String>) -> (Vec<SessionFile>, bool) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(format!("The session folder could not be read: {error}"));
            return (Vec::new(), false);
        }
    };
    let mut files = Vec::new();
    let mut truncated = false;
    for (examined, entry) in entries.enumerate() {
        if examined == MAX_FILE_ENTRIES {
            truncated = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(meta) = regular_file(&entry.path()) else {
            continue;
        };
        files.push(SessionFile {
            name,
            bytes: meta.len(),
            lines: None,
            truncated: meta.len() >= MAX_LOG_BYTES as u64,
        });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    if files.len() > MAX_FILES {
        files.truncate(MAX_FILES);
        truncated = true;
    }
    (files, truncated)
}

/// Line-by-line reader of the server's tracing output, as written by
/// `tracing_subscriber::fmt` without colors:
/// `2026-10-18T08:19:41.123Z  WARN target: message field=value ...`.
#[derive(Default)]
struct LogScanner {
    line: Vec<u8>,
    overlong: bool,
    lines: u64,
    levels: Levels,
    unsupported_total: u64,
    groups: BTreeMap<(Option<String>, Option<String>), u64>,
    groups_truncated: bool,
}

impl LogScanner {
    fn push(&mut self, mut bytes: &[u8]) {
        while let Some(end) = bytes.iter().position(|&b| b == b'\n') {
            self.append(&bytes[..end]);
            self.end_line();
            bytes = &bytes[end + 1..];
        }
        self.append(bytes);
    }

    fn append(&mut self, part: &[u8]) {
        let room = MAX_PARSED_LINE - self.line.len();
        if part.len() > room {
            self.line.extend_from_slice(&part[..room]);
            self.overlong = true;
        } else {
            self.line.extend_from_slice(part);
        }
    }

    fn end_line(&mut self) {
        self.lines += 1;
        if self.overlong {
            self.levels.other += 1;
        } else {
            let line = std::mem::take(&mut self.line);
            self.parse(&String::from_utf8_lossy(&line));
            self.line = line;
        }
        self.line.clear();
        self.overlong = false;
    }

    fn parse(&mut self, line: &str) {
        let line = line.trim_end_matches('\r');
        let mut tokens = line.split_ascii_whitespace();
        let level = match (tokens.next(), tokens.next()) {
            (Some(time), Some(level)) if time.starts_with(|c: char| c.is_ascii_digit()) => level,
            _ => "",
        };
        let counter = match level {
            "ERROR" => &mut self.levels.error,
            "WARN" => &mut self.levels.warn,
            "INFO" => &mut self.levels.info,
            "DEBUG" => &mut self.levels.debug,
            "TRACE" => &mut self.levels.trace,
            _ => &mut self.levels.other,
        };
        *counter += 1;
        let Some(at) = line.find(UNSUPPORTED) else {
            return;
        };
        let fields = &line[at + UNSUPPORTED.len()..];
        let key = (
            field(fields, "service").and_then(field_value),
            field(fields, "route")
                .and_then(field_value)
                .map(without_correlation),
        );
        self.unsupported_total += 1;
        if let Some(count) = self.groups.get_mut(&key) {
            *count += 1;
        } else if self.groups.len() < MAX_UNSUPPORTED_GROUPS {
            self.groups.insert(key, 1);
        } else {
            self.groups_truncated = true;
        }
    }

    fn finish(mut self) -> ServerLog {
        let partial_last_line = !self.line.is_empty() || self.overlong;
        if partial_last_line {
            self.lines += 1;
        }
        let mut unsupported: Vec<UnsupportedRoute> = self
            .groups
            .into_iter()
            .map(|((service, route), warnings)| UnsupportedRoute {
                service,
                route,
                warnings,
            })
            .collect();
        // Stable: equal counts keep service/route order from the map.
        unsupported.sort_by_key(|group| std::cmp::Reverse(group.warnings));
        ServerLog {
            lines: self.lines,
            partial_last_line,
            levels: self.levels,
            unsupported_total: self.unsupported_total,
            unsupported,
            unsupported_truncated: self.groups_truncated,
        }
    }
}

/// The raw value of `key=value` among tracing fields. Values may be quoted
/// strings or Debug output with nested brackets, such as
/// `Some(Route { component: 4 })`.
fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b' ' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b' ' {
            continue;
        }
        let name = &text[start..i];
        let value_start = i + 1;
        i = value_end(bytes, value_start);
        if name == key {
            return text.get(value_start..i);
        }
    }
    None
}

fn value_end(bytes: &[u8], mut i: usize) -> usize {
    let mut depth = 0usize;
    let mut quoted = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if quoted {
            if byte == b'\\' {
                i += 2;
                continue;
            }
            if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                // `Name { .. }` is one Debug value.
                b' ' if depth == 0 && bytes.get(i + 1) != Some(&b'{') => break,
                _ => {}
            }
        }
        i += 1;
    }
    i.min(bytes.len())
}

/// A Blaze route prints its per-request `correlation: N`; drop it so the
/// repeats of one operation group together.
fn without_correlation(route: String) -> String {
    const KEY: &str = "correlation: ";
    let Some(at) = route.find(KEY) else {
        return route;
    };
    let digits = at + KEY.len();
    let end = route[digits..]
        .find(|c: char| !c.is_ascii_digit())
        .map_or(route.len(), |n| digits + n);
    if end == digits {
        return route;
    }
    let (start, end) = if route[..at].ends_with(", ") {
        (at - 2, end)
    } else if route[end..].starts_with(", ") {
        (at, end + 2)
    } else {
        (at, end)
    };
    format!("{}{}", &route[..start], &route[end..])
}

/// A field value without `Some(...)` and quotes; `None` for `None`.
fn field_value(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let inner = raw
        .strip_prefix("Some(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(raw);
    if inner.is_empty() || inner == "None" {
        return None;
    }
    let text = match inner
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(quoted) => {
            let mut out = String::with_capacity(quoted.len());
            let mut chars = quoted.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some('n') => out.push('\n'),
                        Some('t') => out.push('\t'),
                        Some(other) => out.push(other),
                        None => {}
                    }
                } else {
                    out.push(c);
                }
            }
            out
        }
        None => inner.to_owned(),
    };
    Some(if text.chars().count() > MAX_FIELD_CHARS {
        let mut cut: String = text.chars().take(MAX_FIELD_CHARS).collect();
        cut.push('\u{2026}');
        cut
    } else {
        text
    })
}

#[cfg(test)]
mod tests;
