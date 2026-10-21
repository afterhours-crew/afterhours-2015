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
//! | `GET /v1/game` | the `game-endpoints` record: where a game reaches the server |
//! | `GET /v1/profiles` | the `profile-list` record: every save |
//! | `GET /v1/sessions` | the `sessions` record: the newest sessions |
//! | `GET /v1/sessions/<id>` | the `sessions` record of that session |
//! | `GET /v1/sessions/<id>/files/<name>` | the `session-file` record: one log or record |
//! | `POST /v1/car-images` | the `car-images` record: decode the car pictures, or reuse them |
//! | `POST /v1/map-images` | the `map-images` record: decode the world map, or reuse it |
//! | `GET /v1/content/<file>` | one decoded picture or map tile, `image/png` |
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
    net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream},
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
    /// lowercase hexadecimal characters, kept in the data directory's
    /// `api-token` across sessions until that file is deleted.
    pub token: String,
}

/// The body of every refused API request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ApiError {
    pub version: u32,
    /// What was refused and why, for a person to read.
    pub message: String,
}

/// `GET /v1/game`: where a game, on this computer or another, reaches this
/// session's server. A game is routed to a relay on its own computer that
/// forwards to these ([`crate::relay`]); the server's answers give it the
/// rest of its addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GameEndpoints {
    pub version: u32,
    /// The server's redirector, `<IPv4 address>:<port>`.
    pub redirector: String,
    /// The server's launcher services, `<IPv4 address>:<port>`, when it
    /// serves them (a session with game support); `null` otherwise.
    pub launcher_services: Option<String>,
}

impl GameEndpoints {
    /// Where `start --remote` relays to; refused when the server serves
    /// no launcher services for a game elsewhere.
    pub fn remote(&self) -> Result<crate::records::RemoteGame> {
        let parse = |text: &str| {
            text.parse::<SocketAddrV4>()
                .ok()
                .filter(|a| !a.ip().is_unspecified() && a.port() != 0)
        };
        let redirector = parse(&self.redirector)
            .ok_or_else(|| Failure::session("The server's redirector address is invalid."))?;
        let launcher_services = self.launcher_services.as_deref().and_then(parse).ok_or_else(|| {
            Failure::refused(
                "That server serves no launcher services for a game on another computer; start it with serve --server-only --listen <address>.",
            )
        })?;
        Ok(crate::records::RemoteGame {
            redirector,
            launcher_services,
        })
    }

    /// From the server's readiness record (`server.stdout.jsonl`).
    pub fn from_readiness(line: &str) -> Result<Self> {
        let invalid = || Failure::session("The server's readiness record is invalid.");
        let ready: serde_json::Value = serde_json::from_str(line).map_err(|_| invalid())?;
        let address = |key: &str| {
            ready[key]
                .as_str()
                .filter(|text| text.parse::<std::net::SocketAddrV4>().is_ok())
                .map(str::to_owned)
        };
        if ready["ready"] != true {
            return Err(invalid());
        }
        Ok(Self {
            version: FORMAT_VERSION,
            redirector: address("redirector").ok_or_else(invalid)?,
            launcher_services: address("lsx"),
        })
    }
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
    /// Where a game reaches the server, once it is ready.
    fn game(&self) -> Result<GameEndpoints>;
    /// `profile --list --json`.
    fn profiles(&self) -> Result<crate::profile::Listing>;
    /// `sessions --json`, or with `--id`.
    fn sessions(&self, id: Option<&str>) -> Result<crate::sessions::SessionList>;
    /// One file of one session folder.
    fn session_file(&self, id: &str, name: &str) -> Result<crate::sessions::file::SessionFileText>;
    /// `car-images --json`.
    fn car_images(&self) -> Result<crate::profile::images::CarImagesReport>;
    /// `map-images --json`.
    fn map_images(&self) -> Result<crate::map::MapImagesReport>;
    /// One decoded picture or map tile, by its file below the content
    /// folder ([`content_file`]).
    fn content(&self, file: &str) -> Result<Vec<u8>>;
}

/// Longest picture or tile served.
pub const MAX_CONTENT_FILE: u64 = 16 * 1024 * 1024;

/// A decoded picture or map tile below `<data>/artifacts/content`:
/// `car-images/<fingerprint>/<40 hex>.png` or
/// `map-images/<fingerprint>/<level>-<column>-<row>.png`. Anything else, a
/// link or a file over [`MAX_CONTENT_FILE`] is refused; a missing one is
/// exit 4.
pub fn content_file(data: &Path, file: &str) -> Result<Vec<u8>> {
    let refused = || Failure::refused(format!("{file} is not a decoded picture or map tile."));
    let parts: Vec<&str> = file.split('/').collect();
    let [kind, fingerprint, name] = parts.as_slice() else {
        return Err(refused());
    };
    let digits = |text: &str, most: usize| {
        !text.is_empty() && text.len() <= most && text.bytes().all(|b| b.is_ascii_digit())
    };
    let named = match *kind {
        crate::profile::images::DIRECTORY => name
            .strip_suffix(".png")
            .is_some_and(|stem| crate::records::is_lower_hex(stem, 40)),
        crate::map::DIRECTORY => name.strip_suffix(".png").is_some_and(|stem| {
            let parts: Vec<&str> = stem.split('-').collect();
            matches!(parts.as_slice(), [level, column, row]
                if digits(level, 2) && digits(column, 4) && digits(row, 4))
        }),
        _ => false,
    };
    if !named || !crate::records::is_lower_hex(fingerprint, 64) {
        return Err(refused());
    }
    let data = crate::paths::checked_data_path(data, None)?;
    let path = data
        .join("artifacts")
        .join("content")
        .join(kind)
        .join(fingerprint)
        .join(name);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| Failure::no_session(format!("No {file} has been decoded.")))?;
    if !meta.is_file() || meta.len() > MAX_CONTENT_FILE {
        return Err(refused());
    }
    Ok(std::fs::read(&path)?)
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
    /// `Host` values this API answers: its own address, and `localhost`
    /// with its port when that address is loopback.
    hosts: Vec<String>,
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
    /// Listen on `bind`: `127.0.0.1`, or the one network address a `serve
    /// --listen` session uses; port 0 lets the system pick.
    pub fn start(
        control: Arc<dyn Control>,
        hub: Arc<Hub>,
        token: String,
        bind: SocketAddrV4,
    ) -> Result<Self> {
        let listener = TcpListener::bind(bind)?;
        let address = listener.local_addr()?;
        let mut hosts = vec![address.to_string()];
        if address.ip().is_loopback() {
            hosts.push(format!("localhost:{}", address.port()));
        }
        let shared = Arc::new(Shared {
            control,
            hub,
            token,
            hosts,
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
    if !shared.hosts.iter().any(|a| a.as_bytes() == host) {
        return Some((403, "This API answers only on its own address."));
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
        ("GET", "/v1/game") => answer(&mut stream, control.game()),
        ("GET", "/v1/profiles") => answer(&mut stream, control.profiles()),
        ("POST", "/v1/car-images") => answer(&mut stream, control.car_images()),
        ("POST", "/v1/map-images") => answer(&mut stream, control.map_images()),
        ("GET", path) if path.starts_with("/v1/content/") => {
            match control.content(&path["/v1/content/".len()..]) {
                Ok(bytes) => http::write_body(&mut stream, 200, "image/png", &[], &bytes),
                Err(failure) => refuse(&mut stream, status_of(&failure), &[], &failure.message),
            }
        }
        ("GET", "/v1/sessions") => answer(&mut stream, control.sessions(None)),
        ("GET", path) if path.starts_with("/v1/sessions/") => {
            let rest = &path["/v1/sessions/".len()..];
            match rest.split('/').collect::<Vec<_>>().as_slice() {
                [id] => answer(&mut stream, control.sessions(Some(id))),
                [id, "files", name] => answer(&mut stream, control.session_file(id, name)),
                _ => refuse(&mut stream, 404, &[], "No such API address."),
            }
        }
        (
            _,
            "/v1/info" | "/v1/status" | "/v1/events" | "/v1/profile" | "/v1/game" | "/v1/profiles"
            | "/v1/sessions",
        ) => refuse(
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
        (_, "/v1/stop" | "/v1/play" | "/v1/car-images" | "/v1/map-images") => refuse(
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
    fn profiles(&self) -> Result<crate::profile::Listing> {
        crate::commands::profile::profile_list_report(&self.data, None)
    }
    fn sessions(&self, id: Option<&str>) -> Result<crate::sessions::SessionList> {
        crate::commands::sessions::sessions_report(
            &self.data,
            crate::sessions::DEFAULT_LIMIT,
            id.map(str::to_owned),
        )
    }
    fn session_file(&self, id: &str, name: &str) -> Result<crate::sessions::file::SessionFileText> {
        crate::sessions::file::read(&self.data, id, name)
    }
    fn car_images(&self) -> Result<crate::profile::images::CarImagesReport> {
        crate::commands::profile::car_images_report(&self.data, None)
    }
    fn map_images(&self) -> Result<crate::map::MapImagesReport> {
        crate::commands::map::map_images_report(&self.data, None)
    }
    fn content(&self, file: &str) -> Result<Vec<u8>> {
        content_file(&self.data, file)
    }
    fn game(&self) -> Result<GameEndpoints> {
        let not_ready = || Failure::no_session("The server is not ready yet.");
        let status = crate::session::current_status(&self.data)?.ok_or_else(not_ready)?;
        let path = crate::paths::session_dir(&self.data, &status.id).join(READINESS_FILE);
        // The supervisor keeps only the readiness line in this file.
        let mut line = String::new();
        let file = std::fs::File::open(&path).map_err(|_| not_ready())?;
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(std::io::Read::take(file, 64 * 1024)),
            &mut line,
        )?;
        if line.trim().is_empty() {
            return Err(not_ready());
        }
        GameEndpoints::from_readiness(line.trim_end())
    }
}

/// `GET /v1/game` from a session API elsewhere, for `start --remote`:
/// `url` is `http://<IPv4 address>:<port>` and `token` its API token.
pub fn fetch_game(url: &str, token: &str) -> Result<crate::records::RemoteGame> {
    let address: SocketAddrV4 = url
        .trim()
        .trim_end_matches('/')
        .strip_prefix("http://")
        .and_then(|rest| rest.parse().ok())
        .ok_or_else(|| {
            Failure::refused(format!(
                "--remote takes http://<IPv4 address>:<port>, not {url}."
            ))
        })?;
    let token = token.trim();
    if token.len() != 64
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Failure::refused(
            "--token takes the server's 64-character token from its api --json.",
        ));
    }
    let unreachable =
        |e: std::io::Error| Failure::session(format!("Could not reach the server at {url}: {e}"));
    let mut stream =
        TcpStream::connect_timeout(&address.into(), IO_TIMEOUT).map_err(unreachable)?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(unreachable)?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(unreachable)?;
    write!(
        stream,
        "GET /v1/game HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .map_err(unreachable)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(&mut stream, 64 * 1024), &mut bytes)
        .map_err(unreachable)?;
    let mut headers = [httparse::EMPTY_HEADER; http::MAX_HEADERS];
    let mut response = httparse::Response::new(&mut headers);
    let Ok(httparse::Status::Complete(end)) = response.parse(&bytes) else {
        return Err(Failure::session("The server's answer was not HTTP."));
    };
    let body = &bytes[end..];
    if response.code != Some(200) {
        let message = serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("The server answered {:?}.", response.code));
        return Err(if response.code == Some(409) {
            Failure::no_session(message)
        } else {
            Failure::refused(message)
        });
    }
    let game: GameEndpoints = serde_json::from_slice(body)
        .map_err(|_| Failure::session("The server's game-endpoints record is invalid."))?;
    game.remote()
}

/// The session file holding the server's readiness record.
const READINESS_FILE: &str = "server.stdout.jsonl";

/// A session's API while it runs: the listener and its `api.json`.
pub struct SessionApi {
    server: ApiServer,
    file: PathBuf,
}

impl SessionApi {
    /// Listen on `bind` with `token` (see [`stored_token`]), and write how
    /// to reach the API to the session directory.
    pub fn start(
        control: Arc<dyn Control>,
        hub: Arc<Hub>,
        id: &str,
        run: &Path,
        bind: SocketAddrV4,
        token: String,
    ) -> Result<Self> {
        let server = ApiServer::start(control, hub, token.clone(), bind)?;
        let record = ApiRecord {
            version: FORMAT_VERSION,
            id: id.to_owned(),
            url: server.url(),
            token,
        };
        let file = run.join(FILE);
        let mut text = serde_json::to_vec_pretty(&record)?;
        text.push(b'\n');
        write_private(&file, &text)?;
        Ok(Self { server, file })
    }

    /// The session is over: deliver the last events, close, remove `api.json`.
    pub fn finish(self) {
        self.server.finish(DRAIN_GRACE);
        let _ = std::fs::remove_file(&self.file);
    }
}

/// The environment variable a front end that starts its own server sets to
/// choose the API token, so it knows the token without reading this
/// computer's files, as it would for a server elsewhere.
pub const TOKEN_ENV: &str = "AFTERHOURS_API_TOKEN";

/// Whether `text` is an API token: 64 lowercase hexadecimal characters.
pub fn is_token(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// This session's API token: [`TOKEN_ENV`] when set, else
/// [`stored_token`].
pub fn session_token(data: &Path) -> Result<String> {
    match std::env::var(TOKEN_ENV) {
        Ok(token) if is_token(&token) => Ok(token),
        Ok(_) => Err(Failure::refused(format!(
            "{TOKEN_ENV} must be 64 lowercase hexadecimal characters."
        ))),
        Err(_) => stored_token(data),
    }
}

/// The data directory's API token: the one in `api-token`, or a new one
/// written there. It stays the same across sessions, so a launcher that
/// paired with this server (on another PC, say) reconnects after a restart;
/// deleting the file makes the next session choose a new one.
pub fn stored_token(data: &Path) -> Result<String> {
    let path = data.join(TOKEN_FILE);
    let read = |path: &Path| -> Result<String> {
        let text = std::fs::read_to_string(path)?;
        let token = text.trim_end();
        if !is_token(token) {
            return Err(Failure::refused(format!(
                "{} is damaged; delete it to choose a new API token.",
                path.display()
            )));
        }
        Ok(token.to_owned())
    };
    if std::fs::symlink_metadata(&path).is_ok() {
        return read(&path);
    }
    let token = new_token()?;
    match write_private(&path, format!("{token}\n").as_bytes()) {
        Ok(()) => Ok(token),
        // Another session wrote it first: use that one.
        Err(_) if std::fs::symlink_metadata(&path).is_ok() => read(&path),
        Err(failure) => Err(failure),
    }
}

/// The file in the data directory that keeps the API token.
pub const TOKEN_FILE: &str = "api-token";

/// Write a new file readable only by this user where the system allows it;
/// it lies under the user's data directory.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

/// `api`: how to reach the current session's API, if it is live and has one.
pub fn current(data: &Path) -> Result<ApiRecord> {
    let none = || {
        Failure::no_session("No running Afterhours session serves the API. Start one with --api.")
    };
    // A session becomes current with its first status and starts the API
    // next: until then this is exit 4, no API yet.
    let status = crate::session::current_status(data)?.ok_or_else(none)?;
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
