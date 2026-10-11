// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::{
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream},
    sync::{Arc, OnceLock},
    thread,
    time::{Duration, Instant},
};

use nfs_server_support::qos_tls::{
    Identity, MAX_HTTP_HEADER_BYTES, MAX_WIRE_BYTES, Observation, Stop, capture,
};
use openssl::{
    nid::Nid,
    ssl::{
        Ssl, SslContext, SslContextBuilder, SslMethod, SslOptions, SslStream, SslVerifyMode,
        SslVersion,
    },
    x509::{X509, X509VerifyResult},
};

const LIMIT: Duration = Duration::from_secs(4);
const AES256: &str = "AES256-GCM-SHA384";
const AES128: &str = "AES128-GCM-SHA256";

fn identity() -> Arc<Identity> {
    static ID: OnceLock<Arc<Identity>> = OnceLock::new();
    Arc::clone(ID.get_or_init(|| Arc::new(Identity::new().unwrap())))
}

fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let client = TcpStream::connect_timeout(&listener.local_addr().unwrap(), LIMIT).unwrap();
    let deadline = Instant::now() + LIMIT;
    loop {
        match listener.accept() {
            Ok((server, peer)) => {
                assert!(peer.ip().is_loopback());
                return (server, client);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(e) => panic!("loopback accept failed: {e}"),
        }
    }
}

struct DeadlineIo {
    socket: TcpStream,
    deadline: Instant,
}
impl DeadlineIo {
    fn new(socket: TcpStream) -> Self {
        Self {
            socket,
            deadline: Instant::now() + LIMIT,
        }
    }
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| io::ErrorKind::TimedOut.into())
    }
}
impl Read for DeadlineIo {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.socket.set_read_timeout(Some(self.remaining()?))?;
        self.socket.read(bytes)
    }
}
impl Write for DeadlineIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.socket.set_write_timeout(Some(self.remaining()?))?;
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.remaining()?;
        self.socket.flush()
    }
}

fn client_context(ca: Option<&[u8]>, cipher: &str) -> SslContext {
    let mut context = SslContextBuilder::new(SslMethod::tls_client()).unwrap();
    context
        .set_min_proto_version(Some(SslVersion::TLS1_2))
        .unwrap();
    context
        .set_max_proto_version(Some(SslVersion::TLS1_2))
        .unwrap();
    context.set_cipher_list(cipher).unwrap();
    context.set_security_level(2);
    context.set_options(SslOptions::from_bits_retain(1));
    context.set_sigalgs_list("rsa_pkcs1_sha256").unwrap();
    context.set_verify(SslVerifyMode::PEER);
    if let Some(ca) = ca {
        context
            .cert_store_mut()
            .add_cert(X509::from_pem(ca).unwrap())
            .unwrap();
    }
    context.build()
}

fn client(socket: TcpStream, context: &SslContext, ip: Ipv4Addr) -> SslStream<DeadlineIo> {
    let mut ssl = Ssl::new(context).unwrap();
    ssl.set_hostname("127.0.0.1").unwrap();
    ssl.param_mut().set_ip(IpAddr::V4(ip)).unwrap();
    SslStream::new(ssl, DeadlineIo::new(socket)).unwrap()
}

fn run_request(id: Arc<Identity>, cipher: &str, request: &[u8], chunk: usize) -> Observation {
    let context = client_context(Some(id.ca_pem()), cipher);
    let (server, socket) = pair();
    let handle = thread::spawn(move || capture(server, Instant::now() + LIMIT, &id));
    let mut client = client(socket, &context, Ipv4Addr::LOCALHOST);
    let established = client.connect().is_ok();
    let mut application_response = [0; 1];
    let mut response_bytes = 0;
    if established {
        for bytes in request.chunks(chunk) {
            if client.write_all(bytes).is_err() {
                break;
            }
        }
        response_bytes = client.read(&mut application_response).unwrap_or(0);
    }
    drop(client);
    let result = handle.join().unwrap();
    assert!(established, "synthetic trusted client handshake failed");
    assert_eq!(response_bytes, 0, "observer must send no HTTP response");
    assert!(result.client_wire.len() + result.server_wire.len() <= MAX_WIRE_BYTES);
    assert!(result.http_header.len() <= MAX_HTTP_HEADER_BYTES);
    result
}

#[test]
fn latency_service_answers_only_observed_bodyless_get_and_reports_actual_bytes() {
    let udp = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let std::net::SocketAddr::V4(endpoint) = udp.local_addr().unwrap() else {
        unreachable!()
    };
    for (method, target, version, headers, accepted) in [
        (
            "GET",
            "/qos/qos?vers=1&qtyp=1&prpt=3659",
            "HTTP/1.1",
            "Host: 127.0.0.1\r\n",
            true,
        ),
        (
            "GET",
            "/qos/qos?vers=1&qtyp=1&prpt=3659",
            "HTTP/1.1",
            "Host: 127.0.0.1\r\nContent-Length: 0\r\n",
            true,
        ),
        (
            "POST",
            "/qos/qos?vers=1&qtyp=1&prpt=3659",
            "HTTP/1.1",
            "Host: 127.0.0.1\r\n",
            false,
        ),
        (
            "GET",
            "/qos/qos?vers=1&qtyp=2&prpt=3659",
            "HTTP/1.1",
            "Host: 127.0.0.1\r\n",
            false,
        ),
        (
            "GET",
            "/qos/firewall?vers=1&nint=2",
            "HTTP/1.1",
            "Host: 127.0.0.1\r\n",
            false,
        ),
        (
            "GET",
            "/qos/qos?vers=1&qtyp=1&prpt=3659",
            "HTTP/1.0",
            "Host: 127.0.0.1\r\n",
            false,
        ),
        (
            "GET",
            "/qos/qos?vers=1&qtyp=1&prpt=3659",
            "HTTP/1.1",
            "Host: 127.0.0.1\r\nContent-Length: 1\r\n",
            false,
        ),
        (
            "GET",
            "/qos/qos?vers=1&qtyp=1&prpt=3659",
            "HTTP/1.1",
            "Host: 127.0.0.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n",
            false,
        ),
        (
            "GET",
            "/qos/qos?vers=1&qtyp=1&prpt=3659",
            "HTTP/1.1",
            "Host: 127.0.0.1\r\nTransfer-Encoding: chunked\r\n",
            false,
        ),
        (
            "GET",
            "/qos/qos?vers=1&qtyp=1&prpt=3659",
            "HTTP/1.1",
            "",
            false,
        ),
        (
            "GET",
            "/qos/qos?vers=1&qtyp=1&prpt=3659",
            "HTTP/1.1",
            "Host: 127.0.0.1\r\nHost: other.invalid\r\n",
            false,
        ),
    ] {
        let id = identity();
        let context = client_context(Some(id.ca_pem()), AES256);
        let (server, socket) = pair();
        let handle = thread::spawn(move || {
            nfs_server_support::qos_tls::serve_latency(
                server,
                Instant::now() + LIMIT,
                &id,
                endpoint,
            )
        });
        let mut tls = client(socket, &context, Ipv4Addr::LOCALHOST);
        tls.connect().unwrap();
        let request = format!("{method} {target} {version}\r\n{headers}\r\n");
        tls.write_all(request.as_bytes()).unwrap();
        let mut response = Vec::new();
        let _ = tls.read_to_end(&mut response);
        let observed = handle.join().unwrap();
        assert_eq!(observed.http_header, request.as_bytes());
        assert_eq!(observed.http_response_bytes, response.len());
        assert_eq!(observed.http_response, response);
        if accepted {
            assert_eq!(observed.status, Stop::ResponseComplete);
            let response = String::from_utf8(response).unwrap();
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
            assert_eq!(
                response.split("\r\n\r\n").nth(1).unwrap(),
                format!(
                    "<qos><qosip>2130706433</qosip><qosport>{}</qosport></qos>",
                    endpoint.port()
                )
            );
        } else {
            assert_eq!(observed.status, Stop::UnsupportedRequest);
            assert!(response.is_empty());
        }
    }
}

#[test]
fn fresh_ca_and_leaf_constraints_support_verified_ip_tls12() {
    let id = Arc::new(Identity::new().unwrap());
    let ca = X509::from_pem(id.ca_pem()).unwrap();
    assert_eq!(ca.public_key().unwrap().bits(), 2048);
    assert_eq!(ca.pathlen(), Some(0));
    assert!(ca.verify(&ca.public_key().unwrap()).unwrap());
    assert_eq!(
        ca.signature_algorithm().object().nid(),
        Nid::SHA256WITHRSAENCRYPTION
    );
    let context = client_context(Some(id.ca_pem()), AES256);
    let (server, socket) = pair();
    let handle = thread::spawn(move || capture(server, Instant::now() + LIMIT, &id));
    let mut client = client(socket, &context, Ipv4Addr::LOCALHOST);
    let established = client.connect().is_ok();
    let cert = client.ssl().peer_certificate();
    let verify = client.ssl().verify_result();
    let ems = client.ssl().extms_support();
    if established {
        let _ = client.write_all(b"GET /qos HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    }
    drop(client);
    let result = handle.join().unwrap();
    assert!(established);
    assert_eq!(verify, X509VerifyResult::OK);
    assert_eq!(ems, Some(false));
    let cert = cert.unwrap();
    assert_eq!(cert.public_key().unwrap().bits(), 2048);
    assert_eq!(
        cert.signature_algorithm().object().nid(),
        Nid::SHA256WITHRSAENCRYPTION
    );
    assert_eq!(ca.issued(&cert), X509VerifyResult::OK);
    assert!(cert.verify(&ca.public_key().unwrap()).unwrap());
    assert_eq!(
        cert.subject_alt_names()
            .unwrap()
            .get(0)
            .unwrap()
            .ipaddress(),
        Some(&[127, 0, 0, 1][..])
    );
    assert_eq!(result.status, Stop::HeaderComplete);
    assert!(result.tls_established);
    assert_eq!(result.protocol, Some("TLSv1.2"));
    assert_eq!(result.cipher, Some(AES256));
}

#[test]
fn observed_header_excludes_body_and_does_not_send_http_response() {
    let header =
        b"POST /latency?synthetic=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 8\r\n\r\n";
    let request = [header.as_slice(), b"body1234"].concat();
    let result = run_request(identity(), AES128, &request, request.len());
    assert_eq!(result.status, Stop::HeaderComplete);
    assert_eq!(result.http_header, header);
    assert_eq!(result.cipher, Some(AES128));
    assert!(result.tls_error.is_none());
    assert!(!result.server_wire.is_empty());
    assert!(result.client_wire.len() > result.http_header.len());
}

fn rejected(ca: Option<&[u8]>, cipher: &str, ip: Ipv4Addr) -> Observation {
    let id = identity();
    let context = client_context(ca, cipher);
    let (server, socket) = pair();
    let handle = thread::spawn(move || capture(server, Instant::now() + LIMIT, &id));
    let mut client = client(socket, &context, ip);
    let failed = client.connect().is_err();
    let result = handle.join().unwrap();
    drop(client);
    assert!(failed);
    assert!(!result.tls_established);
    assert_eq!(result.status, Stop::TlsFailure);
    assert!(result.http_header.is_empty());
    assert!(result.tls_error.is_some());
    result
}

#[test]
fn unknown_ca_rejection_retains_only_numeric_tls_error_and_alert_shape() {
    let result = rejected(None, AES256, Ipv4Addr::LOCALHOST);
    let error = result.tls_error.unwrap();
    assert_eq!(error.code, 1); // SSL_ERROR_SSL, from the checked OpenSSL header.
    assert_eq!(error.client_alert.unwrap().level, 2);
    assert!(error.reason.is_some());
}

#[test]
fn wrong_ip_and_nonoverlapping_suite_are_rejected() {
    let id = identity();
    rejected(Some(id.ca_pem()), AES256, Ipv4Addr::new(127, 0, 0, 2));
    let result = rejected(
        Some(id.ca_pem()),
        "ECDHE-RSA-AES128-GCM-SHA256",
        Ipv4Addr::LOCALHOST,
    );
    assert_eq!(result.tls_error.unwrap().server_alert.unwrap().level, 2);
}

fn vec16(bytes: &[u8]) -> Vec<u8> {
    [
        u16::try_from(bytes.len()).unwrap().to_be_bytes().as_slice(),
        bytes,
    ]
    .concat()
}

fn synthetic_hello() -> Vec<u8> {
    let suites: Vec<u8> = [
        0x009d_u16, 0x009c, 0x003d, 0x003c, 0x0035, 0x002f, 0x0005, 0x0004,
    ]
    .iter()
    .flat_map(|n| n.to_be_bytes())
    .collect();
    let sni = vec16(&[vec![0], vec16(b"127.0.0.1")].concat());
    let sigalgs = vec16(&[4, 1, 2, 1, 1, 1]);
    let extensions = [vec![0, 0], vec16(&sni), vec![0, 13], vec16(&sigalgs)].concat();
    let body = [
        vec![3, 3],
        vec![0x42; 32],
        vec![0],
        vec16(&suites),
        vec![1, 0],
        vec16(&extensions),
    ]
    .concat();
    let length = u32::try_from(body.len()).unwrap().to_be_bytes();
    let handshake = [vec![1], length[1..].to_vec(), body].concat();
    [vec![22, 3, 0], vec16(&handshake)].concat()
}

#[test]
fn synthetic_offer_selects_tls12_rsa_gcm_without_claiming_handshake_completion() {
    let id = identity();
    let (server, socket) = pair();
    let handle = thread::spawn(move || capture(server, Instant::now() + LIMIT, &id));
    let mut client = DeadlineIo::new(socket);
    let hello = synthetic_hello();
    client.write_all(&hello).unwrap();
    let mut header = [0; 5];
    client.read_exact(&mut header).unwrap();
    let size = usize::from(u16::from_be_bytes([header[3], header[4]]));
    assert!(size < 16_384);
    let mut record = vec![0; size];
    client.read_exact(&mut record).unwrap();
    client.socket.shutdown(Shutdown::Both).unwrap();
    drop(client);
    let result = handle.join().unwrap();
    assert_eq!(header[0], 22);
    assert_eq!(record[0], 2);
    assert_eq!(&record[4..6], &[3, 3]);
    let suite = 39 + usize::from(record[38]);
    assert!([&[0, 0x9d][..], &[0, 0x9c][..]].contains(&&record[suite..suite + 2]));
    assert_eq!(result.client_wire, hello);
    assert!(
        result
            .server_wire
            .starts_with(&[header.as_slice(), record.as_slice()].concat())
    );
    assert!(!result.tls_established);
    assert!(result.http_header.is_empty());
}

#[test]
fn malformed_and_oversized_tls_records_fail_before_payload_or_deadline() {
    for prefix in [
        &b"not TLS"[..],
        &[22, 3, 3, 0xff, 0xff][..],
        &[21, 4, 0, 0, 2, 2, 40][..],
    ] {
        let id = identity();
        let (server, mut client) = pair();
        client.set_write_timeout(Some(LIMIT)).unwrap();
        client.write_all(prefix).unwrap();
        let start = Instant::now();
        let result = capture(server, start + LIMIT, &id);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(result.status, Stop::TlsFailure);
        assert!(!result.tls_established);
        assert!(result.client_wire.len() <= prefix.len());
        assert!(result.tls_error.unwrap().client_alert.is_none());
    }
}

#[test]
fn expired_and_silent_peer_deadlines_are_absolute_and_release_the_socket() {
    let id = identity();
    for duration in [Duration::ZERO, Duration::from_millis(150)] {
        let (server, mut client) = pair();
        let start = Instant::now();
        let result = capture(server, start + duration, &id);
        assert_eq!(result.status, Stop::Deadline);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(result.client_wire.is_empty());
        assert!(result.server_wire.is_empty());
        client.set_read_timeout(Some(LIMIT)).unwrap();
        let mut byte = [0];
        assert!(matches!(client.read(&mut byte), Ok(0) | Err(_)));
    }
}

#[test]
fn slowly_dripped_record_cannot_renew_the_operation_deadline() {
    let id = identity();
    let (server, mut client) = pair();
    client.set_write_timeout(Some(LIMIT)).unwrap();
    let writer = thread::spawn(move || {
        for byte in synthetic_hello() {
            if client.write_all(&[byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    let start = Instant::now();
    let result = capture(server, start + Duration::from_millis(150), &id);
    let elapsed = start.elapsed();
    writer.join().unwrap();
    assert_eq!(result.status, Stop::Deadline);
    assert!(elapsed < Duration::from_millis(800));
    assert!(!result.client_wire.is_empty());
    assert!(result.client_wire.len() < synthetic_hello().len());
}

#[test]
fn established_tls_keeps_the_same_deadline_while_waiting_for_header() {
    let id = identity();
    let context = client_context(Some(id.ca_pem()), AES256);
    let (server, socket) = pair();
    let start = Instant::now();
    let handle = thread::spawn(move || capture(server, start + Duration::from_millis(300), &id));
    let mut client = client(socket, &context, Ipv4Addr::LOCALHOST);
    let established = client.connect().is_ok();
    if established {
        let _ = client.write_all(b"GET /partial");
        let mut byte = [0];
        let _ = client.read(&mut byte);
    }
    drop(client);
    let result = handle.join().unwrap();
    assert!(established);
    assert!(result.tls_established);
    assert_eq!(result.status, Stop::Deadline);
    assert_eq!(result.http_header, b"GET /partial");
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn decrypted_header_bound_and_http_syntax_are_enforced() {
    let result = run_request(
        identity(),
        AES256,
        &vec![b'A'; MAX_HTTP_HEADER_BYTES + 100],
        1024,
    );
    assert_eq!(result.status, Stop::HeaderLimit);
    assert_eq!(result.http_header.len(), MAX_HTTP_HEADER_BYTES);
    assert!(result.tls_established);
    let malformed = run_request(identity(), AES256, b"GET / HTTP/1.1\nHost: x\r\n\r\n", 128);
    assert_eq!(malformed.status, Stop::InvalidHttp);
    assert!(malformed.tls_established);
}

#[test]
fn many_small_encrypted_records_hit_combined_wire_cap_before_plaintext_cap() {
    let result = run_request(identity(), AES256, &vec![b'A'; 4000], 1);
    assert_eq!(result.status, Stop::WireLimit);
    assert_eq!(
        result.client_wire.len() + result.server_wire.len(),
        MAX_WIRE_BYTES
    );
    assert!(result.http_header.len() < MAX_HTTP_HEADER_BYTES);
    assert!(result.tls_established);
}

#[test]
fn concurrent_identities_and_request_buffers_are_isolated() {
    let first = Arc::new(Identity::new().unwrap());
    let second = Arc::new(Identity::new().unwrap());
    assert_ne!(first.ca_pem(), second.ca_pem());
    let first_key = X509::from_pem(first.ca_pem())
        .unwrap()
        .public_key()
        .unwrap();
    let second_key = X509::from_pem(second.ca_pem())
        .unwrap()
        .public_key()
        .unwrap();
    assert!(!first_key.public_eq(&second_key));
    let a = thread::spawn(move || {
        run_request(
            first,
            AES256,
            b"GET /first HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            5,
        )
    });
    let b = thread::spawn(move || {
        run_request(
            second,
            AES128,
            b"GET /second HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            7,
        )
    });
    let a = a.join().unwrap();
    let b = b.join().unwrap();
    assert_eq!(a.status, Stop::HeaderComplete);
    assert_eq!(b.status, Stop::HeaderComplete);
    assert_eq!(a.cipher, Some(AES256));
    assert_eq!(b.cipher, Some(AES128));
    assert_eq!(
        a.http_header,
        b"GET /first HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
    );
    assert_eq!(
        b.http_header,
        b"GET /second HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
    );
    assert_ne!(a.client_wire, b.client_wire);
}
