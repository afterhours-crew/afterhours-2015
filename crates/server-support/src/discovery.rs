// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Strict bounded HTTP/XML discovery for an already-bound loopback listener.
use nfs_protocol::redirector::{
    IpAddress, Limits, ServerInstanceInfo, request::ServerInstanceRequest,
};
use std::{
    fmt,
    io::{self, Read},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::Instant,
};
pub const MAX_HEADER: usize = 16 * 1024;
pub const MAX_BODY: usize = 64 * 1024;
pub const MAX_HEADERS: usize = 64;
pub const ERROR_RESPONSE: &[u8] =
    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
pub const ROUTE: &str = "/redirector/getServerInstance";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    InvalidHttp,
    HeaderLimit,
    HeaderCount,
    ContentLength,
    BodyLimit,
    UnsupportedFraming,
    Host,
    Deadline,
    Eof,
    Io,
    Allocation,
    NonLoopback,
    Output,
    IneligibleRequest,
    Reply,
    ProfileShape,
    ProfileConfig,
    ProfileQos,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "server {self:?}")
    }
}
impl std::error::Error for Failure {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Head {
    pub bytes: usize,
    pub body_bytes: usize,
    pub expected_route: bool,
    pub xml_content_type: bool,
}

pub fn parse_head(bytes: &[u8]) -> Result<Option<Head>, Failure> {
    let end = bytes
        .windows(4)
        .position(|s| s == b"\r\n\r\n")
        .map(|p| p + 4);
    let Some(end) = end else {
        return if bytes.len() >= MAX_HEADER {
            Err(Failure::HeaderLimit)
        } else {
            Ok(None)
        };
    };
    if end > MAX_HEADER {
        return Err(Failure::HeaderLimit);
    }
    let bytes = &bytes[..end];
    if bytes.starts_with(b"\r\n")
        || bytes.iter().enumerate().any(|(i, b)| match b {
            b'\n' => i == 0 || bytes[i - 1] != b'\r',
            b'\r' => bytes.get(i + 1) != Some(&b'\n'),
            _ => false,
        })
    {
        return Err(Failure::InvalidHttp);
    }
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(bytes) {
        Ok(httparse::Status::Complete(n)) if n == end => {}
        Err(httparse::Error::TooManyHeaders) => return Err(Failure::HeaderCount),
        _ => return Err(Failure::InvalidHttp),
    }
    let mut length = None;
    let mut host_count = 0;
    let mut content_type_count = 0;
    let mut xml_content_type = false;
    for h in request.headers.iter() {
        if h.name.eq_ignore_ascii_case("content-length") {
            let value = h.value.trim_ascii();
            if length.is_some() || value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return Err(Failure::ContentLength);
            }
            let mut n = 0_usize;
            for digit in value {
                n = n
                    .checked_mul(10)
                    .and_then(|n| n.checked_add((digit - b'0') as usize))
                    .ok_or(Failure::ContentLength)?;
            }
            if n > MAX_BODY {
                return Err(Failure::BodyLimit);
            }
            length = Some(n);
        } else if h.name.eq_ignore_ascii_case("transfer-encoding")
            || h.name.eq_ignore_ascii_case("expect")
        {
            return Err(Failure::UnsupportedFraming);
        } else if h.name.eq_ignore_ascii_case("host") {
            host_count += 1;
            if h.value.trim_ascii().is_empty() || host_count > 1 {
                return Err(Failure::Host);
            }
        } else if h.name.eq_ignore_ascii_case("content-type") {
            content_type_count += 1;
            if content_type_count > 1 {
                return Err(Failure::InvalidHttp);
            }
            let media_type = h
                .value
                .split(|b| *b == b';')
                .next()
                .unwrap_or_default()
                .trim_ascii();
            xml_content_type = media_type.eq_ignore_ascii_case(b"application/xml")
                || media_type.eq_ignore_ascii_case(b"text/xml");
        }
    }
    if request.version == Some(1) && host_count != 1 {
        return Err(Failure::Host);
    }
    Ok(Some(Head {
        bytes: end,
        body_bytes: length.ok_or(Failure::ContentLength)?,
        expected_route: request.method == Some("POST") && request.path == Some(ROUTE),
        xml_content_type,
    }))
}

pub struct Capture {
    pub wire: Vec<u8>,
    pub head: Option<Head>,
    pub result: Result<(), Failure>,
}

pub fn receive(reader: &mut impl Read) -> Capture {
    let mut capture = Capture {
        wire: Vec::new(),
        head: None,
        result: Ok(()),
    };
    if capture
        .wire
        .try_reserve_exact(MAX_HEADER + MAX_BODY)
        .is_err()
    {
        capture.result = Err(Failure::Allocation);
        return capture;
    }
    loop {
        if capture.head.is_none() {
            match parse_head(&capture.wire) {
                Ok(head) => capture.head = head,
                Err(error) => {
                    capture.result = Err(error);
                    break;
                }
            }
        }
        let limit = match capture.head {
            Some(head) => {
                let total = head.bytes + head.body_bytes;
                if capture.wire.len() >= total {
                    break;
                }
                total
            }
            None => MAX_HEADER,
        };
        let mut buf = [0_u8; 4096];
        let wanted = buf.len().min(limit - capture.wire.len());
        match reader.read(&mut buf[..wanted]) {
            Ok(0) => {
                capture.result = Err(Failure::Eof);
                break;
            }
            Ok(n) => capture.wire.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                capture.result = Err(
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) {
                        Failure::Deadline
                    } else {
                        Failure::Io
                    },
                );
                break;
            }
        }
    }
    capture
}

impl Capture {
    pub fn body(&self) -> Option<&[u8]> {
        self.result.ok()?;
        let h = self.head?;
        self.wire.get(h.bytes..h.bytes + h.body_bytes)
    }
}
struct DeadlineSocket<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}
impl Read for DeadlineSocket<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(io::ErrorKind::TimedOut)?;
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(buf)
    }
}

pub fn receive_socket(stream: &mut TcpStream, deadline: Instant) -> Capture {
    receive(&mut DeadlineSocket { stream, deadline })
}

pub fn eligible(capture: &Capture) -> bool {
    let Some(head) = capture.head else {
        return false;
    };
    if !head.expected_route
        || !head.xml_content_type
        || capture.wire.len() != head.bytes + head.body_bytes
    {
        return false;
    }
    let Some(body) = capture.body() else {
        return false;
    };
    let Ok(request) = ServerInstanceRequest::decode_xml(
        body,
        nfs_protocol::redirector::request::Limits {
            max_input_bytes: MAX_BODY,
            max_string_bytes: MAX_BODY,
            max_unknown_bytes: MAX_BODY,
            ..Default::default()
        },
    ) else {
        return false;
    };
    request.unknown.is_empty()
        && request.client_type.is_some_and(|v| v.value == 0)
        && request.client_platform.is_some_and(|v| v.value == 4)
        && request.client_locale.is_some()
        && request.is_trial == Some(false)
        && request.platform.as_deref() == Some("Windows")
        && [
            &request.blaze_sdk_version,
            &request.blaze_sdk_build_date,
            &request.client_name,
            &request.client_sku_id,
            &request.client_version,
            &request.dirty_sdk_version,
            &request.environment,
            &request.name,
            &request.connection_profile,
        ]
        .iter()
        .all(|field| field.as_ref().is_some_and(|value| !value.is_empty()))
}

pub fn local_response(address: SocketAddr) -> Result<Vec<u8>, Failure> {
    local_response_with_ca(address, None)
}

pub fn local_response_with_ca(address: SocketAddr, ca: Option<&[u8]>) -> Result<Vec<u8>, Failure> {
    if address.ip() != Ipv4Addr::LOCALHOST || address.port() == 0 {
        return Err(Failure::NonLoopback);
    }
    if ca.is_some_and(|bytes| bytes.is_empty() || bytes.len() > 4096) {
        return Err(Failure::BodyLimit);
    }
    let certificate_list = ca.into_iter().collect::<Vec<_>>();
    let body = ServerInstanceInfo {
        address: IpAddress {
            hostname: "127.0.0.1",
            ip: u32::from(Ipv4Addr::LOCALHOST),
            port: address.port(),
        },
        address_remaps: &[],
        certificate_list: &certificate_list,
        messages: &[],
        name_remaps: &[],
        secure: false,
        trial_service_name: "",
        default_dns_address: 0,
    }
    .encode_xml(Limits {
        max_output_bytes: if ca.is_some() { 8192 } else { 512 },
        max_blob_bytes: 4096,
        ..Default::default()
    })
    .map_err(|_| Failure::Reply)?;
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nX-BLAZE-ERRORCODE: 0\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut response = Vec::new();
    response
        .try_reserve_exact(header.len() + body.len())
        .map_err(|_| Failure::Allocation)?;
    response.extend_from_slice(header.as_bytes());
    response.extend_from_slice(&body);
    Ok(response)
}
