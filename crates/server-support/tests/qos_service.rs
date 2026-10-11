// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::{
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use nfs_server_support::{
    qos_service::{MAX_CONNECTIONS, MAX_DATAGRAM_BYTES, MAX_DATAGRAMS, Observation, Stop, run},
    qos_tls::{Identity, Stop as TlsStop},
};
use openssl::{
    ssl::{Ssl, SslContextBuilder, SslMethod, SslStream, SslVerifyMode, SslVersion},
    x509::{X509, X509VerifyResult},
};

const CLIENT_LIMIT: Duration = Duration::from_secs(4);
const SERVICE_LIMIT: Duration = Duration::from_secs(2);
const REQUEST: &[u8] = b"GET /qos/qos?vers=1&qtyp=1&prpt=1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";

struct Service {
    tls: SocketAddr,
    udp: SocketAddr,
    identity: Arc<Identity>,
    worker: JoinHandle<Observation>,
}

fn service(limit: Duration) -> Service {
    let identity = Arc::new(Identity::new().unwrap());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let tls_addr = listener.local_addr().unwrap();
    let udp_addr = udp.local_addr().unwrap();
    let id = Arc::clone(&identity);
    let deadline = Instant::now() + limit;
    Service {
        tls: tls_addr,
        udp: udp_addr,
        identity,
        worker: thread::spawn(move || run(listener, udp, deadline, &id)),
    }
}

struct DeadlineIo {
    socket: TcpStream,
    deadline: Instant,
}

impl DeadlineIo {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
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

fn client(address: SocketAddr, identity: &Identity) -> SslStream<DeadlineIo> {
    let socket = TcpStream::connect_timeout(&address, CLIENT_LIMIT).unwrap();
    let mut context = SslContextBuilder::new(SslMethod::tls_client()).unwrap();
    context
        .set_min_proto_version(Some(SslVersion::TLS1_2))
        .unwrap();
    context
        .set_max_proto_version(Some(SslVersion::TLS1_2))
        .unwrap();
    context.set_cipher_list("AES256-GCM-SHA384").unwrap();
    context.set_security_level(2);
    context.set_verify(SslVerifyMode::PEER);
    context
        .cert_store_mut()
        .add_cert(X509::from_pem(identity.ca_pem()).unwrap())
        .unwrap();
    let mut ssl = Ssl::new(&context.build()).unwrap();
    ssl.param_mut()
        .set_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
        .unwrap();
    let mut client = SslStream::new(
        ssl,
        DeadlineIo {
            socket,
            deadline: Instant::now() + CLIENT_LIMIT,
        },
    )
    .unwrap();
    client.connect().unwrap();
    assert_eq!(client.ssl().verify_result(), X509VerifyResult::OK);
    client
}

fn http_request(address: SocketAddr, identity: &Identity, request: &[u8]) -> Vec<u8> {
    let mut client = client(address, identity);
    client.write_all(request).unwrap();
    let mut response = Vec::new();
    let mut bytes = [0; 512];
    while let Ok(size) = client.read(&mut bytes) {
        if size == 0 {
            break;
        }
        response.extend_from_slice(&bytes[..size]);
        assert!(response.len() <= 1024);
        if let Some(header_end) = response.windows(4).position(|s| s == b"\r\n\r\n") {
            let header_end = header_end + 4;
            let header = std::str::from_utf8(&response[..header_end]).unwrap();
            let body_size: usize = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            if response.len() >= header_end + body_size {
                assert_eq!(response.len(), header_end + body_size);
                break;
            }
        }
    }
    response
}

fn assert_http(response: &[u8], port: u16) {
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    let end = response.windows(4).position(|s| s == b"\r\n\r\n").unwrap() + 4;
    assert_eq!(
        &response[end..],
        format!("<qos><qosip>2130706433</qosip><qosport>{port}</qosport></qos>").as_bytes()
    );
}

fn probe(id: u32) -> [u8; 20] {
    let mut bytes = [0; 20];
    bytes[..4].copy_from_slice(&id.to_be_bytes());
    bytes[7] = 1;
    bytes[16..].copy_from_slice(&(0x1234_0000 | id).to_be_bytes());
    bytes
}

fn udp_client() -> UdpSocket {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.set_read_timeout(Some(CLIENT_LIMIT)).unwrap();
    socket.set_write_timeout(Some(CLIENT_LIMIT)).unwrap();
    socket
}

fn exchange(socket: &UdpSocket, service: SocketAddr, request: &[u8; 20]) -> Vec<u8> {
    assert_eq!(socket.send_to(request, service).unwrap(), request.len());
    let mut bytes = [0; 64];
    let (size, peer) = socket.recv_from(&mut bytes).unwrap();
    assert_eq!(peer, service);
    assert_eq!(size, 30);
    assert_eq!(&bytes[..20], request);
    assert_eq!(&bytes[20..24], &[127, 0, 0, 1]);
    assert_eq!(
        &bytes[24..26],
        &socket.local_addr().unwrap().port().to_be_bytes()
    );
    assert_eq!(&bytes[26..30], &[0; 4]);
    bytes[..size].to_vec()
}

fn assert_cleanup(tls: SocketAddr, udp: SocketAddr) {
    assert!(TcpStream::connect_timeout(&tls, Duration::from_millis(150)).is_err());
    let rebound = UdpSocket::bind(udp).unwrap();
    assert_eq!(rebound.local_addr().unwrap(), udp);
}

#[test]
fn four_verified_tls_requests_and_forty_latency_probes_share_owned_endpoint() {
    let service = service(SERVICE_LIMIT);
    let mut clients = Vec::new();
    for _ in 0..4 {
        let id = Arc::clone(&service.identity);
        let address = service.tls;
        clients.push(thread::spawn(move || http_request(address, &id, REQUEST)));
    }
    let socket = udp_client();
    assert_ne!(socket.local_addr().unwrap().port(), 1);
    let mut sent = Vec::new();
    for id in 1..=40 {
        let request = probe(id);
        let response = exchange(&socket, service.udp, &request);
        sent.push((request, response));
    }
    let responses: Vec<_> = clients
        .into_iter()
        .map(|client| client.join().unwrap())
        .collect();
    for response in &responses {
        assert_http(response, service.udp.port());
    }
    let result = service.worker.join().unwrap();
    assert_eq!(result.status, Stop::ConnectionLimit);
    assert_eq!(result.accepted_connections, 4);
    assert_eq!(result.connections.len(), 4);
    assert_eq!(result.rejected_connections, 0);
    assert_eq!(result.connection_errors, 0);
    for connection in &result.connections {
        assert_eq!(connection.status, TlsStop::ResponseComplete);
        assert!(connection.tls_established);
        assert_eq!(connection.http_header, REQUEST);
        assert_eq!(
            connection.http_response_bytes,
            connection.http_response.len()
        );
        assert_http(&connection.http_response, service.udp.port());
        assert!(!connection.client_wire.is_empty());
        assert!(!connection.server_wire.is_empty());
    }
    assert_eq!(result.udp.status, Stop::Deadline);
    assert_eq!(result.udp.received_datagrams, 40);
    assert_eq!(result.udp.replies_sent, 40);
    assert_eq!(result.udp.invalid_datagrams, 0);
    assert_eq!(result.udp.datagrams.len(), 40);
    for (record, (request, response)) in result.udp.datagrams.iter().zip(sent) {
        assert_eq!(record.peer, socket.local_addr().unwrap());
        assert_eq!(record.request, request);
        assert_eq!(record.response, response);
    }
    assert_cleanup(service.tls, service.udp);
}

#[test]
fn invalid_and_oversized_datagrams_get_no_reply_and_do_not_poison_next_probe() {
    let service = service(SERVICE_LIMIT);
    let socket = udp_client();
    let mut wrong_type = probe(1);
    wrong_type[7] = 2;
    let mut reserved = probe(2);
    reserved[8] = 1;
    let invalid = [
        vec![0; 20],
        wrong_type.to_vec(),
        reserved.to_vec(),
        probe(3)[..19].to_vec(),
        [probe(4).as_slice(), &[0]].concat(),
        Vec::new(),
        vec![0; MAX_DATAGRAM_BYTES],
        vec![0; MAX_DATAGRAM_BYTES * 2],
    ];
    for request in &invalid {
        socket.send_to(request, service.udp).unwrap();
    }
    exchange(&socket, service.udp, &probe(99));
    socket
        .set_read_timeout(Some(Duration::from_millis(80)))
        .unwrap();
    assert!(socket.recv_from(&mut [0; 64]).is_err());
    let result = service.worker.join().unwrap();
    assert_eq!(result.udp.received_datagrams, 9);
    assert_eq!(result.udp.invalid_datagrams, 8);
    assert_eq!(result.udp.oversized_datagrams, 1);
    assert_eq!(result.udp.replies_sent, 1);
    assert!(result.udp.datagrams.len() <= 9);
    assert_eq!(result.udp.datagrams.last().unwrap().request, probe(99));
    assert!(result.udp.datagrams.iter().all(|record| {
        record.request.len() <= MAX_DATAGRAM_BYTES && record.response.len() <= 30
    }));
    assert_eq!(
        result
            .udp
            .datagrams
            .iter()
            .filter(|record| !record.response.is_empty())
            .count(),
        1
    );
    assert_cleanup(service.tls, service.udp);
}

#[test]
fn unsupported_http_targets_and_bodies_receive_no_success() {
    let service = service(SERVICE_LIMIT);
    let requests: [&[u8]; 4] = [
        b"GET /qos/qos?vers=1&qtyp=2&prpt=3659 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        b"GET /qos/firewall?vers=1&nint=2 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        b"GET /qos/qos?vers=1&qtyp=1&prpt=3659 HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 1\r\n\r\nx",
        b"GET /qos/qos?vers=1&qtyp=1&prpt=3659 HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
    ];
    for request in requests {
        assert!(http_request(service.tls, &service.identity, request).is_empty());
    }
    let result = service.worker.join().unwrap();
    assert_eq!(result.connections.len(), 4);
    for connection in result.connections {
        assert_eq!(connection.status, TlsStop::UnsupportedRequest);
        assert!(connection.http_response.is_empty());
        assert_eq!(connection.http_response_bytes, 0);
        assert!(connection.tls_established);
    }
    assert_eq!(result.udp.replies_sent, 0);
    assert_cleanup(service.tls, service.udp);
}

#[test]
fn slow_tls_does_not_block_other_tls_or_udp_and_all_workers_share_deadline() {
    let service = service(Duration::from_millis(900));
    let started = Instant::now();
    let slow = TcpStream::connect_timeout(&service.tls, CLIENT_LIMIT).unwrap();
    let mut partial = TcpStream::connect_timeout(&service.tls, CLIENT_LIMIT).unwrap();
    partial.write_all(&[22, 3, 3, 0, 80, 1]).unwrap();
    let response = http_request(service.tls, &service.identity, REQUEST);
    assert_http(&response, service.udp.port());
    exchange(&udp_client(), service.udp, &probe(1));
    let result = service.worker.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(result.accepted_connections, 3);
    assert_eq!(result.status, Stop::Deadline);
    assert_eq!(result.udp.replies_sent, 1);
    assert_eq!(result.connections[0].status, TlsStop::Deadline);
    assert_eq!(result.connections[1].status, TlsStop::Deadline);
    assert_eq!(result.connections[1].client_wire, [22, 3, 3, 0, 80, 1]);
    assert_eq!(result.connections[2].status, TlsStop::ResponseComplete);
    drop((slow, partial));
    assert_cleanup(service.tls, service.udp);
}

#[test]
fn idle_and_expired_deadlines_close_owned_sockets() {
    let service = service(Duration::from_millis(100));
    let started = Instant::now();
    let result = service.worker.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(result.status, Stop::Deadline);
    assert_eq!(result.udp.status, Stop::Deadline);
    assert_eq!(result.accepted_connections, 0);
    assert_eq!(result.udp.received_datagrams, 0);
    assert_cleanup(service.tls, service.udp);

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let udp = udp_client();
    let tls_addr = listener.local_addr().unwrap();
    let udp_addr = udp.local_addr().unwrap();
    let result = run(listener, udp, Instant::now(), &service.identity);
    assert_eq!(result.status, Stop::Deadline);
    assert_eq!(result.udp.status, Stop::Deadline);
    assert_cleanup(tls_addr, udp_addr);
}

#[test]
fn total_connection_and_datagram_limits_are_strict() {
    let service = service(SERVICE_LIMIT);
    let mut stalled = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        stalled.push(TcpStream::connect_timeout(&service.tls, CLIENT_LIMIT).unwrap());
    }
    let socket = udp_client();
    for id in 1..=MAX_DATAGRAMS {
        exchange(&socket, service.udp, &probe(u32::try_from(id).unwrap()));
    }
    if let Ok(mut fifth) = TcpStream::connect_timeout(&service.tls, Duration::from_millis(150)) {
        fifth
            .set_read_timeout(Some(Duration::from_millis(150)))
            .unwrap();
        assert!(matches!(fifth.read(&mut [0; 1]), Err(_) | Ok(0)));
    }
    socket.send_to(&probe(129), service.udp).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(80)))
        .unwrap();
    assert!(socket.recv_from(&mut [0; 64]).is_err());
    let result = service.worker.join().unwrap();
    assert_eq!(result.status, Stop::ConnectionLimit);
    assert_eq!(result.accepted_connections, MAX_CONNECTIONS);
    assert_eq!(result.connections.len(), MAX_CONNECTIONS);
    assert!(
        result
            .connections
            .iter()
            .all(|connection| connection.status == TlsStop::Deadline)
    );
    assert_eq!(result.udp.status, Stop::DatagramLimit);
    assert_eq!(result.udp.received_datagrams, MAX_DATAGRAMS);
    assert_eq!(result.udp.datagrams.len(), MAX_DATAGRAMS);
    assert_eq!(result.udp.replies_sent, MAX_DATAGRAMS);
    drop(stalled);
    assert_cleanup(service.tls, service.udp);
}

#[test]
fn oversized_receive_errors_consume_the_same_finite_datagram_budget() {
    let service = service(SERVICE_LIMIT);
    let socket = udp_client();
    let oversized = vec![0; MAX_DATAGRAM_BYTES * 2];
    for id in 1..=MAX_DATAGRAMS / 2 {
        socket.send_to(&oversized, service.udp).unwrap();
        exchange(&socket, service.udp, &probe(u32::try_from(id).unwrap()));
    }
    socket.send_to(&probe(999), service.udp).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(80)))
        .unwrap();
    assert!(socket.recv_from(&mut [0; 64]).is_err());
    let result = service.worker.join().unwrap();
    assert_eq!(result.udp.status, Stop::DatagramLimit);
    assert_eq!(result.udp.received_datagrams, MAX_DATAGRAMS);
    assert_eq!(result.udp.invalid_datagrams, MAX_DATAGRAMS / 2);
    assert_eq!(result.udp.oversized_datagrams, MAX_DATAGRAMS / 2);
    assert_eq!(result.udp.replies_sent, MAX_DATAGRAMS / 2);
    if cfg!(windows) {
        assert_eq!(result.udp.io_errors, MAX_DATAGRAMS / 2);
        assert_eq!(result.udp.datagrams.len(), MAX_DATAGRAMS / 2);
    }
    assert_cleanup(service.tls, service.udp);
}

#[test]
fn concurrent_runs_have_separate_cas_endpoints_and_observations() {
    let first = service(Duration::from_secs(3));
    let second = service(SERVICE_LIMIT);
    assert_ne!(first.identity.ca_pem(), second.identity.ca_pem());
    assert_ne!(first.tls, second.tls);
    assert_ne!(first.udp, second.udp);
    let first_socket = udp_client();
    let second_socket = udp_client();
    let first_id = Arc::clone(&first.identity);
    let second_id = Arc::clone(&second.identity);
    let first_tls = first.tls;
    let second_tls = second.tls;
    let first_client = thread::spawn(move || http_request(first_tls, &first_id, REQUEST));
    let second_client = thread::spawn(move || http_request(second_tls, &second_id, REQUEST));
    exchange(&first_socket, first.udp, &probe(11));
    exchange(&second_socket, second.udp, &probe(22));
    assert_http(&first_client.join().unwrap(), first.udp.port());
    assert_http(&second_client.join().unwrap(), second.udp.port());
    let first_result = first.worker.join().unwrap();
    let second_result = second.worker.join().unwrap();
    assert_eq!(first_result.udp.datagrams.len(), 1);
    assert_eq!(second_result.udp.datagrams.len(), 1);
    assert_eq!(first_result.udp.datagrams[0].request, probe(11));
    assert_eq!(second_result.udp.datagrams[0].request, probe(22));
    assert_eq!(
        first_result.udp.datagrams[0].peer,
        first_socket.local_addr().unwrap()
    );
    assert_eq!(
        second_result.udp.datagrams[0].peer,
        second_socket.local_addr().unwrap()
    );
    assert_cleanup(first.tls, first.udp);
    assert_cleanup(second.tls, second.udp);
}

#[test]
fn wildcard_owned_endpoints_are_rejected_before_any_worker_starts() {
    let identity = Identity::new().unwrap();
    for tls_wildcard in [true, false] {
        let listener = TcpListener::bind((
            if tls_wildcard {
                Ipv4Addr::UNSPECIFIED
            } else {
                Ipv4Addr::LOCALHOST
            },
            0,
        ))
        .unwrap();
        let udp = UdpSocket::bind((
            if tls_wildcard {
                Ipv4Addr::LOCALHOST
            } else {
                Ipv4Addr::UNSPECIFIED
            },
            0,
        ))
        .unwrap();
        let started = Instant::now();
        let result = run(listener, udp, Instant::now() + SERVICE_LIMIT, &identity);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(result.status, Stop::NonLoopback);
        assert_eq!(result.udp.status, Stop::NonLoopback);
        assert!(result.connections.is_empty());
        assert!(result.udp.datagrams.is_empty());
    }
}
