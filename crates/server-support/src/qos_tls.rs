// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::{
    fmt,
    io::{self, Read, Write},
    mem,
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    time::{Duration, Instant},
};

use openssl::{
    asn1::Asn1Time,
    bn::{BigNum, MsbOption},
    error::ErrorStack,
    hash::MessageDigest,
    pkey::PKey,
    rsa::Rsa,
    ssl::{
        Error as SslError, ErrorCode, Ssl, SslContext, SslContextBuilder, SslMethod, SslOptions,
        SslSessionCacheMode, SslStream, SslVersion,
    },
    x509::{
        X509, X509NameBuilder,
        extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName},
    },
};

pub const MAX_WIRE_BYTES: usize = 64 * 1024;
pub const MAX_HTTP_HEADER_BYTES: usize = 8 * 1024;
const CIPHERS: &str = "AES256-GCM-SHA384:AES128-GCM-SHA256";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    Identity,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TLS observation {self:?}")
    }
}
impl std::error::Error for Failure {}

pub struct Identity {
    context: SslContext,
    ca_pem: Vec<u8>,
}
impl Identity {
    pub fn new() -> Result<Self, Failure> {
        Self::generate().map_err(|_| Failure::Identity)
    }

    pub fn ca_pem(&self) -> &[u8] {
        &self.ca_pem
    }

    fn generate() -> Result<Self, ErrorStack> {
        let ca_key = PKey::from_rsa(Rsa::generate(2048)?)?;
        let leaf_key = PKey::from_rsa(Rsa::generate(2048)?)?;
        let mut ca_name = X509NameBuilder::new()?;
        ca_name.append_entry_by_text("CN", "NFS local observation CA")?;
        let ca_name = ca_name.build();
        let mut ca = X509::builder()?;
        ca.set_version(2)?;
        ca.set_serial_number(serial()?.as_ref())?;
        ca.set_subject_name(&ca_name)?;
        ca.set_issuer_name(&ca_name)?;
        ca.set_pubkey(&ca_key)?;
        ca.set_not_before(Asn1Time::days_from_now(0)?.as_ref())?;
        ca.set_not_after(Asn1Time::days_from_now(1)?.as_ref())?;
        ca.append_extension(BasicConstraints::new().critical().ca().pathlen(0).build()?)?;
        ca.append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()?,
        )?;
        ca.sign(&ca_key, MessageDigest::sha256())?;
        let ca = ca.build();

        let mut leaf_name = X509NameBuilder::new()?;
        leaf_name.append_entry_by_text("CN", "127.0.0.1")?;
        let mut leaf = X509::builder()?;
        leaf.set_version(2)?;
        leaf.set_serial_number(serial()?.as_ref())?;
        leaf.set_subject_name(&leaf_name.build())?;
        leaf.set_issuer_name(ca.subject_name())?;
        leaf.set_pubkey(&leaf_key)?;
        leaf.set_not_before(Asn1Time::days_from_now(0)?.as_ref())?;
        leaf.set_not_after(Asn1Time::days_from_now(1)?.as_ref())?;
        leaf.append_extension(BasicConstraints::new().critical().build()?)?;
        leaf.append_extension(
            KeyUsage::new()
                .critical()
                .digital_signature()
                .key_encipherment()
                .build()?,
        )?;
        leaf.append_extension(ExtendedKeyUsage::new().server_auth().build()?)?;
        let san = SubjectAlternativeName::new()
            .ip("127.0.0.1")
            .build(&leaf.x509v3_context(Some(&ca), None))?;
        leaf.append_extension(san)?;
        leaf.sign(&ca_key, MessageDigest::sha256())?;
        let leaf = leaf.build();

        let mut context = SslContextBuilder::new(SslMethod::tls_server())?;
        context.set_min_proto_version(Some(SslVersion::TLS1_2))?;
        context.set_max_proto_version(Some(SslVersion::TLS1_2))?;
        context.set_cipher_list(CIPHERS)?;
        context.set_security_level(2);
        context.set_options(
            SslOptions::NO_COMPRESSION | SslOptions::NO_TICKET | SslOptions::NO_RENEGOTIATION,
        );
        context.set_session_cache_mode(SslSessionCacheMode::OFF);
        context.set_certificate(&leaf)?;
        context.set_private_key(&leaf_key)?;
        context.check_private_key()?;
        Ok(Self {
            context: context.build(),
            ca_pem: ca.to_pem()?,
        })
    }
}

fn serial() -> Result<openssl::asn1::Asn1Integer, ErrorStack> {
    let mut serial = BigNum::new()?;
    serial.rand(128, MsbOption::ONE, false)?;
    serial.to_asn1_integer()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stop {
    HeaderComplete,
    ResponseComplete,
    UnsupportedRequest,
    Deadline,
    Eof,
    WireLimit,
    HeaderLimit,
    InvalidHttp,
    TlsFailure,
    Io,
    NonLoopback,
    Setup,
    Allocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Alert {
    pub level: u8,
    pub description: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TlsErrorShape {
    pub code: i32,
    pub reason: Option<i32>,
    pub io_kind: Option<io::ErrorKind>,
    pub client_alert: Option<Alert>,
    pub server_alert: Option<Alert>,
}

pub struct Observation {
    pub client_wire: Vec<u8>,
    pub server_wire: Vec<u8>,
    pub http_header: Vec<u8>,
    pub http_response: Vec<u8>,
    pub http_response_bytes: usize,
    pub tls_established: bool,
    pub protocol: Option<&'static str>,
    pub cipher: Option<&'static str>,
    pub status: Stop,
    pub tls_error: Option<TlsErrorShape>,
}
impl Observation {
    fn empty(status: Stop) -> Self {
        Self {
            client_wire: Vec::new(),
            server_wire: Vec::new(),
            http_header: Vec::new(),
            http_response: Vec::new(),
            http_response_bytes: 0,
            tls_established: false,
            protocol: None,
            cipher: None,
            status,
            tls_error: None,
        }
    }
}

pub fn capture(socket: TcpStream, deadline: Instant, identity: &Identity) -> Observation {
    capture_inner(socket, deadline, identity, None)
}

pub fn serve_latency(
    socket: TcpStream,
    deadline: Instant,
    identity: &Identity,
    udp: SocketAddrV4,
) -> Observation {
    if *udp.ip() != Ipv4Addr::LOCALHOST || udp.port() == 0 {
        return Observation::empty(Stop::NonLoopback);
    }
    capture_inner(socket, deadline, identity, Some(udp))
}

fn capture_inner(
    socket: TcpStream,
    deadline: Instant,
    identity: &Identity,
    udp: Option<SocketAddrV4>,
) -> Observation {
    let mut result = Observation::empty(Stop::Setup);
    let addresses = socket
        .local_addr()
        .and_then(|local| socket.peer_addr().map(|peer| (local, peer)));
    match addresses {
        Ok((local, peer)) if local.ip().is_loopback() && peer.ip().is_loopback() => {}
        Ok(_) => {
            result.status = Stop::NonLoopback;
            return result;
        }
        Err(_) => {
            result.status = Stop::Io;
            return result;
        }
    }
    if deadline <= Instant::now() {
        result.status = Stop::Deadline;
        return result;
    }
    if socket.set_nonblocking(false).is_err() {
        result.status = Stop::Io;
        return result;
    }
    let Ok(ssl) = Ssl::new(&identity.context) else {
        return result;
    };
    let io = CaptureIo {
        socket,
        deadline,
        client: Vec::new(),
        server: Vec::new(),
        stopped: None,
    };
    let Ok(mut stream) = SslStream::new(ssl, io) else {
        return result;
    };
    let handshake = stream.accept();
    result.protocol = (stream.ssl().version_str() == "TLSv1.2").then_some("TLSv1.2");
    result.cipher = stream
        .ssl()
        .current_cipher()
        .and_then(|cipher| match cipher.name() {
            "AES256-GCM-SHA384" => Some("AES256-GCM-SHA384"),
            "AES128-GCM-SHA256" => Some("AES128-GCM-SHA256"),
            _ => None,
        });
    match handshake {
        Err(error) => record_error(&mut result, &error, stream.get_ref()),
        Ok(()) => {
            result.tls_established = true;
            observe_header(&mut stream, &mut result);
            if result.status == Stop::HeaderComplete
                && let Some(udp) = udp
            {
                respond_latency(&mut stream, &mut result, udp);
            }
        }
    }
    result.client_wire = mem::take(&mut stream.get_mut().client);
    result.server_wire = mem::take(&mut stream.get_mut().server);
    if let Some(error) = &mut result.tls_error {
        error.client_alert = first_plaintext_alert(&result.client_wire);
        error.server_alert = first_plaintext_alert(&result.server_wire);
    }
    result
}

fn respond_latency(stream: &mut SslStream<CaptureIo>, result: &mut Observation, udp: SocketAddrV4) {
    let Some(response) = latency_response(&result.http_header, udp) else {
        result.status = Stop::UnsupportedRequest;
        return;
    };
    result.http_response = response;
    while result.http_response_bytes < result.http_response.len() {
        if stream.get_mut().remaining().is_err() {
            result.status = Stop::Deadline;
            return;
        }
        match stream.ssl_write(&result.http_response[result.http_response_bytes..]) {
            Ok(0) => {
                result.status = Stop::Io;
                return;
            }
            Ok(n) => result.http_response_bytes += n,
            Err(error) => {
                record_error(result, &error, stream.get_ref());
                return;
            }
        }
    }
    result.status = Stop::ResponseComplete;
    if let Err(error) = stream.shutdown() {
        record_error(result, &error, stream.get_ref());
    }
}

fn latency_response(header: &[u8], udp: SocketAddrV4) -> Option<Vec<u8>> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut request = httparse::Request::new(&mut headers);
    if !matches!(request.parse(header), Ok(httparse::Status::Complete(n)) if n == header.len())
        || request.method != Some("GET")
        || request.version != Some(1)
    {
        return None;
    }
    nfs_protocol::qos::parse_latency_target(request.path?.as_bytes()).ok()?;
    let mut hosts = 0;
    let mut lengths = 0;
    for h in request.headers {
        if h.name.eq_ignore_ascii_case("Transfer-Encoding") {
            return None;
        }
        if h.name.eq_ignore_ascii_case("Content-Length") {
            lengths += 1;
            if lengths > 1 || h.value != b"0" {
                return None;
            }
        }
        if h.name.eq_ignore_ascii_case("Host") {
            hosts += 1;
            if hosts > 1 || h.value.is_empty() {
                return None;
            }
        }
    }
    if hosts != 1 {
        return None;
    }
    let body = nfs_protocol::qos::encode_latency_xml(*udp.ip(), udp.port()).ok()?;
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
    response.extend_from_slice(&body);
    Some(response)
}

fn observe_header(stream: &mut SslStream<CaptureIo>, result: &mut Observation) {
    if result
        .http_header
        .try_reserve_exact(MAX_HTTP_HEADER_BYTES)
        .is_err()
    {
        result.status = Stop::Allocation;
        return;
    }
    while result.http_header.len() < MAX_HTTP_HEADER_BYTES {
        if stream.get_mut().remaining().is_err() {
            result.status = Stop::Deadline;
            return;
        }
        let mut byte = [0];
        match stream.ssl_read(&mut byte) {
            Ok(0) => {
                result.status = Stop::Eof;
                return;
            }
            Ok(_) => result.http_header.push(byte[0]),
            Err(error) => {
                record_error(result, &error, stream.get_ref());
                return;
            }
        }
        if result.http_header.ends_with(b"\r\n\r\n") {
            result.status = if valid_header(&result.http_header) {
                Stop::HeaderComplete
            } else {
                Stop::InvalidHttp
            };
            return;
        }
    }
    result.status = Stop::HeaderLimit;
}

fn valid_header(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"\r\n")
        || bytes.iter().enumerate().any(|(i, byte)| match byte {
            b'\n' => i == 0 || bytes[i - 1] != b'\r',
            b'\r' => bytes.get(i + 1) != Some(&b'\n'),
            _ => false,
        })
    {
        return false;
    }
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut request = httparse::Request::new(&mut headers);
    matches!(request.parse(bytes), Ok(httparse::Status::Complete(n)) if n == bytes.len())
        && request.method.is_some()
        && request.path.is_some()
        && request.version.is_some()
}

fn record_error(result: &mut Observation, error: &SslError, io: &CaptureIo) {
    result.status = io
        .stopped
        .unwrap_or(if error.code() == ErrorCode::ZERO_RETURN {
            Stop::Eof
        } else {
            Stop::TlsFailure
        });
    result.tls_error = Some(TlsErrorShape {
        code: error.code().as_raw(),
        reason: error
            .ssl_error()
            .and_then(|stack| stack.errors().last())
            .map(|e| e.reason_code()),
        io_kind: error.io_error().map(io::Error::kind),
        client_alert: None,
        server_alert: None,
    });
}

fn first_plaintext_alert(mut wire: &[u8]) -> Option<Alert> {
    while wire.len() >= 5 {
        if wire[1] != 3 || wire[2] > 3 || !(20..=23).contains(&wire[0]) {
            return None;
        }
        let size = usize::from(u16::from_be_bytes([wire[3], wire[4]]));
        let record = wire.get(..5 + size)?;
        if record[0] == 20 {
            return None;
        }
        if record[0] == 21 && size == 2 {
            return Some(Alert {
                level: record[5],
                description: record[6],
            });
        }
        wire = &wire[5 + size..];
    }
    None
}

struct CaptureIo {
    socket: TcpStream,
    deadline: Instant,
    client: Vec<u8>,
    server: Vec<u8>,
    stopped: Option<Stop>,
}
impl CaptureIo {
    fn remaining(&mut self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| {
                self.stopped = Some(Stop::Deadline);
                io::ErrorKind::TimedOut.into()
            })
    }

    fn capacity(&mut self, requested: usize) -> io::Result<usize> {
        let left = MAX_WIRE_BYTES - self.client.len() - self.server.len();
        if left == 0 {
            self.stopped = Some(Stop::WireLimit);
            Err(io::Error::other("TLS observation wire limit"))
        } else {
            Ok(left.min(requested))
        }
    }

    fn failed(&mut self, error: io::Error) -> io::Error {
        self.stopped = Some(
            if matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) {
                Stop::Deadline
            } else {
                Stop::Io
            },
        );
        error
    }
}
impl Read for CaptureIo {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let left = self.remaining()?;
            let wanted = self.capacity(bytes.len())?;
            self.client.try_reserve_exact(wanted).map_err(|_| {
                self.stopped = Some(Stop::Allocation);
                io::Error::other("TLS observation allocation")
            })?;
            self.socket
                .set_read_timeout(Some(left))
                .map_err(|e| self.failed(e))?;
            match self.socket.read(&mut bytes[..wanted]) {
                Ok(n) => {
                    self.client.extend_from_slice(&bytes[..n]);
                    if n == 0 {
                        self.stopped = Some(Stop::Eof);
                    }
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(self.failed(e)),
            }
        }
    }
}
impl Write for CaptureIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let left = self.remaining()?;
            let wanted = self.capacity(bytes.len())?;
            self.server.try_reserve_exact(wanted).map_err(|_| {
                self.stopped = Some(Stop::Allocation);
                io::Error::other("TLS observation allocation")
            })?;
            self.socket
                .set_write_timeout(Some(left))
                .map_err(|e| self.failed(e))?;
            match self.socket.write(&bytes[..wanted]) {
                Ok(0) => return Err(self.failed(io::ErrorKind::WriteZero.into())),
                Ok(n) => {
                    self.server.extend_from_slice(&bytes[..n]);
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(self.failed(e)),
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.remaining()?;
        self.socket.flush().map_err(|e| self.failed(e))
    }
}
