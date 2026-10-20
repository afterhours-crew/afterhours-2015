// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The small, bounded part of HTTP/1.1 the API speaks: one request per
//! connection, a small JSON request body only with a `Content-Length`, a
//! JSON response or an event stream, and `Connection: close`.

use std::io::{self, Read, Write};

/// Longest request head accepted, request line and headers together.
pub const MAX_HEAD: usize = 8 * 1024;
/// Most request headers accepted.
pub const MAX_HEADERS: usize = 32;

/// A parsed request head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub method: String,
    /// The path without any query string.
    pub path: String,
    headers: Vec<(String, Vec<u8>)>,
}

impl Head {
    /// The value of the first header named `name`, ignoring case.
    pub fn header(&self, name: &str) -> Option<&[u8]> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_slice())
    }

    /// The values of every header named `name`, ignoring case.
    pub fn headers<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a [u8]> {
        self.headers
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_slice())
    }
}

/// The request's `Content-Length`: `None` without one;
/// [`ReadError::Malformed`] unless there is one and it is a whole number.
pub fn content_length(head: &Head) -> Result<Option<usize>, ReadError> {
    let mut values = head.headers("content-length");
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() || value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return Err(ReadError::Malformed);
    }
    std::str::from_utf8(value)
        .ok()
        .and_then(|text| text.parse().ok())
        .map(Some)
        .ok_or(ReadError::Malformed)
}

/// Whether the request says its body is JSON (`application/json`, with or
/// without parameters such as `charset=utf-8`).
pub fn is_json(head: &Head) -> bool {
    head.header("content-type").is_some_and(|value| {
        let media = value.split(|&b| b == b';').next().unwrap_or_default();
        media.trim_ascii().eq_ignore_ascii_case(b"application/json")
    })
}

/// Read the `length` bytes of body that follow a head; the caller bounds
/// `length`. [`ReadError::Closed`] when the peer sends fewer.
pub fn read_body(stream: &mut impl Read, length: usize) -> Result<Vec<u8>, ReadError> {
    let mut body = vec![0u8; length];
    let mut filled = 0;
    while filled < length {
        match stream.read(&mut body[filled..]) {
            Ok(0) => return Err(ReadError::Closed),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ReadError::Io(e)),
        }
    }
    Ok(body)
}

/// Why a request head could not be read.
#[derive(Debug)]
pub enum ReadError {
    /// The peer closed the connection before sending a whole head.
    Closed,
    /// The head is longer than [`MAX_HEAD`] or has too many headers.
    TooLarge,
    /// The bytes are not an HTTP/1.x request head.
    Malformed,
    Io(io::Error),
}

/// Read and parse one request head, reading no further than its end or
/// [`MAX_HEAD`] bytes.
pub fn read_head(stream: &mut impl Read) -> Result<Head, ReadError> {
    let mut buffer = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    // Byte at a time: a head is small, and nothing after it is consumed.
    while !buffer.ends_with(b"\r\n\r\n") {
        if buffer.len() >= MAX_HEAD {
            return Err(ReadError::TooLarge);
        }
        match stream.read(&mut byte) {
            Ok(0) => return Err(ReadError::Closed),
            Ok(_) => buffer.push(byte[0]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ReadError::Io(e)),
        }
    }
    parse_head(&buffer)
}

/// Parse a complete request head.
pub fn parse_head(bytes: &[u8]) -> Result<Head, ReadError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(bytes) {
        Ok(httparse::Status::Complete(n)) if n == bytes.len() => {}
        Err(httparse::Error::TooManyHeaders) => return Err(ReadError::TooLarge),
        _ => return Err(ReadError::Malformed),
    }
    let (Some(method), Some(target)) = (request.method, request.path) else {
        return Err(ReadError::Malformed);
    };
    let path = target.split('?').next().unwrap_or_default().to_owned();
    Ok(Head {
        method: method.to_owned(),
        path,
        headers: request
            .headers
            .iter()
            .map(|h| (h.name.to_owned(), h.value.to_vec()))
            .collect(),
    })
}

/// The reason phrase of the status codes the API sends.
pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

/// Write a whole response with a JSON body.
pub fn write_json(
    stream: &mut impl Write,
    status: u16,
    extra: &[(&str, &str)],
    body: &[u8],
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        reason(status),
        body.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(COMMON_HEADERS);
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

const COMMON_HEADERS: &str =
    "Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n";

/// Start an event stream (`text/event-stream`).
pub fn write_stream_head(stream: &mut impl Write) -> io::Result<()> {
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n")?;
    stream.write_all(COMMON_HEADERS.as_bytes())?;
    stream.flush()
}

/// One server-sent event. `data` is one line of JSON, which never holds a
/// line break.
pub fn write_event(stream: &mut impl Write, id: u64, kind: &str, data: &str) -> io::Result<()> {
    debug_assert!(!data.contains('\n') && !data.contains('\r'));
    stream.write_all(format!("id: {id}\nevent: {kind}\ndata: {data}\n\n").as_bytes())
}

/// A comment line that keeps an idle stream open and finds readers gone.
pub fn write_keep_alive(stream: &mut impl Write) -> io::Result<()> {
    stream.write_all(b": keep-alive\n\n")?;
    stream.flush()
}

/// Compare two byte strings in time that depends only on their lengths.
pub fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
