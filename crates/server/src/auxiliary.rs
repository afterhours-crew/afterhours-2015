// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::record::{Direction, Recorder};
use nfs_storage::{
    AccountId,
    records::{Record, Repository, UnixMicros},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinSet,
    time::{Instant, timeout_at},
};
use tracing::{info, warn};

pub const SEED_BYTES: usize = 32;
pub const MAX_HEADER_BYTES: usize = 8192;
pub const MAX_BODY_BYTES: usize = nfs_storage::records::MAX_RECORD_BYTES;
pub const MAX_REQUEST_BYTES: usize = MAX_HEADER_BYTES + MAX_BODY_BYTES;
const MAX_HEADERS: usize = 32;
const MAX_CONNECTIONS: usize = 4;
const DEADLINE: Duration = Duration::from_secs(10);
pub const NO_MATCHING_RECORD: u32 = 0x0012_001f;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stop {
    Token,
    MissingRecord,
    Record,
    Stored,
    Deleted,
    Storage,
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
pub struct LocalRecords {
    persona: i64,
    account: i64,
    storage: Option<(Arc<dyn Repository>, AccountId)>,
    workers: Arc<Semaphore>,
}
/// Compatibility name for callers constructing an explicitly empty catalog.
pub type EmptyLocalRecords = LocalRecords;
impl LocalRecords {
    pub fn new(persona: i64, account: i64) -> Option<Self> {
        (persona > 0 && account > 0 && persona != account).then_some(Self {
            persona,
            account,
            storage: None,
            workers: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        })
    }
    pub fn with_storage(mut self, repository: Arc<dyn Repository>, account: AccountId) -> Self {
        self.storage = Some((repository, account));
        self
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
    name: &'a str,
    operation: Operation<'a>,
}
enum Operation<'a> {
    Get,
    Head,
    Put {
        content_type: &'a str,
        bytes: &'a [u8],
    },
    Delete,
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
            if length.is_some_and(|n| n > 4096) {
                return Err(Stop::BodyLimit);
            }
            if length.is_none() || get("content-type") != Some(b"application/x-www-form-urlencoded")
            {
                return Err(Stop::UnsupportedRequest);
            }
            token_body(&bytes[end..])?;
            Route::Token
        }
        Some(method @ ("GET" | "HEAD" | "PUT" | "DELETE")) => {
            if method != "PUT" && length.unwrap_or(0) != 0 {
                return Err(Stop::UnsupportedRequest);
            }
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
            let (mut owner, mut kind, mut subrecord, mut update) = (None, None, None, None);
            for field in query.split('&') {
                let (key, value) = field.split_once('=').ok_or(Stop::InvalidHttp)?;
                match key {
                    "ownerId" if owner.is_none() => owner = Some(integer(value.as_bytes())?),
                    "ownerType" if kind.is_none() => kind = Some(integer(value.as_bytes())?),
                    "subrecord" if subrecord.is_none() && value.is_empty() => subrecord = Some(()),
                    "subrecordupdate" if update.is_none() && value == "false" => update = Some(()),
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
            let operation = match method {
                "GET" | "HEAD" if update.is_none() => {
                    if method == "GET" {
                        Operation::Get
                    } else {
                        Operation::Head
                    }
                }
                "PUT"
                    if owner_type == 2
                        && length.is_some()
                        && update.is_some()
                        && subrecord.is_none() =>
                {
                    let content_type =
                        std::str::from_utf8(get("content-type").ok_or(Stop::InvalidHttp)?)
                            .map_err(|_| Stop::InvalidHttp)?;
                    if content_type != "image/jpeg" && content_type != "application/octet-stream" {
                        return Err(Stop::UnsupportedRequest);
                    }
                    Operation::Put {
                        content_type,
                        bytes: &bytes[end..],
                    }
                }
                "DELETE" if owner_type == 2 && update.is_none() && subrecord.is_none() => {
                    Operation::Delete
                }
                _ => return Err(Stop::UnsupportedRequest),
            };
            Route::Record(RecordRequest {
                user: integer(get("x-user-id").ok_or(Stop::InvalidHttp)?)?,
                owner: owner.ok_or(Stop::InvalidHttp)?,
                owner_type,
                name,
                operation,
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
    answer_at(request, authority, records, seed, None)
}
fn answer_at(
    request: Request<'_>,
    authority: &str,
    records: &LocalRecords,
    seed: &[u8; SEED_BYTES],
    now: Option<UnixMicros>,
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
        Route::Record(record) if records.permits(&record) => record_answer(record, records, now),
        Route::Record(_) => Ok((
            Stop::Forbidden,
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        )),
    }
}
fn missing_record() -> (Stop, Vec<u8>) {
    (Stop::MissingRecord,format!("HTTP/1.1 404 Not Found\r\nX-BLAZE-ERRORCODE: {NO_MATCHING_RECORD}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes())
}
fn record_wire(record: &Record, head: bool) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nX-CREATION: {}\r\nX-LASTUPDATE: {}\r\nConnection: close\r\n\r\n", record.content_type, record.bytes.len(), record.created.0, record.updated.0).into_bytes();
    if !head {
        out.extend_from_slice(&record.bytes);
    }
    out
}
fn record_answer(
    record: RecordRequest<'_>,
    records: &LocalRecords,
    now: Option<UnixMicros>,
) -> Result<(Stop, Vec<u8>), Stop> {
    let storage = records.storage.as_ref().filter(|_| record.owner_type == 2);
    match record.operation {
        Operation::Get | Operation::Head => {
            let Some((store, account)) = storage else {
                return Ok(missing_record());
            };
            match store
                .get(*account, record.name)
                .map_err(|_| Stop::Storage)?
            {
                Some(value) => Ok((
                    Stop::Record,
                    record_wire(&value, matches!(record.operation, Operation::Head)),
                )),
                None => Ok(missing_record()),
            }
        }
        Operation::Put {
            content_type,
            bytes,
        } => {
            let (store, account) = storage.ok_or(Stop::UnsupportedRequest)?;
            let value = store
                .put(
                    *account,
                    record.name,
                    content_type,
                    bytes,
                    now.ok_or(Stop::InvalidHttp)?,
                )
                .map_err(|_| Stop::Storage)?;
            // The generic client PUT callback consumes status only. A success is
            // emitted only after the owned repository has committed the payload.
            Ok((Stop::Stored,format!("HTTP/1.1 200 OK\r\nX-LASTUPDATE: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",value.updated.0).into_bytes()))
        }
        Operation::Delete => {
            let (store, account) = storage.ok_or(Stop::UnsupportedRequest)?;
            if now.is_none() {
                return Err(Stop::InvalidHttp);
            }
            store
                .delete(*account, record.name)
                .map_err(|_| Stop::Storage)?;
            Ok((
                Stop::Deleted,
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            ))
        }
    }
}
/// Synchronous entry point with explicit time for durable record operations.
/// Run off async workers; responses contain the stored payload, never a template.
pub fn response_at(
    bytes: &[u8],
    authority: &str,
    records: &LocalRecords,
    seed: &[u8; SEED_BYTES],
    now: UnixMicros,
) -> Result<Option<(Stop, Vec<u8>)>, Stop> {
    inspect(bytes)?
        .map(|request| answer_at(request, authority, records, seed, Some(now)))
        .transpose()
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
    let peer = stream.peer_addr().map_err(|_| Stop::IoError)?;
    if !crate::net::serves(local.ip(), peer.ip()) {
        return Err(Stop::NonLoopback);
    }
    let mut chunk = [0u8; 1024];
    let (stop, response) = loop {
        if inspect(&result.request)?.is_some() {
            let permit = records
                .workers
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| Stop::Storage)?;
            let bytes = result.request.clone();
            let records = records.clone();
            let seed = *seed;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| Stop::Storage)?
                .as_micros();
            let now = UnixMicros(u64::try_from(now).map_err(|_| Stop::Storage)?);
            let response = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                response_at(&bytes, &local.to_string(), &records, &seed, now)
            })
            .await
            .map_err(|_| Stop::Storage)??
            .ok_or(Stop::InvalidHttp)?;
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
                    let note=(!matches!(observed.stop,Stop::Token|Stop::MissingRecord|Stop::Record|Stop::Stored|Stop::Deleted)).then_some("unsupported");
                    let _=recorder.chunk("auxiliary",n,Direction::In,&observed.request,note).await;
                    if observed.response_bytes>0 { let _=recorder.chunk("auxiliary",n,Direction::Out,&observed.response[..observed.response_bytes],None).await; }
                    info!(connection=n,%peer,stop=?observed.stop,"auxiliary request");
                    // One exchange per connection; do not print the URL,
                    // query, headers or body (they may carry authentication).
                    if observed.stop == Stop::UnsupportedRequest {
                        warn!(service="auxiliary",connection=n,disposition="closing","unsupported operation received");
                    }
                });
            },
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
