// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Optional loopback LSX edge using the backend's current account identity.
use crate::{
    Failure,
    seeds::{OsSeeds, fresh},
};
use nfs_lsx::{
    message::Profile,
    session::{Config, Event, Launcher, Phase},
};
use std::{net::Ipv4Addr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    task::JoinSet,
};
use tracing::{info, warn};

pub fn profile(identity: &nfs_services::authentication::Identity) -> Profile {
    Profile {
        user_id: identity.account_id() as u64,
        persona_id: identity.persona_id() as u64,
        persona: identity.name().to_owned(),
        avatar_id: String::new(),
    }
}

pub async fn bind(port: u16) -> Result<TcpListener, Failure> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| Failure::Io)
}

pub async fn serve(
    listener: TcpListener,
    profile: Profile,
    idle: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Failure> {
    let config = Config::for_profile(profile);
    let mut tasks = JoinSet::new();
    let result = loop {
        if *shutdown.borrow() {
            break Ok(());
        }
        tokio::select! {
            _ = shutdown.changed() => { break Ok(()); }
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if result.is_err() { break Err(Failure::Io); }
            }
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(value) => value,
                    Err(_) => break Err(Failure::Io),
                };
                if !peer.ip().is_loopback() || tasks.len() >= 4 { continue; }
                let entropy = match fresh::<48>(&mut OsSeeds) {
                    Ok(value) => value,
                    Err(error) => break Err(error),
                };
                let config = config.clone();
                tasks.spawn(async move { connection(stream, config, entropy, idle).await; });
            }
        }
    };
    tasks.shutdown().await;
    result
}

struct Entropy {
    bytes: [u8; 48],
    offset: usize,
}
impl nfs_lsx::session::Random for Entropy {
    fn fill(&mut self, bytes: &mut [u8]) {
        bytes.copy_from_slice(&self.bytes[self.offset..self.offset + bytes.len()]);
        self.offset += bytes.len();
    }
}

async fn connection(mut stream: TcpStream, config: Config, entropy: [u8; 48], idle: Duration) {
    let (mut launcher, first) = Launcher::new(
        config,
        &mut Entropy {
            bytes: entropy,
            offset: 0,
        },
    );
    if write(&mut stream, &first.send).await.is_err() {
        return;
    }
    let mut buffer = [0; 8192];
    let mut deadline = Some(tokio::time::Instant::now() + idle);
    loop {
        // Established SDK connections have no periodic heartbeat. Bound the
        // handshake and each partial frame, not the time between requests.
        let read = match deadline {
            Some(end) => tokio::time::timeout_at(end, stream.read(&mut buffer))
                .await
                .unwrap_or_else(|_| Err(std::io::ErrorKind::TimedOut.into())),
            None => stream.read(&mut buffer).await,
        };
        let count = match read {
            Ok(n) if n != 0 => n,
            _ => break,
        };
        let output = launcher.receive(&buffer[..count]);
        if launcher.phase() == Phase::Established {
            if buffer[count - 1] == 0 {
                deadline = None;
            } else if deadline.is_none() || buffer[..count].contains(&0) {
                deadline = Some(tokio::time::Instant::now() + idle);
            }
        }
        for event in &output.events {
            match event {
                Event::HandshakeAccepted { .. } => info!("LSX handshake accepted"),
                Event::Answered { request, id } => info!(request, id, "LSX answered"),
                Event::Unsupported { name, id } => warn!(name, id, "LSX unsupported"),
                Event::Malformed(_) | Event::ChallengeMismatch => warn!("LSX rejected input"),
                _ => {}
            }
        }
        if write(&mut stream, &output.send).await.is_err() || launcher.phase() == Phase::Rejected {
            break;
        }
    }
    info!(phase=?launcher.phase(), stats=?launcher.stats(), "LSX disconnected");
}

async fn write(stream: &mut TcpStream, frames: &[Vec<u8>]) -> std::io::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        for frame in frames {
            stream.write_all(frame).await?;
        }
        Ok(())
    })
    .await
    .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_lsx::{
        cipher::{self, Aes128},
        frame,
        message::{Envelope, Kind},
        xml::Element,
    };

    async fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut bytes = Vec::new();
            loop {
                let byte = stream.read_u8().await.unwrap();
                if byte == 0 {
                    return bytes;
                }
                bytes.push(byte);
                assert!(bytes.len() < frame::MAX_FRAME);
            }
        })
        .await
        .unwrap()
    }
    async fn connect(address: std::net::SocketAddr) -> (TcpStream, Aes128) {
        let mut stream = TcpStream::connect(address).await.unwrap();
        let hello = Envelope::parse(&read_frame(&mut stream).await).unwrap();
        let key = cipher::lower_hex(&[7; 16]);
        let zero = Aes128::parameter_zero();
        let response = zero
            .encode_hex(hello.payload.attr("key").unwrap().as_bytes())
            .unwrap();
        let request = Envelope {
            kind: Kind::Request,
            sender: None,
            recipient: Some("EALS".into()),
            id: 1,
            payload: Element::new("ChallengeResponse")
                .attribute("response", &response)
                .attribute("key", &key)
                .child(Element::new("ContentId").text("test"))
                .child(Element::new("Title").text("test"))
                .child(Element::new("MultiplayerId").text("test"))
                .child(Element::new("Language").text("en_US"))
                .child(Element::new("Version").text("9.10.1.7")),
        };
        stream
            .write_all(&frame::encode(request.to_xml().as_bytes()).unwrap())
            .await
            .unwrap();
        let accepted = Envelope::parse(&read_frame(&mut stream).await).unwrap();
        let cipher = Aes128::for_parameter(cipher::transport_parameter(
            accepted.payload.attr("response").unwrap().as_bytes(),
        ));
        (stream, cipher)
    }
    async fn request(
        stream: &mut TcpStream,
        cipher: &Aes128,
        payload: Element,
        id: u64,
    ) -> Envelope {
        let request = Envelope {
            kind: Kind::Request,
            sender: None,
            recipient: Some("EbisuSDK".into()),
            id,
            payload,
        };
        let encoded = cipher.encode_hex(request.to_xml().as_bytes()).unwrap();
        // A real split read must preserve framing and transport state.
        let wire = frame::encode(encoded.as_bytes()).unwrap();
        stream.write_all(&wire[..3]).await.unwrap();
        stream.write_all(&wire[3..]).await.unwrap();
        Envelope::parse(
            &cipher
                .decode_hex(&read_frame(stream).await)
                .unwrap()
                .visible,
        )
        .unwrap()
    }
    #[tokio::test]
    async fn loopback_uses_account_identity_and_shutdown_closes_active_clients() {
        let account = nfs_storage::AccountId::from_owned_config([9; 16]).unwrap();
        let identity =
            nfs_services::authentication::Identity::new(account, 101, 202, "Local driver".into())
                .unwrap();
        let listener = bind(0).await.unwrap();
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        let (stop, shutdown) = watch::channel(false);
        let server = tokio::spawn(serve(
            listener,
            profile(&identity),
            Duration::from_secs(2),
            shutdown,
        ));
        let (mut stream, cipher) = connect(address).await;
        let reply = request(
            &mut stream,
            &cipher,
            Element::new("GetProfile").attribute("index", "0"),
            2,
        )
        .await;
        assert_eq!(reply.payload.attr("UserId"), Some("202"));
        assert_eq!(reply.payload.attr("PersonaId"), Some("101"));
        assert_eq!(reply.payload.attr("Persona"), Some("Local driver"));
        let auth = || {
            Element::new("GetAuthCode")
                .attribute("ClientId", "local")
                .attribute("Scope", "")
        };
        let first = request(&mut stream, &cipher, auth(), 3).await;
        let (mut second, cipher2) = connect(address).await;
        let second_code = request(&mut second, &cipher2, auth(), 2).await;
        assert_ne!(
            first.payload.attr("value"),
            second_code.payload.attr("value")
        );
        assert_eq!(first.payload.attr("value").unwrap().len(), 64);
        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), stream.read_u8())
                .await
                .unwrap()
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        assert!(TcpListener::bind(address).await.is_ok());
    }
    #[tokio::test]
    async fn connection_limit_rejects_excess_clients_and_recovers_closed_slots() {
        let listener = bind(0).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, shutdown) = watch::channel(false);
        let server = tokio::spawn(serve(
            listener,
            Profile {
                user_id: 1,
                persona_id: 2,
                persona: "Test".into(),
                avatar_id: String::new(),
            },
            Duration::from_secs(30),
            shutdown,
        ));
        let mut clients = Vec::new();
        for _ in 0..4 {
            let mut stream = TcpStream::connect(address).await.unwrap();
            let hello = Envelope::parse(&read_frame(&mut stream).await).unwrap();
            assert_eq!(hello.payload.name, "Challenge");
            clients.push(stream);
        }
        let mut excess = TcpStream::connect(address).await.unwrap();
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), excess.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        drop(clients.pop().unwrap());
        // Accept and task completion are independently scheduled. A connection
        // may still be refused until the closed client's task has been reaped.
        let mut replacement = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let mut candidate = TcpStream::connect(address).await.unwrap();
                if candidate.read(&mut byte).await.unwrap() == 1 {
                    assert_eq!(byte, [b'<']);
                    break candidate;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut greeting = vec![b'<'];
        greeting.extend(read_frame(&mut replacement).await);
        assert_eq!(
            Envelope::parse(&greeting).unwrap().payload.name,
            "Challenge"
        );
        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn established_idle_survives_but_partial_frame_has_a_deadline() {
        let listener = bind(0).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, shutdown) = watch::channel(false);
        let server = tokio::spawn(serve(
            listener,
            Profile {
                user_id: 1,
                persona_id: 2,
                persona: "Test".into(),
                avatar_id: String::new(),
            },
            Duration::from_millis(200),
            shutdown,
        ));
        let (mut stream, cipher) = connect(address).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        let reply = request(
            &mut stream,
            &cipher,
            Element::new("GetProfile").attribute("index", "0"),
            2,
        )
        .await;
        assert_eq!(reply.payload.attr("Persona"), Some("Test"));
        stream.write_all(b"aa").await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), stream.read_u8())
                .await
                .unwrap()
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn idle_incomplete_handshake_is_closed() {
        let listener = bind(0).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, shutdown) = watch::channel(false);
        let server = tokio::spawn(serve(
            listener,
            Profile {
                user_id: 1,
                persona_id: 2,
                persona: "Test".into(),
                avatar_id: String::new(),
            },
            Duration::from_millis(30),
            shutdown,
        ));
        let mut stream = TcpStream::connect(address).await.unwrap();
        let _ = read_frame(&mut stream).await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), stream.read_u8())
                .await
                .unwrap()
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
