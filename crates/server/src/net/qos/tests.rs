// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_server_support::qos_tls::Identity;
use openssl::{
    ssl::{Ssl, SslContextBuilder, SslMethod, SslStream, SslVerifyMode, SslVersion},
    x509::{X509, X509VerifyResult},
};
use std::{
    io::{self, Read, Write},
    net::{IpAddr, TcpStream},
};
use tokio::time::{sleep, timeout};
const CLIENT_LIMIT: Duration = Duration::from_secs(4);
const REQUEST: &[u8] = b"GET /qos/qos?vers=1&qtyp=1&prpt=1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";

struct Service {
    tls: SocketAddr,
    udp: SocketAddr,
    identity: Arc<Identity>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), Failure>>,
}
fn service(request_timeout: Duration) -> Service {
    let tcp = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let udp = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let tls = tcp.local_addr().unwrap();
    let udp_addr = udp.local_addr().unwrap();
    let identity = Arc::new(Identity::new().unwrap());
    let (stop, shutdown) = watch::channel(false);
    let task = tokio::spawn(serve(tcp, udp, identity.clone(), request_timeout, shutdown));
    Service {
        tls,
        udp: udp_addr,
        identity,
        stop,
        task,
    }
}
async fn stop(s: Service) {
    s.stop.send(true).unwrap();
    timeout(Duration::from_secs(3), s.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(TcpStream::connect_timeout(&s.tls, Duration::from_millis(100)).is_err());
    std::net::UdpSocket::bind(s.udp).unwrap();
}
async fn request(s: &Service) {
    let id = s.identity.clone();
    let address = s.tls;
    let response = tokio::task::spawn_blocking(move || http_request(address, &id, REQUEST))
        .await
        .unwrap();
    assert_http(&response, s.udp.port());
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

#[tokio::test]
async fn idle_and_repeated_requests_do_not_consume_service_lifetime() {
    let s = service(Duration::from_secs(1));
    sleep(Duration::from_millis(1200)).await;
    for _ in 0..8 {
        request(&s).await;
    }
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    timeout(Duration::from_secs(6), async {
        for id in 1u32..=160 {
            // Malformed traffic also must not consume a lifetime quota.
            socket.send_to(b"invalid", s.udp).await.unwrap();
            let mut probe = [0u8; 20];
            probe[..4].copy_from_slice(&id.to_be_bytes());
            probe[7] = 1;
            probe[16..].copy_from_slice(&id.to_be_bytes());
            socket.send_to(&probe, s.udp).await.unwrap();
            let mut reply = [0; 64];
            let (size, peer) = socket.recv_from(&mut reply).await.unwrap();
            assert_eq!(peer, s.udp);
            assert_eq!(size, 30);
            assert_eq!(&reply[..20], &probe);
            assert_eq!(&reply[20..24], &[127, 0, 0, 1]);
            assert_eq!(
                &reply[24..26],
                &socket.local_addr().unwrap().port().to_be_bytes()
            );
        }
    })
    .await
    .unwrap();
    sleep(Duration::from_millis(1200)).await;
    request(&s).await;
    stop(s).await;
}

#[tokio::test]
async fn stalled_tls_requests_expire_and_do_not_block_udp_or_later_requests() {
    let s = service(Duration::from_millis(300));
    let mut stalled = Vec::new();
    for _ in 0..MAX_TLS {
        stalled.push(tokio::net::TcpStream::connect(s.tls).await.unwrap());
    }
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut probe = [0; 20];
    probe[3] = 1;
    probe[7] = 1;
    udp.send_to(&probe, s.udp).await.unwrap();
    let mut reply = [0; 64];
    let (length, _) = timeout(Duration::from_secs(1), udp.recv_from(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(length, 30);
    // Each accepted connection has its own deadline; idle listeners stay alive.
    for mut client in stalled {
        use tokio::io::AsyncReadExt;
        let mut byte = [0];
        let result = timeout(Duration::from_secs(3), client.read(&mut byte))
            .await
            .unwrap();
        assert!(matches!(result, Ok(0) | Err(_)));
    }
    request(&s).await;
    stop(s).await;
}

#[tokio::test]
async fn shutdown_interrupts_idle_and_incomplete_tls_without_waiting_for_deadline() {
    let s = service(Duration::from_secs(30));
    let mut clients = Vec::new();
    for _ in 0..MAX_TLS {
        clients.push(tokio::net::TcpStream::connect(s.tls).await.unwrap());
    }
    // Give the accept loop a turn; shutdown must interrupt any blocking read.
    sleep(Duration::from_millis(50)).await;
    stop(s).await;
    drop(clients);
    let idle = service(Duration::from_secs(30));
    stop(idle).await;
}
