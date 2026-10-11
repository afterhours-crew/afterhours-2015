// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::record::{Direction, Recorder};
use std::{
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::{Instant, timeout_at},
};
use tracing::info;

pub const SEED_BYTES: usize = 32;
pub const MAX_HEADER_BYTES: usize = 8192;
pub const MAX_BODY_BYTES: usize = 4096;
pub const MAX_REQUEST_BYTES: usize = MAX_HEADER_BYTES + MAX_BODY_BYTES;
const MAX_HEADERS: usize = 32;
const MAX_CONNECTIONS: usize = 4;
const DEADLINE: Duration = Duration::from_secs(10);
pub const NO_MATCHING_RECORD: u32 = 0x0012_001f;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stop {
    Token,
    MissingRecord,
    Forbidden,
    UnsupportedRequest,
    InvalidHttp,
    HeaderLimit,
    HeaderCount,
    BodyLimit,
    ExtraBytes,
    PeerClosed,
    Deadline,
    IoError,
    NonLoopback,
}
pub struct Observation {
    pub stop: Stop,
    pub request: Vec<u8>,
    pub response: Vec<u8>,
    pub response_bytes: usize,
}
#[derive(Clone)]
pub struct EmptyLocalRecords {
    persona: i64,
    account: i64,
}
impl EmptyLocalRecords {
    pub fn new(persona: i64, account: i64) -> Option<Self> {
        (persona > 0 && account > 0 && persona != account).then_some(Self { persona, account })
    }
    fn permits(&self, request: &RecordRequest<'_>) -> bool {
        request.user == self.persona
            && request.owner
                == match request.owner_type {
                    1 => self.account,
                    2 => self.persona,
                    _ => return false,
                }
    }
}

struct RecordRequest<'a> {
    user: i64,
    owner: i64,
    owner_type: u8,
    _name: &'a str,
}
enum Route<'a> {
    Token,
    Record(RecordRequest<'a>),
}
struct Request<'a> {
    host: &'a [u8],
    route: Route<'a>,
}

fn integer(bytes: &[u8]) -> Result<i64, Stop> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(Stop::InvalidHttp);
    }
    let value = std::str::from_utf8(bytes)
        .map_err(|_| Stop::InvalidHttp)?
        .parse()
        .map_err(|_| Stop::InvalidHttp)?;
    if value <= 0 {
        return Err(Stop::InvalidHttp);
    }
    Ok(value)
}

fn inspect(bytes: &[u8]) -> Result<Option<Request<'_>>, Stop> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(Stop::BodyLimit);
    }
    let Some(end) = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
    else {
        return if bytes.len() >= MAX_HEADER_BYTES {
            Err(Stop::HeaderLimit)
        } else {
            Ok(None)
        };
    };
    if end > MAX_HEADER_BYTES {
        return Err(Stop::HeaderLimit);
    }
    let header = &bytes[..end];
    if header.starts_with(b"\r\n")
        || header.iter().enumerate().any(|(i, b)| match b {
            b'\n' => i == 0 || header[i - 1] != b'\r',
            b'\r' => header.get(i + 1) != Some(&b'\n'),
            _ => false,
        })
    {
        return Err(Stop::InvalidHttp);
    }
    let mut slots = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut parsed = httparse::Request::new(&mut slots);
    match parsed.parse(header) {
        Ok(httparse::Status::Complete(n)) if n == end => {}
        Err(httparse::Error::TooManyHeaders) => return Err(Stop::HeaderCount),
        _ => return Err(Stop::InvalidHttp),
    }
    if parsed.version != Some(1) {
        return Err(Stop::UnsupportedRequest);
    }
    let mut length = None;
    for (index, h) in parsed.headers.iter().enumerate() {
        if parsed.headers[..index]
            .iter()
            .any(|prior| prior.name.eq_ignore_ascii_case(h.name))
        {
            return Err(Stop::InvalidHttp);
        }
        if h.name.eq_ignore_ascii_case("content-length") {
            let value = h.value.trim_ascii();
            if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return Err(Stop::InvalidHttp);
            }
            let mut n = 0usize;
            for digit in value {
                n = n
                    .checked_mul(10)
                    .and_then(|n| n.checked_add(usize::from(digit - b'0')))
                    .ok_or(Stop::BodyLimit)?;
            }
            if n > MAX_BODY_BYTES {
                return Err(Stop::BodyLimit);
            }
            length = Some(n);
        } else if h.name.eq_ignore_ascii_case("transfer-encoding")
            || h.name.eq_ignore_ascii_case("expect")
        {
            return Err(Stop::UnsupportedRequest);
        }
    }
    let get = |name: &str| {
        parsed
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.trim_ascii())
    };
    let host = get("host")
        .filter(|v| !v.is_empty())
        .ok_or(Stop::InvalidHttp)?;
    let expected = end + length.unwrap_or(0);
    if bytes.len() > expected {
        return Err(Stop::ExtraBytes);
    }
    if bytes.len() < expected {
        return Ok(None);
    }
    let path = parsed.path.ok_or(Stop::InvalidHttp)?;
    let route = match parsed.method {
        Some("POST") if path == "/connect/token" => {
            if length.is_none() || get("content-type") != Some(b"application/x-www-form-urlencoded")
            {
                return Err(Stop::UnsupportedRequest);
            }
            token_body(&bytes[end..])?;
            Route::Token
        }
        Some("GET") if length.unwrap_or(0) == 0 => {
            let (path, query) = path.split_once('?').ok_or(Stop::UnsupportedRequest)?;
            let tail = path
                .strip_prefix("/1.0/contexts/")
                .ok_or(Stop::UnsupportedRequest)?;
            let (prefix, name) = tail
                .split_once("/records/")
                .ok_or(Stop::UnsupportedRequest)?;
            let owner_type = match prefix {
                "nfs-rivals-common/categories/Pictures" => 1,
                "nfs-2016-pc/categories/Liveries" => 2,
                _ => return Err(Stop::UnsupportedRequest),
            };
            if name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return Err(Stop::UnsupportedRequest);
            }
            let (mut owner, mut kind, mut subrecord) = (None, None, None);
            for field in query.split('&') {
                let (key, value) = field.split_once('=').ok_or(Stop::InvalidHttp)?;
                match key {
                    "ownerId" if owner.is_none() => owner = Some(integer(value.as_bytes())?),
                    "ownerType" if kind.is_none() => kind = Some(integer(value.as_bytes())?),
                    "subrecord" if subrecord.is_none() && value.is_empty() => subrecord = Some(()),
                    _ => return Err(Stop::UnsupportedRequest),
                }
            }
            if kind != Some(i64::from(owner_type))
                || get("x-user-type") != Some(b"NUCLEUS_PERSONA")
                || get("x-token-type") != Some(b"NUCLEUS_ACCESS_TOKEN")
                || get("authorization").is_none_or(|v| v.is_empty())
            {
                return Err(Stop::UnsupportedRequest);
            }
            Route::Record(RecordRequest {
                user: integer(get("x-user-id").ok_or(Stop::InvalidHttp)?)?,
                owner: owner.ok_or(Stop::InvalidHttp)?,
                owner_type,
                _name: name,
            })
        }
        _ => return Err(Stop::UnsupportedRequest),
    };
    Ok(Some(Request { host, route }))
}
fn token_body(body: &[u8]) -> Result<(), Stop> {
    if !body.iter().all(u8::is_ascii_graphic) {
        return Err(Stop::UnsupportedRequest);
    }
    let mut parts = body.split(|b| *b == b'&');
    for (i, name) in [
        b"grant_type".as_slice(),
        b"code",
        b"redirect_uri",
        b"client_id",
        b"client_secret",
    ]
    .into_iter()
    .enumerate()
    {
        let part = parts.next().ok_or(Stop::UnsupportedRequest)?;
        let at = part
            .iter()
            .position(|b| *b == b'=')
            .ok_or(Stop::UnsupportedRequest)?;
        if &part[..at] != name
            || part[at + 1..].is_empty()
            || (i == 0 && &part[at + 1..] != b"authorization_code")
        {
            return Err(Stop::UnsupportedRequest);
        }
    }
    if parts.next().is_some() {
        return Err(Stop::UnsupportedRequest);
    }
    Ok(())
}

fn answer(
    request: Request<'_>,
    authority: &str,
    records: &EmptyLocalRecords,
    seed: &[u8; SEED_BYTES],
) -> Result<(Stop, Vec<u8>), Stop> {
    if request.host != authority.as_bytes() {
        return Err(Stop::UnsupportedRequest);
    }
    match request.route {
        Route::Token => {
            if seed.iter().all(|b| *b == 0) {
                return Err(Stop::InvalidHttp);
            }
            let mut token = String::with_capacity(64);
            for byte in seed {
                token.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
                token.push(char::from(b"0123456789abcdef"[usize::from(byte & 15)]));
            }
            Ok((Stop::Token,format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 83\r\nConnection: close\r\n\r\n{{\"access_token\":\"{token}\"}}").into_bytes()))
        }
        Route::Record(record) if records.permits(&record) => {
            Ok((Stop::MissingRecord,format!("HTTP/1.1 404 Not Found\r\nX-BLAZE-ERRORCODE: {NO_MATCHING_RECORD}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()))
        }
        Route::Record(_) => Ok((
            Stop::Forbidden,
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        )),
    }
}
pub fn response(
    bytes: &[u8],
    authority: &str,
    records: &EmptyLocalRecords,
    seed: &[u8; SEED_BYTES],
) -> Result<Option<(Stop, Vec<u8>)>, Stop> {
    inspect(bytes)?
        .map(|request| answer(request, authority, records, seed))
        .transpose()
}

async fn exchange(
    stream: &mut TcpStream,
    records: &EmptyLocalRecords,
    seed: &[u8; SEED_BYTES],
    result: &mut Observation,
) -> Result<Stop, Stop> {
    let local = stream.local_addr().map_err(|_| Stop::IoError)?;
    if local.ip() != Ipv4Addr::LOCALHOST
        || stream.peer_addr().map_err(|_| Stop::IoError)?.ip() != Ipv4Addr::LOCALHOST
    {
        return Err(Stop::NonLoopback);
    }
    let mut chunk = [0u8; 1024];
    let (stop, response) = loop {
        if let Some(response) = response(&result.request, &local.to_string(), records, seed)? {
            break response;
        }
        let available = (MAX_REQUEST_BYTES - result.request.len()).min(chunk.len());
        if available == 0 {
            return Err(Stop::BodyLimit);
        }
        let n = stream
            .read(&mut chunk[..available])
            .await
            .map_err(|_| Stop::IoError)?;
        if n == 0 {
            return Err(Stop::PeerClosed);
        }
        result.request.extend_from_slice(&chunk[..n]);
    };
    result.response = response;
    while result.response_bytes < result.response.len() {
        let n = stream
            .write(&result.response[result.response_bytes..])
            .await
            .map_err(|_| Stop::IoError)?;
        if n == 0 {
            return Err(Stop::IoError);
        }
        result.response_bytes += n;
    }
    Ok(stop)
}

async fn serve(
    mut stream: TcpStream,
    records: &EmptyLocalRecords,
    seed: &[u8; SEED_BYTES],
    deadline: Instant,
) -> Observation {
    let mut result = Observation {
        stop: Stop::IoError,
        request: Vec::new(),
        response: Vec::new(),
        response_bytes: 0,
    };
    result.stop =
        match timeout_at(deadline, exchange(&mut stream, records, seed, &mut result)).await {
            Ok(Ok(stop) | Err(stop)) => stop,
            Err(_) => Stop::Deadline,
        };
    result
}

pub async fn listener(
    listener: TcpListener,
    records: EmptyLocalRecords,
    seed: [u8; SEED_BYTES],
    connections: Arc<AtomicU32>,
    recorder: Recorder,
) {
    let records = Arc::new(records);
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept(), if tasks.len()<MAX_CONNECTIONS => {
                let Ok((stream,peer))=accepted else { break };
                let n=connections.fetch_add(1,Ordering::Relaxed);
                let records=records.clone();
                let recorder=recorder.clone();
                tasks.spawn(async move {
                    let observed=serve(stream,&records,&seed,Instant::now()+DEADLINE).await;
                    let note=(!matches!(observed.stop,Stop::Token|Stop::MissingRecord)).then_some("unsupported");
                    let _=recorder.chunk("auxiliary",n,Direction::In,&observed.request,note).await;
                    if observed.response_bytes>0 { let _=recorder.chunk("auxiliary",n,Direction::Out,&observed.response[..observed.response_bytes],None).await; }
                    info!(connection=n,%peer,stop=?observed.stop,"auxiliary request");
                });
            },
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
