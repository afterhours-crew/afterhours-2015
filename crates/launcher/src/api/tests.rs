// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    contract::{Build, StopOutcome},
    events::{Body, DiagnosticSource, Emitter},
    paths,
    records::{self, Action, State, Status},
    session,
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    sync::Mutex,
};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ID: &str = "0123456789abcdef0123456789abcdef";

fn status(state: State) -> Status {
    Status {
        version: records::VERSION,
        id: ID.into(),
        action: Some(Action::Serve),
        game_running: false,
        utc: "2026-10-20T12:00:00Z".into(),
        state,
        message: format!("{state}"),
        worker_pid: 1,
        game_pid: None,
        server_pid: Some(2),
    }
}

/// Answers from fixed records and counts the requests that change things.
#[derive(Default)]
struct Fake {
    stops: Mutex<u32>,
    saves: Mutex<Vec<(Option<u16>, Option<Rivals>)>>,
}

fn settings_report(traffic_percent: u16, rival_limit: Option<u16>, saved: bool) -> SettingsReport {
    let values = settings::Settings {
        traffic_percent,
        rival_limit,
    };
    SettingsReport::new(&values, "settings.json".into(), saved)
}

impl Control for Fake {
    fn info(&self) -> VersionReport {
        VersionReport::new(&Build::LAUNCHER)
    }
    fn status(&self) -> Result<StatusReport> {
        Ok(StatusReport::new(Some(status(State::Ready))))
    }
    fn stop(&self) -> Result<StopReport> {
        *self.stops.lock().unwrap() += 1;
        Ok(StopReport::new(
            StopOutcome::Requested,
            status(State::Ready),
            Some("stop".into()),
        ))
    }
    fn play(&self) -> Result<PlayReport> {
        Err(Failure::refused(
            "The game is already running in this session.",
        ))
    }
    fn settings(&self) -> Result<SettingsReport> {
        Ok(settings_report(100, None, false))
    }
    fn save_settings(
        &self,
        traffic: Option<u16>,
        rivals: Option<Rivals>,
    ) -> Result<SettingsReport> {
        self.saves.lock().unwrap().push((traffic, rivals));
        let limit = match rivals {
            Some(Rivals::Count(count)) => Some(count),
            _ => None,
        };
        Ok(settings_report(traffic.unwrap_or(100), limit, true))
    }
    fn profile(&self) -> Result<Profile> {
        Err(Failure::no_session("No saved driver is selected."))
    }
}

fn server(hub: &Arc<Hub>) -> (ApiServer, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    let control: Arc<dyn Control> = fake.clone();
    let server = ApiServer::start(control, Arc::clone(hub), TOKEN.into()).unwrap();
    (server, fake)
}

fn host(server: &ApiServer) -> String {
    server.url().trim_start_matches("http://").to_owned()
}

/// Send raw request bytes; the status code and body of the answer.
fn exchange(server: &ApiServer, request: &[u8]) -> (u16, String, String) {
    let mut stream = TcpStream::connect(host(server)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    let code = head
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (code, head.to_owned(), body.to_owned())
}

fn request(server: &ApiServer, method: &str, path: &str) -> (u16, String, String) {
    let text = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\n\r\n",
        host(server)
    );
    exchange(server, text.as_bytes())
}

fn error_message(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(value["version"], FORMAT_VERSION);
    value["message"].as_str().unwrap().to_owned()
}

#[test]
fn the_hub_starts_readers_at_the_newest_status_and_map_frame() {
    let hub = Hub::new();
    hub.publish("status", "s0".into());
    hub.publish("diagnostic", "d1".into());
    hub.publish("map-feed", "m2".into());
    hub.publish("status", "s3".into());
    let (snapshot, cursor) = hub.subscribe();
    let texts: Vec<&str> = snapshot.iter().map(|e| &*e.json).collect();
    assert_eq!(texts, ["m2", "s3"]);
    assert_eq!(cursor, 4);
    assert_eq!(hub.wait(cursor, Duration::from_millis(10)), hub::Next::Idle);
    hub.publish("diagnostic", "d4".into());
    match hub.wait(cursor, Duration::from_secs(5)) {
        hub::Next::Events(events, next) => {
            assert_eq!(events.len(), 1);
            assert_eq!((events[0].sequence, events[0].kind), (4, "diagnostic"));
            assert_eq!(next, 5);
        }
        other => panic!("{other:?}"),
    }
    hub.close();
    hub.publish("status", "late".into());
    assert_eq!(hub.wait(5, Duration::from_secs(5)), hub::Next::Closed);
    // A reader still behind gets the rest before the end.
    assert!(matches!(hub.wait(3, Duration::ZERO), hub::Next::Events(e, 5) if e.len() == 2));
}

#[test]
fn the_hub_keeps_a_bounded_tail_for_readers_that_fall_behind() {
    let hub = Hub::new();
    for i in 0..(hub::KEPT_EVENTS + 10) {
        hub.publish("diagnostic", format!("{i}"));
    }
    match hub.wait(0, Duration::ZERO) {
        hub::Next::Events(events, next) => {
            assert_eq!(events.len(), hub::KEPT_EVENTS);
            assert_eq!(events[0].sequence, 10);
            assert_eq!(next, (hub::KEPT_EVENTS + 10) as u64);
        }
        other => panic!("{other:?}"),
    }
    let big = "x".repeat(hub::KEPT_BYTES / 4 + 1);
    for _ in 0..4 {
        hub.publish("map-feed", big.clone());
    }
    match hub.wait(0, Duration::ZERO) {
        hub::Next::Events(events, _) => {
            let bytes: usize = events.iter().map(|e| e.json.len()).sum();
            assert!(bytes <= hub::KEPT_BYTES, "{bytes}");
            assert_eq!(events.len(), 3);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn request_heads_are_parsed_within_bounds() {
    let head = http::parse_head(
        b"GET /v1/status?x=1 HTTP/1.1\r\nHost: 127.0.0.1:1\r\nauthorization: Bearer t\r\n\r\n",
    )
    .unwrap();
    assert_eq!(
        (head.method.as_str(), head.path.as_str()),
        ("GET", "/v1/status")
    );
    assert_eq!(head.header("Authorization"), Some(&b"Bearer t"[..]));
    assert_eq!(head.header("origin"), None);
    assert!(matches!(
        http::parse_head(b"hello\r\n\r\n"),
        Err(http::ReadError::Malformed)
    ));
    let many: String = (0..=http::MAX_HEADERS)
        .map(|i| format!("X-{i}: y\r\n"))
        .collect();
    let crowded = format!("GET / HTTP/1.1\r\n{many}\r\n");
    assert!(matches!(
        http::parse_head(crowded.as_bytes()),
        Err(http::ReadError::TooLarge)
    ));
    // Reading stops at the end of the head, and at the bound.
    let mut input = &b"GET / HTTP/1.1\r\nHost: h\r\n\r\nleft over"[..];
    assert_eq!(http::read_head(&mut input).unwrap().path, "/");
    assert_eq!(input, b"left over");
    let endless = vec![b'a'; http::MAX_HEAD + 1];
    assert!(matches!(
        http::read_head(&mut &endless[..]),
        Err(http::ReadError::TooLarge)
    ));
    assert!(matches!(
        http::read_head(&mut &b"GET / HTTP/1.1\r\n"[..]),
        Err(http::ReadError::Closed)
    ));
    assert!(http::same_secret(b"abc", b"abc"));
    assert!(!http::same_secret(b"abc", b"abd"));
    assert!(!http::same_secret(b"abc", b"abcd"));
}

#[test]
fn every_request_needs_the_token_its_host_and_no_origin_or_body() {
    let hub = Hub::new();
    let (server, fake) = server(&hub);
    let address = host(&server);
    let plain = |headers: &str| {
        exchange(
            &server,
            format!("POST /v1/stop HTTP/1.1\r\n{headers}\r\n").as_bytes(),
        )
    };
    let auth = format!("Authorization: Bearer {TOKEN}\r\n");
    let (code, head, body) = plain(&format!("Host: {address}\r\n"));
    assert_eq!(code, 401);
    assert!(head.contains("WWW-Authenticate: Bearer"));
    assert!(error_message(&body).contains("token"));
    let wrong = format!(
        "Host: {address}\r\nAuthorization: Bearer {}\r\n",
        "0".repeat(64)
    );
    assert_eq!(plain(&wrong).0, 401);
    assert_eq!(plain(&format!("Host: evil.example\r\n{auth}")).0, 403);
    assert_eq!(plain(&auth).0, 403, "no Host header");
    let origin = format!("Host: {address}\r\n{auth}Origin: http://evil.example\r\n");
    assert_eq!(plain(&origin).0, 403);
    let body =
        format!("POST /v1/stop HTTP/1.1\r\nHost: {address}\r\n{auth}Content-Length: 2\r\n\r\n{{}}");
    assert_eq!(exchange(&server, body.as_bytes()).0, 413);
    let chunked = format!("Host: {address}\r\n{auth}Transfer-Encoding: chunked\r\n");
    assert_eq!(plain(&chunked).0, 413);
    assert_eq!(
        *fake.stops.lock().unwrap(),
        0,
        "nothing refused reached the session"
    );
    let localhost = format!("Host: localhost:{}\r\n{auth}", server.address.port());
    assert_eq!(plain(&localhost).0, 200);
    assert_eq!(*fake.stops.lock().unwrap(), 1);
}

#[test]
fn malformed_and_oversized_requests_are_refused() {
    let hub = Hub::new();
    let (server, _) = server(&hub);
    assert_eq!(exchange(&server, b"nonsense\r\n\r\n").0, 400);
    // Exactly the bound without an end: the server reads it all, then refuses.
    let mut huge = b"GET /v1/status HTTP/1.1\r\nX: ".to_vec();
    huge.resize(http::MAX_HEAD, b'a');
    let (code, _, body) = exchange(&server, &huge);
    assert_eq!(code, 413);
    assert!(error_message(&body).contains("too large"));
}

#[test]
fn routes_answer_the_commands_records() {
    let hub = Hub::new();
    let (server, fake) = server(&hub);
    let (code, head, body) = request(&server, "GET", "/v1/info");
    assert_eq!(code, 200);
    assert!(head.contains("Content-Type: application/json"));
    assert!(head.contains("Cache-Control: no-store"));
    let info: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        info["capabilities"]
            .as_array()
            .unwrap()
            .contains(&"api".into())
    );
    let (code, _, body) = request(&server, "GET", "/v1/status");
    assert_eq!(code, 200);
    let status: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(status["session"]["state"], "ready");
    let (code, _, body) = request(&server, "POST", "/v1/stop");
    assert_eq!(code, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["outcome"],
        "requested"
    );
    assert_eq!(*fake.stops.lock().unwrap(), 1);
    let (code, _, body) = request(&server, "POST", "/v1/play");
    assert_eq!(code, 409);
    assert!(error_message(&body).contains("already running"));
    let (code, head, _) = request(&server, "GET", "/v1/stop");
    assert_eq!(code, 405);
    assert!(head.contains("Allow: POST"));
    let (code, head, _) = request(&server, "POST", "/v1/status");
    assert_eq!(code, 405);
    assert!(head.contains("Allow: GET"));
    assert_eq!(request(&server, "GET", "/v2/info").0, 404);
    assert_eq!(request(&server, "GET", "/").0, 404);
    let (code, _, body) = request(&server, "GET", "/v1/settings");
    assert_eq!(code, 200);
    let settings: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        (&settings["traffic_percent"], &settings["saved"]),
        (&100.into(), &false.into())
    );
    for method in ["POST", "DELETE", "PATCH"] {
        let (code, head, _) = request(&server, method, "/v1/settings");
        assert_eq!(code, 405, "{method}");
        assert!(head.contains("Allow: GET, PUT"));
    }
    let (code, _, body) = request(&server, "GET", "/v1/profile");
    assert_eq!(code, 409, "no save selected, as profile exits with 4");
    assert!(error_message(&body).contains("No saved driver"));
    let (code, head, _) = request(&server, "PUT", "/v1/profile");
    assert_eq!(code, 405);
    assert!(head.contains("Allow: GET\r\n"));
    assert!(fake.saves.lock().unwrap().is_empty());
}

/// `PUT /v1/settings` with the given extra headers and body bytes.
fn put_settings(server: &ApiServer, headers: &str, body: &str) -> (u16, String, String) {
    let text = format!(
        "PUT /v1/settings HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\n{headers}\r\n{body}",
        host(server)
    );
    exchange(server, text.as_bytes())
}

/// `PUT /v1/settings` with a JSON body as a client sends it.
fn put_json(server: &ApiServer, body: &str) -> (u16, String, String) {
    let headers = format!(
        "Content-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    put_settings(server, &headers, body)
}

#[test]
fn settings_are_saved_from_a_small_checked_json_body() {
    let hub = Hub::new();
    let (server, fake) = server(&hub);
    for (body, expected) in [
        (r#"{"traffic_percent":200}"#, (Some(200), None)),
        (r#"{"rival_limit":null}"#, (None, Some(Rivals::Original))),
        (
            r#"{"traffic_percent":0,"rival_limit":15}"#,
            (Some(0), Some(Rivals::Count(15))),
        ),
        (
            "  {\"rival_limit\": 0, \"traffic_percent\": 300}\n",
            (Some(300), Some(Rivals::Count(0))),
        ),
    ] {
        let (code, _, answer) = put_json(&server, body);
        assert_eq!(code, 200, "{body}: {answer}");
        let report: serde_json::Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(report["saved"], true);
        assert_eq!(fake.saves.lock().unwrap().pop(), Some(expected), "{body}");
    }
    // Media type parameters are fine; the bound is inclusive.
    let headers = "Content-Type: Application/JSON; charset=utf-8\r\nContent-Length: 23\r\n";
    assert_eq!(
        put_settings(&server, headers, r#"{"traffic_percent":150}"#).0,
        200
    );
    let padded = format!("{{\"traffic_percent\":1{}}}", " ".repeat(MAX_BODY - 21));
    assert_eq!(padded.len(), MAX_BODY);
    assert_eq!(put_json(&server, &padded).0, 200);
    assert_eq!(fake.saves.lock().unwrap().len(), 2);
    fake.saves.lock().unwrap().clear();

    // Bodies that are not exactly a change within bounds save nothing.
    for (body, why) in [
        ("{}", "traffic_percent, rival_limit or both"),
        (r#"{"traffic":50}"#, "unknown field"),
        (r#"{"traffic_percent":null}"#, "invalid type: null"),
        (r#"{"traffic_percent":301}"#, "0 to 300"),
        (r#"{"traffic_percent":70000}"#, "invalid value"),
        (r#"{"traffic_percent":-1}"#, "invalid value"),
        (r#"{"traffic_percent":"50"}"#, "invalid type"),
        (r#"{"traffic_percent":1.5}"#, "invalid type"),
        (r#"{"rival_limit":16}"#, "null or 0 to 15"),
        (r#"{"rival_limit":"original"}"#, "invalid type"),
        (
            r#"{"traffic_percent":5,"traffic_percent":6}"#,
            "duplicate field",
        ),
        (r#"{"traffic_percent":5} {}"#, "trailing characters"),
        (r#"{"traffic_percent":5"#, "EOF"),
        ("[100, null]", "a JSON object"),
        ("not json", "a JSON object"),
        ("", "a JSON object"),
    ] {
        let (code, _, answer) = put_json(&server, body);
        assert_eq!(code, 400, "{body}");
        let message = error_message(&answer);
        assert!(
            message.starts_with("Invalid settings: ") && message.contains(why),
            "{body}: {message}"
        );
    }
    let text = "Content-Type: text/plain\r\nContent-Length: 23\r\n";
    let (code, _, answer) = put_settings(&server, text, r#"{"traffic_percent":150}"#);
    assert_eq!(code, 415);
    assert!(error_message(&answer).contains("application/json"));
    assert_eq!(
        put_settings(
            &server,
            "Content-Length: 23\r\n",
            r#"{"traffic_percent":150}"#
        )
        .0,
        415
    );
    assert!(fake.saves.lock().unwrap().is_empty(), "nothing was saved");
}

#[test]
fn request_bodies_are_bounded_and_need_their_length() {
    let hub = Hub::new();
    let (server, fake) = server(&hub);
    let json = "Content-Type: application/json\r\n";
    // Refused from the head alone: no body bytes follow, so none go unread.
    for (headers, code, why) in [
        (json.to_owned(), 411, "Content-Length"),
        (
            format!("{json}Transfer-Encoding: chunked\r\n"),
            411,
            "Content-Length",
        ),
        (
            format!("{json}Content-Length: {}\r\n", MAX_BODY + 1),
            413,
            "over 1 KiB",
        ),
        (
            format!("{json}Content-Length: 99999999999999999999999\r\n"),
            400,
            "whole number",
        ),
        (format!("{json}Content-Length: +5\r\n"), 400, "whole number"),
        (format!("{json}Content-Length: \r\n"), 400, "whole number"),
        (
            format!("{json}Content-Length: 5\r\nContent-Length: 5\r\n"),
            400,
            "whole number",
        ),
    ] {
        let (status, _, answer) = put_settings(&server, &headers, "");
        assert_eq!(status, code, "{headers}");
        assert!(error_message(&answer).contains(why), "{headers}: {answer}");
    }
    // Only PUT /v1/settings has a body.
    let text = format!(
        "GET /v1/settings HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 2\r\n\r\n{{}}",
        host(&server)
    );
    let (code, _, answer) = exchange(&server, text.as_bytes());
    assert_eq!(code, 413);
    assert!(error_message(&answer).contains("Only PUT /v1/settings"));

    // A body that ends early is refused once the peer stops sending.
    let mut stream = TcpStream::connect(host(&server)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "PUT /v1/settings HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\n{json}Content-Length: 40\r\n\r\n{{\"traffic_percent\"",
        host(&server)
    )
    .unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 400 "), "{answer}");
    assert!(answer.contains("shorter than its Content-Length"));

    // A head and body that arrive in pieces are read whole.
    let body = r#"{"traffic_percent":250,"rival_limit":3}"#;
    let mut stream = TcpStream::connect(host(&server)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let whole = format!(
        "PUT /v1/settings HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\n{json}Content-Length: {}\r\n\r\n{body}",
        host(&server),
        body.len()
    );
    for piece in whole.as_bytes().chunks(17) {
        stream.write_all(piece).unwrap();
        stream.flush().unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert_eq!(
        *fake.saves.lock().unwrap(),
        [(Some(250), Some(Rivals::Count(3)))]
    );
}

#[test]
fn a_refused_request_with_a_small_body_still_gets_its_answer() {
    // Refused before its body is needed, the body is still read: a
    // connection closed over unread bytes is reset, losing the answer.
    let hub = Hub::new();
    let (server, fake) = server(&hub);
    let address = host(&server);
    let body = r#"{"traffic_percent":100}"#;
    let put = |headers: String| {
        format!(
            "PUT /v1/settings HTTP/1.1\r\nHost: {address}\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    let auth = format!("Authorization: Bearer {TOKEN}\r\n");
    let cases = [
        (put(String::new()), 401, "token"),
        (
            put(format!("{auth}Origin: http://evil.example\r\n")),
            403,
            "web pages",
        ),
        (
            format!(
                "POST /v1/play HTTP/1.1\r\nHost: {address}\r\n{auth}Content-Length: {}\r\n\r\n{body}",
                body.len()
            ),
            413,
            "Only PUT /v1/settings",
        ),
    ];
    for _ in 0..10 {
        for (request, code, why) in &cases {
            let (status, _, answer) = exchange(&server, request.as_bytes());
            assert_eq!(status, *code);
            assert!(error_message(&answer).contains(why), "{answer}");
        }
    }
    assert!(fake.saves.lock().unwrap().is_empty());
}

/// Open the event stream; the reader positioned after the response head.
fn open_stream(server: &ApiServer) -> BufReader<TcpStream> {
    let mut stream = TcpStream::connect(host(server)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET /v1/events HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\n\r\n",
        host(server)
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        head.push_str(&line);
    }
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert!(head.contains("Content-Type: text/event-stream"));
    reader
}

/// The next event: its id, kind and data.
fn next_event(reader: &mut BufReader<TcpStream>) -> Option<(u64, String, String)> {
    let (mut id, mut kind, mut data) = (None, None, None);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end_matches('\n');
        if line.is_empty() {
            if let (Some(id), Some(kind), Some(data)) = (id, kind.take(), data.take()) {
                return Some((id, kind, data));
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("id: ") {
            id = value.parse().ok();
        } else if let Some(value) = line.strip_prefix("event: ") {
            kind = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("data: ") {
            data = Some(value.to_owned());
        }
    }
}

#[test]
fn the_event_stream_sends_the_newest_state_then_every_event_until_the_end() {
    let hub = Hub::new();
    let mut emitter = Emitter::disabled();
    emitter.attach(Arc::clone(&hub));
    emitter.status(&status(State::Starting));
    emitter.emit(Body::MapFeed {
        frame: 7,
        skipped: 0,
        worlds: Vec::new(),
    });
    emitter.status(&status(State::Ready));
    let (server, _) = server(&hub);
    let mut reader = open_stream(&server);
    let (id, kind, data) = next_event(&mut reader).unwrap();
    assert_eq!((id, kind.as_str()), (1, "map-feed"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&data).unwrap()["frame"],
        7
    );
    let (id, kind, data) = next_event(&mut reader).unwrap();
    assert_eq!((id, kind.as_str()), (2, "status"));
    let ready: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(ready["status"]["state"], "ready");
    assert_eq!(ready["event"], "status");
    emitter.diagnostic(DiagnosticSource::Server, "first line\nsecond");
    let (id, kind, data) = next_event(&mut reader).unwrap();
    assert_eq!((id, kind.as_str()), (3, "diagnostic"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&data).unwrap()["text"],
        "first line"
    );
    emitter.flush_all();
    emitter.status(&status(State::Finished));
    server.finish(Duration::from_secs(5));
    let (_, _, data) = next_event(&mut reader).unwrap();
    assert!(data.contains("second"));
    let (_, kind, data) = next_event(&mut reader).unwrap();
    assert_eq!(kind, "status");
    assert!(data.contains("\"finished\""));
    assert_eq!(
        next_event(&mut reader),
        None,
        "the stream ends with the session"
    );
}

#[test]
fn event_streams_are_bounded() {
    let hub = Hub::new();
    let (server, _) = server(&hub);
    let readers: Vec<_> = (0..MAX_STREAMS).map(|_| open_stream(&server)).collect();
    while server.open_streams() < MAX_STREAMS {
        std::thread::sleep(Duration::from_millis(10));
    }
    let (code, _, body) = request(&server, "GET", "/v1/events");
    assert_eq!(code, 503);
    assert!(error_message(&body).contains("event streams"));
    // Commands still get through while the streams are full.
    assert_eq!(request(&server, "GET", "/v1/status").0, 200);
    drop(readers);
    hub.publish("status", "{}".into());
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.open_streams() > 0 && Instant::now() < deadline {
        hub.publish("diagnostic", "{}".into());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.open_streams(), 0, "closed readers free their slots");
}

#[test]
fn a_finished_api_stops_listening() {
    let hub = Hub::new();
    let (server, _) = server(&hub);
    let address = host(&server);
    server.finish(Duration::from_secs(1));
    let refused = match TcpStream::connect(&address) {
        Err(_) => true,
        Ok(mut stream) => {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let _ = stream.write_all(b"GET /v1/info HTTP/1.1\r\n\r\n");
            let mut answer = String::new();
            stream
                .read_to_string(&mut answer)
                .map_or(true, |_| answer.is_empty())
        }
    };
    assert!(refused);
}

#[test]
fn the_emitter_publishes_to_the_api_even_after_stdout_fails() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let hub = Hub::new();
    let mut emitter = Emitter::to(Broken);
    emitter.attach(Arc::clone(&hub));
    emitter.status(&status(State::Ready));
    assert!(!emitter.enabled(), "the failed write stops stdout");
    assert!(emitter.observed());
    emitter.diagnostic(DiagnosticSource::Game, "line\n");
    match hub.wait(0, Duration::ZERO) {
        hub::Next::Events(events, _) => {
            let kinds: Vec<&str> = events.iter().map(|e| e.kind).collect();
            assert_eq!(kinds, ["status", "diagnostic"]);
            let first: serde_json::Value = serde_json::from_str(&events[0].json).unwrap();
            assert_eq!(first["version"], crate::events::FORMAT_VERSION);
        }
        other => panic!("{other:?}"),
    }
    assert!(!Emitter::disabled().observed());
}

#[test]
fn tokens_are_new_random_hex() {
    let a = new_token().unwrap();
    let b = new_token().unwrap();
    assert_eq!(a.len(), 64);
    assert!(
        a.bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_ne!(a, b);
}

/// A data directory whose current session is live: its status says
/// `ready` and the returned lock stands in for its supervisor.
fn live_session(name: &str) -> (std::path::PathBuf, session::Lock) {
    let data = crate::testing::scratch(name);
    let run = paths::session_dir(&data, ID);
    std::fs::create_dir_all(&run).unwrap();
    records::write_new_json(&run.join("status.json"), &status(State::Ready)).unwrap();
    session::point_current_session(&data, ID).unwrap();
    let lock = session::Lock::acquire(&data).unwrap();
    (data, lock)
}

#[test]
fn the_session_api_writes_how_to_reach_it_and_forgets_the_token_at_the_end() {
    let (data, lock) = live_session("api-record");
    let run = paths::session_dir(&data, ID);
    assert_eq!(current(&data).unwrap_err().exit, Exit::NoSession);
    let control = Arc::new(LiveControl::new(data.clone(), Build::LAUNCHER));
    let api = SessionApi::start(control, Hub::new(), ID, &run).unwrap();
    let record = current(&data).unwrap();
    assert_eq!(record.id, ID);
    assert!(record.url.starts_with("http://127.0.0.1:"));
    assert_eq!(record.token.len(), 64);
    // The live control answers the commands' records over the session's data.
    let mut stream = TcpStream::connect(record.url.trim_start_matches("http://")).unwrap();
    write!(
        stream,
        "POST /v1/stop HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n\r\n",
        record.url.trim_start_matches("http://"),
        record.token
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(answer.contains("\"outcome\":\"requested\""));
    assert!(run.join(session::STOP_FILE).exists());
    api.finish();
    assert!(!run.join(FILE).exists());
    assert_eq!(current(&data).unwrap_err().exit, Exit::NoSession);
    // A record left by another session is not this one's.
    let stale = ApiRecord {
        version: FORMAT_VERSION,
        id: "f".repeat(32),
        url: "http://127.0.0.1:1".into(),
        token: TOKEN.into(),
    };
    records::write_new_json(&run.join(FILE), &stale).unwrap();
    assert_eq!(current(&data).unwrap_err().exit, Exit::Refused);
    drop(lock);
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn a_session_without_its_first_status_has_no_api_yet() {
    // The supervisor points at its session, starts the API, then writes the
    // first status: `api` polled in between answers "not yet" (exit 4).
    let data = crate::testing::scratch("api-starting");
    let run = paths::session_dir(&data, ID);
    std::fs::create_dir_all(&run).unwrap();
    session::point_current_session(&data, ID).unwrap();
    let lock = session::Lock::acquire(&data).unwrap();
    assert!(session::current_status(&data).is_err(), "no status yet");
    assert_eq!(current(&data).unwrap_err().exit, Exit::NoSession);
    let record = ApiRecord {
        version: FORMAT_VERSION,
        id: ID.into(),
        url: "http://127.0.0.1:1".into(),
        token: TOKEN.into(),
    };
    records::write_new_json(&run.join(FILE), &record).unwrap();
    assert_eq!(current(&data).unwrap_err().exit, Exit::NoSession);
    records::write_new_json(&run.join("status.json"), &status(State::Ready)).unwrap();
    assert_eq!(current(&data).unwrap(), record);
    // Anything else wrong is still reported as it is.
    drop(lock);
    std::fs::write(data.join(session::POINTER_FILE), b"not json").unwrap();
    assert_eq!(current(&data).unwrap_err().exit, Exit::Refused);
    std::fs::remove_dir_all(data).unwrap();
}

/// One request to a live session's API, with a JSON body when given; its
/// status code and body.
fn live_request(record: &ApiRecord, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let address = record.url.trim_start_matches("http://");
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let length = body.map_or_else(String::new, |body| {
        format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        )
    });
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\n{length}\r\n{}",
        record.token,
        body.unwrap_or_default()
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap();
    let code = head.split(' ').nth(1).unwrap().parse().unwrap();
    (code, body.to_owned())
}

/// A save for a driver called `Api Tester`, selected by `account.json`.
fn select_saved_driver(data: &std::path::Path) {
    use nfs_storage::{
        AccountId, Batch, GarageSlots, Op, Timestamp,
        account::{Document, Kind, Repository},
    };
    let id = "5a".repeat(16);
    let account = AccountId::from_owned_config([0x5a; 16]).unwrap();
    let identity = serde_json::json!({"version": 1, "storage_account": id,
        "persona": 41, "account": 42, "name": "Api Tester"});
    let documents: Vec<_> = Kind::ALL
        .into_iter()
        .map(|kind| {
            let bytes = if kind == Kind::Identity {
                identity.to_string().into_bytes()
            } else {
                b"{}".to_vec()
            };
            (kind, Document { revision: 1, bytes })
        })
        .collect();
    let batch = Batch {
        id: 1,
        expected_generation: 0,
        ops: vec![Op::SetGarage(GarageSlots::default())],
    };
    let accounts = data.join("artifacts").join("account");
    std::fs::create_dir_all(&accounts).unwrap();
    Repository::open_owned_directory(&accounts)
        .unwrap()
        .initialize_fresh(account, &documents, &batch, Timestamp(1))
        .unwrap();
    records::write_new_json(
        &data.join("account.json"),
        &records::AccountRecord::post_intro(id),
    )
    .unwrap();
}

#[test]
fn the_live_api_reads_and_saves_settings_and_reads_the_profile_as_the_commands_do() {
    let (data, lock) = live_session("api-settings-profile");
    let run = paths::session_dir(&data, ID);
    // The session started an empty installation, so no test reads a
    // registered game for car names.
    let game = data.join("game");
    std::fs::create_dir_all(&game).unwrap();
    let request = serde_json::json!({"version": 1, "id": ID, "action": "serve",
        "game_directory": game, "data_directory": data, "server_path": "server",
        "server_sha256": "0".repeat(64), "player_name": "Driver", "record_protocol": false});
    std::fs::write(run.join("request.json"), request.to_string()).unwrap();
    let control = Arc::new(LiveControl::new(data.clone(), Build::LAUNCHER));
    let api = SessionApi::start(control, Hub::new(), ID, &run).unwrap();
    let record = current(&data).unwrap();

    // Settings: the command's record, the original amounts until saved.
    let (code, body) = live_request(&record, "GET", "/v1/settings", None);
    assert_eq!(code, 200, "{body}");
    let report: serde_json::Value = serde_json::from_str(&body).unwrap();
    crate::contract::testing::validate("settings", &report);
    let command = crate::commands::settings::settings_report(&data, None, None).unwrap();
    assert_eq!(report, serde_json::to_value(&command).unwrap());
    assert_eq!(
        (&report["traffic_percent"], &report["rival_limit"]),
        (&100.into(), &serde_json::Value::Null)
    );
    // Each save is applied to the file at once; a field left out is kept.
    for (body, traffic, rivals) in [
        (r#"{"traffic_percent":250,"rival_limit":4}"#, 250, Some(4)),
        (r#"{"rival_limit":null}"#, 250, None),
        (r#"{"traffic_percent":0}"#, 0, None),
    ] {
        let (code, answer) = live_request(&record, "PUT", "/v1/settings", Some(body));
        assert_eq!(code, 200, "{body}: {answer}");
        let report: serde_json::Value = serde_json::from_str(&answer).unwrap();
        crate::contract::testing::validate("settings", &report);
        assert_eq!(report["saved"], true);
        assert_eq!(
            (&report["traffic_percent"], &report["rival_limit"]),
            (&traffic.into(), &serde_json::json!(rivals))
        );
        let saved = settings::load(&data).unwrap();
        assert_eq!(
            (saved.traffic_percent, saved.rival_limit),
            (traffic, rivals)
        );
    }
    // A damaged file is refused as the command refuses it, and kept.
    let file = data.join(settings::FILE);
    let damaged = br#"{"traffic_percent": 900}"#;
    std::fs::write(&file, damaged).unwrap();
    assert_eq!(live_request(&record, "GET", "/v1/settings", None).0, 409);
    let put = live_request(&record, "PUT", "/v1/settings", Some(r#"{"rival_limit":1}"#));
    assert_eq!(put.0, 409, "{}", put.1);
    assert_eq!(std::fs::read(&file).unwrap(), damaged);
    std::fs::remove_file(&file).unwrap();
    // Clients saving at once are served one at a time: every save lands
    // whole and none collides over the temporary file.
    let savers: Vec<_> = (0..8u16)
        .map(|i| {
            let record = record.clone();
            std::thread::spawn(move || {
                let body = format!(r#"{{"traffic_percent":{},"rival_limit":{i}}}"#, i * 10);
                live_request(&record, "PUT", "/v1/settings", Some(&body))
            })
        })
        .collect();
    for saver in savers {
        let (code, answer) = saver.join().unwrap();
        assert_eq!(code, 200, "{answer}");
    }
    let last = settings::load(&data).unwrap();
    assert_eq!(
        last.rival_limit.map(|limit| limit * 10),
        Some(last.traffic_percent),
        "one request's values, not a mix"
    );
    let leftovers = std::fs::read_dir(&data)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(leftovers, 0);

    // Profile: refused as the command refuses it until a save is selected.
    let (code, body) = live_request(&record, "GET", "/v1/profile", None);
    assert_eq!(code, 409, "{body}");
    let refusal = crate::commands::profile::profile_report(&data, &Default::default()).unwrap_err();
    assert_eq!(refusal.exit, Exit::NoSession);
    assert_eq!(error_message(&body), refusal.message);
    select_saved_driver(&data);
    let (code, body) = live_request(&record, "GET", "/v1/profile", None);
    assert_eq!(code, 200, "{body}");
    let profile: serde_json::Value = serde_json::from_str(&body).unwrap();
    crate::contract::testing::validate("profile", &profile);
    let command = crate::commands::profile::profile_report(&data, &Default::default()).unwrap();
    assert_eq!(profile, serde_json::to_value(&command).unwrap());
    assert_eq!(profile["identity"]["display_name"], "Api Tester");
    assert_eq!(profile["active"], true);
    assert!(
        profile["localization"].is_null(),
        "names come from the session's installation, which is empty"
    );
    api.finish();
    drop(lock);
    std::fs::remove_dir_all(data).unwrap();
}
