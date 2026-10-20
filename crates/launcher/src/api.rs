// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The session API: `setup`, `serve` and `start` with `--api`.
//!
//! The supervisor serves its session over HTTP on this computer
//! (`127.0.0.1`, a port the system picks), so a desktop front end can
//! follow and control it the way it would a server on another computer,
//! and can go away and come back without affecting the session:
//!
//! | Request | Answer |
//! | --- | --- |
//! | `GET /v1/info` | the `version` record |
//! | `GET /v1/status` | the `status` record |
//! | `GET /v1/events` | the session's `event` records as server-sent events |
//! | `POST /v1/stop` | the `stop` record: close the game, then the server |
//! | `POST /v1/play` | the `play` record: start the game in a `serve` session |
//! | `GET /v1/settings` | the `settings` record: the saved traffic and rivals |
//! | `PUT /v1/settings` | the `settings` record after saving a [`SettingsChange`] |
//! | `GET /v1/profile` | the `profile` record of the selected save |
//!
//! Answers are the records the matching commands print with `--json`, with
//! the same schemas; a refused request answers an `api-error` record. The
//! address and a random token are written to `api.json` in the session
//! directory (the `api` record, printed by `nfs-afterhours api --json`).
//! Every request must carry `Authorization: Bearer <token>` and a `Host` of
//! that address; requests from web pages (an `Origin` header) are refused.
//!
//! The event stream starts with the newest `status` and `map-feed` events,
//! then sends every later event in order, with a keep-alive comment when
//! idle; it ends after the session's last status. A reader that falls
//! behind loses events (the session's log files keep everything) and never
//! holds up the session. One request per connection; only `PUT
//! /v1/settings` has a body, a JSON object of at most [`MAX_BODY`] bytes.

pub mod http;
pub mod hub;
#[cfg(test)]
mod tests;

use crate::{
    contract::{PlayReport, SettingsReport, StatusReport, StopReport, VersionReport},
    exit::{Exit, Failure, Result},
    profile::Profile,
    settings::{self, Rivals},
};
use hub::{Hub, Next};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    io::Write,
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Version of [`ApiRecord`] and [`ApiError`].
pub const FORMAT_VERSION: u32 = 1;
/// The file in the session directory that holds the [`ApiRecord`].
pub const FILE: &str = "api.json";
/// Most connections served at once; more are answered 503.
pub const MAX_CONNECTIONS: usize = 16;
/// Most event streams open at once; more are answered 503.
pub const MAX_STREAMS: usize = 8;
/// How long a peer may take to send its request or accept a write.
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// How often an idle event stream gets a keep-alive comment.
pub const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// How long streams get to deliver the last events when the session ends.
pub const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// Longest request body: only `PUT /v1/settings` has one.
pub const MAX_BODY: usize = 1024;

/// How to reach a session's API: `api.json` and `nfs-afterhours api --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApiRecord {
    pub version: u32,
    /// The session this API serves.
    pub id: String,
    /// Base address, `http://127.0.0.1:<port>`; requests go to `<url>/v1/...`.
    pub url: String,
    /// Send as `Authorization: Bearer <token>` with every request. 64
    /// lowercase hexadecimal characters, new for every session.
    pub token: String,
}

/// The body of every refused API request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ApiError {
    pub version: u32,
    /// What was refused and why, for a person to read.
    pub message: String,
}

/// The body of `PUT /v1/settings`: the `settings` record's fields to save,
/// within the bounds that record reports. A field left out keeps its saved
/// value; `"rival_limit": null` restores the original number of rivals.
/// At least one field is required; any other field is refused.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsChange {
    /// Ambient traffic as a percentage of the original amount.
    #[serde(default, deserialize_with = "number")]
    pub traffic_percent: Option<u16>,
    /// Roaming rivals kept active; `Some(None)` is `null`, the original.
    #[serde(default, deserialize_with = "number_or_null")]
    pub rival_limit: Option<Option<u16>>,
}

/// A field that is present must be a number; `null` is refused.
fn number<'de, D: Deserializer<'de>>(input: D) -> std::result::Result<Option<u16>, D::Error> {
    u16::deserialize(input).map(Some)
}

/// A field that is present is a number or `null`.
fn number_or_null<'de, D: Deserializer<'de>>(
    input: D,
) -> std::result::Result<Option<Option<u16>>, D::Error> {
    Option::<u16>::deserialize(input).map(Some)
}

impl SettingsChange {
    /// Parse a request body into the `--traffic` and `--rivals` values it
    /// stands for, checked as the command line checks them.
    pub fn parse(body: &[u8]) -> std::result::Result<(Option<u16>, Option<Rivals>), String> {
        // Serde would also read a struct from an array of its fields.
        if body.trim_ascii_start().first() != Some(&b'{') {
            return Err("the body must be a JSON object.".into());
        }
        let change: Self = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        if change == Self::default() {
            return Err("send traffic_percent, rival_limit or both.".into());
        }
        if change
            .traffic_percent
            .is_some_and(|percent| percent > settings::MAX_TRAFFIC_PERCENT)
        {
            return Err(format!(
                "traffic_percent must be 0 to {}.",
                settings::MAX_TRAFFIC_PERCENT
            ));
        }
        let rivals = match change.rival_limit {
            None => None,
            Some(None) => Some(Rivals::Original),
            Some(Some(count)) if count <= settings::MAX_RIVALS => Some(Rivals::Count(count)),
            Some(Some(_)) => {
                return Err(format!(
                    "rival_limit must be null or 0 to {}.",
                    settings::MAX_RIVALS
                ));
            }
        };
        Ok((change.traffic_percent, rivals))
    }
}

/// What the API asks of the session; the live implementation runs the
/// same code as the matching commands.
pub trait Control: Send + Sync + 'static {
    /// `version --json`.
    fn info(&self) -> VersionReport;
    /// `status --json`.
    fn status(&self) -> Result<StatusReport>;
    /// `stop --json`.
    fn stop(&self) -> Result<StopReport>;
    /// `play --json`.
    fn play(&self) -> Result<PlayReport>;
    /// `settings --json`.
    fn settings(&self) -> Result<SettingsReport>;
    /// `settings --json` with `--traffic` and `--rivals`, given as checked
    /// values: save them, then report.
    fn save_settings(&self, traffic: Option<u16>, rivals: Option<Rivals>)
    -> Result<SettingsReport>;
    /// `profile --json`: the selected save.
    fn profile(&self) -> Result<Profile>;
}

/// A new token: 32 bytes from the system's random source, as hex.
pub fn new_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    nfs_afterhours_sys::random_bytes(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Shared by the accept loop and the connection threads.
struct Shared {
    control: Arc<dyn Control>,
    hub: Arc<Hub>,
    token: String,
    port: u16,
    shutdown: AtomicBool,
    connections: AtomicUsize,
    streams: AtomicUsize,
}

/// A running API listener.
pub struct ApiServer {
    shared: Arc<Shared>,
    address: SocketAddr,
    accept: Option<JoinHandle<()>>,
}

impl ApiServer {
    /// Listen on `127.0.0.1` at a port the system picks.
    pub fn start(control: Arc<dyn Control>, hub: Arc<Hub>, token: String) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            control,
            hub,
            token,
            port: address.port(),
            shutdown: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
            streams: AtomicUsize::new(0),
        });
        let accept = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("api-accept".into())
                .spawn(move || accept_loop(&listener, &shared))?
        };
        Ok(Self {
            shared,
            address,
            accept: Some(accept),
        })
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Event streams still open.
    pub fn open_streams(&self) -> usize {
        self.shared.streams.load(Ordering::SeqCst)
    }

    /// End the session's streams (after their last events, for up to
    /// `grace`) and stop accepting connections.
    pub fn finish(mut self, grace: Duration) {
        self.shared.hub.close();
        let deadline = Instant::now() + grace;
        while self.open_streams() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.stop_accepting();
    }

    fn stop_accepting(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        // Wake the blocking accept so it sees the flag.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        if self.accept.is_some() {
            self.shared.hub.close();
            self.stop_accepting();
        }
    }
}

/// Decrements a counter when dropped, also on a panic.
struct Count<'a>(&'a AtomicUsize);

impl Drop for Count<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Take one slot of `counter` if fewer than `most` are taken.
fn take_slot(counter: &AtomicUsize, most: usize) -> Option<Count<'_>> {
    counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            (n < most).then_some(n + 1)
        })
        .ok()
        .map(|_| Count(counter))
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }
        let Ok(mut stream) = stream else {
            // A failed accept (for example, too many open files) must not spin.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        if shared.connections.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            let _ = refuse(&mut stream, 503, &[], "The API is busy; try again.");
            continue;
        }
        shared.connections.fetch_add(1, Ordering::SeqCst);
        let worker = Arc::clone(shared);
        let spawned = std::thread::Builder::new()
            .name("api-connection".into())
            .spawn(move || {
                let _slot = Count(&worker.connections);
                serve(stream, &worker);
            });
        if spawned.is_err() {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn error_body(message: &str) -> Vec<u8> {
    serde_json::to_vec(&ApiError {
        version: FORMAT_VERSION,
        message: message.to_owned(),
    })
    .unwrap_or_default()
}

fn refuse(
    stream: &mut impl Write,
    status: u16,
    extra: &[(&str, &str)],
    message: &str,
) -> std::io::Result<()> {
    http::write_json(stream, status, extra, &error_body(message))
}

/// The HTTP status for a refused command.
fn status_of(failure: &Failure) -> u16 {
    match failure.exit {
        Exit::Refused | Exit::NoSession => 409,
        _ => 500,
    }
}

fn answer<T: Serialize>(stream: &mut impl Write, result: Result<T>) -> std::io::Result<()> {
    match result.and_then(|value| Ok(serde_json::to_vec(&value)?)) {
        Ok(body) => http::write_json(stream, 200, &[], &body),
        Err(failure) => refuse(stream, status_of(&failure), &[], &failure.message),
    }
}

/// Why a request is refused before routing, if it is.
fn check(head: &http::Head, shared: &Shared) -> Option<(u16, &'static str)> {
    let host = head.header("host").unwrap_or_default();
    let port = shared.port;
    let allowed = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !allowed.iter().any(|a| a.as_bytes() == host) {
        return Some((403, "This API answers only on its own loopback address."));
    }
    if head.header("origin").is_some() {
        return Some((403, "Requests from web pages are refused."));
    }
    let token = head
        .header("authorization")
        .and_then(|v| v.strip_prefix(b"Bearer "))
        .unwrap_or_default();
    if !http::same_secret(token, shared.token.as_bytes()) {
        return Some((401, "Send the session's token as Authorization: Bearer."));
    }
    None
}

/// The length of the request's body, or why it is refused: only `PUT
/// /v1/settings` has one, sent with a `Content-Length` of at most
/// [`MAX_BODY`].
fn body_length(head: &http::Head) -> std::result::Result<usize, (u16, &'static str)> {
    let takes_body = (head.method.as_str(), head.path.as_str()) == ("PUT", "/v1/settings");
    let no_length = (411, "Send the request body with a Content-Length.");
    let no_body = (413, "Only PUT /v1/settings has a request body.");
    if head.header("transfer-encoding").is_some() {
        return Err(if takes_body { no_length } else { no_body });
    }
    let Ok(length) = http::content_length(head) else {
        return Err((400, "The Content-Length is not one whole number."));
    };
    match (takes_body, length) {
        (false, Some(1..)) => Err(no_body),
        (false, _) => Ok(0),
        (true, None) => Err(no_length),
        (true, Some(n)) if n > MAX_BODY => Err((413, "The request body is over 1 KiB.")),
        (true, Some(n)) => Ok(n),
    }
}

/// Before refusing a request, read the body its head announced when it is
/// no larger than [`MAX_BODY`], so the peer gets the refusal: a connection
/// closed with bytes unread is reset, and the answer can be lost with it.
fn discard_body(stream: &mut TcpStream, head: &http::Head) {
    if let Ok(Some(length @ 1..=MAX_BODY)) = http::content_length(head) {
        let _ = http::read_body(stream, length);
    }
}

/// `PUT /v1/settings`: read and check the body, then save. The body is
/// read before any refusal, so the peer gets the answer rather than a
/// connection reset over unread bytes.
fn save_settings(
    stream: &mut TcpStream,
    head: &http::Head,
    length: usize,
    control: &dyn Control,
) -> std::io::Result<()> {
    let body = match http::read_body(stream, length) {
        Ok(body) => body,
        Err(http::ReadError::Closed) => {
            return refuse(
                stream,
                400,
                &[],
                "The request body is shorter than its Content-Length.",
            );
        }
        Err(_) => return Ok(()),
    };
    if !http::is_json(head) {
        return refuse(
            stream,
            415,
            &[],
            "Send the settings as Content-Type: application/json.",
        );
    }
    match SettingsChange::parse(&body) {
        Ok((traffic, rivals)) => answer(stream, control.save_settings(traffic, rivals)),
        Err(why) => refuse(stream, 400, &[], &format!("Invalid settings: {why}")),
    }
}

fn serve(mut stream: TcpStream, shared: &Shared) {
    let head = match http::read_head(&mut stream) {
        Ok(head) => head,
        Err(http::ReadError::TooLarge) => {
            let _ = refuse(&mut stream, 413, &[], "The request head is too large.");
            return;
        }
        Err(http::ReadError::Malformed) => {
            let _ = refuse(&mut stream, 400, &[], "Not an HTTP/1.1 request.");
            return;
        }
        Err(http::ReadError::Closed | http::ReadError::Io(_)) => return,
    };
    if let Some((status, message)) = check(&head, shared) {
        let extra: &[(&str, &str)] = if status == 401 {
            &[("WWW-Authenticate", "Bearer")]
        } else {
            &[]
        };
        discard_body(&mut stream, &head);
        let _ = refuse(&mut stream, status, extra, message);
        return;
    }
    let length = match body_length(&head) {
        Ok(length) => length,
        Err((status, message)) => {
            discard_body(&mut stream, &head);
            let _ = refuse(&mut stream, status, &[], message);
            return;
        }
    };
    let control = &shared.control;
    let _ = match (head.method.as_str(), head.path.as_str()) {
        ("GET", "/v1/info") => answer(&mut stream, Ok(control.info())),
        ("GET", "/v1/status") => answer(&mut stream, control.status()),
        ("GET", "/v1/events") => stream_events(&mut stream, shared),
        ("POST", "/v1/stop") => answer(&mut stream, control.stop()),
        ("POST", "/v1/play") => answer(&mut stream, control.play()),
        ("GET", "/v1/settings") => answer(&mut stream, control.settings()),
        ("PUT", "/v1/settings") => save_settings(&mut stream, &head, length, control.as_ref()),
        ("GET", "/v1/profile") => answer(&mut stream, control.profile()),
        (_, "/v1/info" | "/v1/status" | "/v1/events" | "/v1/profile") => refuse(
            &mut stream,
            405,
            &[("Allow", "GET")],
            "This address answers GET.",
        ),
        (_, "/v1/settings") => refuse(
            &mut stream,
            405,
            &[("Allow", "GET, PUT")],
            "This address answers GET and PUT.",
        ),
        (_, "/v1/stop" | "/v1/play") => refuse(
            &mut stream,
            405,
            &[("Allow", "POST")],
            "This address answers POST.",
        ),
        _ => refuse(&mut stream, 404, &[], "No such API address."),
    };
}

fn stream_events(stream: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
    let Some(_slot) = take_slot(&shared.streams, MAX_STREAMS) else {
        return refuse(stream, 503, &[], "Too many event streams are open.");
    };
    http::write_stream_head(stream)?;
    let (snapshot, mut cursor) = shared.hub.subscribe();
    for event in &snapshot {
        http::write_event(stream, event.sequence, event.kind, &event.json)?;
    }
    stream.flush()?;
    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            return Ok(());
        }
        match shared.hub.wait(cursor, KEEP_ALIVE) {
            Next::Events(events, next) => {
                for event in &events {
                    http::write_event(stream, event.sequence, event.kind, &event.json)?;
                }
                stream.flush()?;
                cursor = next;
            }
            Next::Idle => http::write_keep_alive(stream)?,
            Next::Closed => return Ok(()),
        }
    }
}

/// The API of the supervisor's own session: the commands' code over its
/// data directory.
pub struct LiveControl {
    data: PathBuf,
    build: crate::contract::Build,
    /// One settings save at a time: each reads, changes and replaces the
    /// file, and the temporary file is named after this process.
    saving: Mutex<()>,
}

impl LiveControl {
    pub fn new(data: PathBuf, build: crate::contract::Build) -> Self {
        Self {
            data,
            build,
            saving: Mutex::new(()),
        }
    }
}

impl Control for LiveControl {
    fn info(&self) -> VersionReport {
        VersionReport::new(&self.build)
    }
    fn status(&self) -> Result<StatusReport> {
        crate::commands::status::status_report(&self.data)
    }
    fn stop(&self) -> Result<StopReport> {
        crate::commands::status::stop_report(&self.data)
    }
    fn play(&self) -> Result<PlayReport> {
        crate::commands::status::play_report(&self.data)
    }
    fn settings(&self) -> Result<SettingsReport> {
        crate::commands::settings::settings_report(&self.data, None, None)
    }
    fn save_settings(
        &self,
        traffic: Option<u16>,
        rivals: Option<Rivals>,
    ) -> Result<SettingsReport> {
        let _one = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        crate::commands::settings::settings_report(&self.data, traffic, rivals)
    }
    fn profile(&self) -> Result<Profile> {
        crate::commands::profile::profile_report(&self.data, &crate::cli::ProfileArgs::default())
    }
}

/// A session's API while it runs: the listener and its `api.json`.
pub struct SessionApi {
    server: ApiServer,
    file: PathBuf,
}

impl SessionApi {
    /// Listen, and write how to reach the API to the session directory.
    pub fn start(control: Arc<dyn Control>, hub: Arc<Hub>, id: &str, run: &Path) -> Result<Self> {
        let token = new_token()?;
        let server = ApiServer::start(control, hub, token.clone())?;
        let record = ApiRecord {
            version: FORMAT_VERSION,
            id: id.to_owned(),
            url: server.url(),
            token,
        };
        let file = run.join(FILE);
        write_private(&file, &record)?;
        Ok(Self { server, file })
    }

    /// The session is over: deliver the last events, close, forget the token.
    pub fn finish(self) {
        self.server.finish(DRAIN_GRACE);
        let _ = std::fs::remove_file(&self.file);
    }
}

/// Write a new record readable only by this user where the system allows
/// it; the session directory already lies under the user's data directory.
fn write_private(path: &Path, record: &ApiRecord) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer_pretty(&mut file, record)?;
    file.write_all(b"\n")?;
    Ok(())
}

/// `api`: how to reach the current session's API, if it is live and has one.
pub fn current(data: &Path) -> Result<ApiRecord> {
    let none = || {
        Failure::no_session("No running Afterhours session serves the API. Start one with --api.")
    };
    let status = match crate::session::current_status(data) {
        Ok(status) => status.ok_or_else(none)?,
        Err(_) if without_status(data) => return Err(none()),
        Err(failure) => return Err(failure),
    };
    if !status.state.is_live() {
        return Err(none());
    }
    let run = crate::paths::session_dir(data, &status.id);
    let path = run.join(FILE);
    if !path.exists() {
        return Err(none());
    }
    let record: ApiRecord = crate::records::read_json(&path)?;
    if record.id != status.id {
        return Err(Failure::refused("Session API identity mismatch."));
    }
    Ok(record)
}

/// Whether the current session has no status yet. A supervisor points at
/// its session, then starts the API, then writes the first status; a
/// front end polling `api` in between sees no API yet, not an error.
fn without_status(data: &Path) -> bool {
    let pointer = data.join(crate::session::POINTER_FILE);
    crate::records::read_json::<crate::records::SessionPointer>(&pointer).is_ok_and(|pointer| {
        crate::records::is_id(&pointer.id)
            && !crate::paths::session_dir(data, &pointer.id)
                .join("status.json")
                .exists()
    })
}
