// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A bounded local TCP relay, for playing on this computer against a
//! server on any address (#172).
//!
//! The game's backend host is a 10-byte field ([`crate::endpoint`]): it
//! holds `127.0.0.1` but not a typical network address. So the game is
//! routed to `127.0.0.1:<port>` of a relay, which forwards each connection
//! unchanged to the server: one relay for the redirector and one for
//! launcher services. The server's own answers then give the game the
//! addresses it listens on. A relay serves only this computer's programs,
//! holds at most [`MAX_CONNECTIONS`] connections, and stops listening when
//! dropped; connections already relayed last until either side closes.

use crate::exit::{Failure, Result};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

/// Most connections relayed at once; more are closed as they arrive.
pub const MAX_CONNECTIONS: usize = 8;
/// How long reaching the server may take for one connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Bytes copied at a time in each direction.
const BUFFER: usize = 16 * 1024;

/// A running relay from `127.0.0.1` to one server address.
pub struct Relay {
    address: SocketAddrV4,
    target: SocketAddr,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl Relay {
    /// Listen on `127.0.0.1:port` (0: the system picks) and relay each
    /// connection from this computer to `target`.
    pub fn start(port: u16, target: SocketAddr) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let SocketAddr::V4(address) = listener.local_addr()? else {
            return Err(Failure::internal("relay listener is not IPv4"));
        };
        let stop = Arc::new(AtomicBool::new(false));
        let accept = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("relay-accept".into())
                .spawn(move || accept_loop(&listener, target, &stop))?
        };
        Ok(Self {
            address,
            target,
            stop,
            accept: Some(accept),
        })
    }

    /// Where the game is routed: `127.0.0.1:<port>`.
    pub fn address(&self) -> SocketAddrV4 {
        self.address
    }

    /// Where connections go.
    pub fn target(&self) -> SocketAddr {
        self.target
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept so it sees the flag.
        let _ = TcpStream::connect_timeout(&self.address.into(), Duration::from_secs(1));
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

/// Decrements the count of relayed connections when dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn accept_loop(listener: &TcpListener, target: SocketAddr, stop: &AtomicBool) {
    let active = Arc::new(AtomicUsize::new(0));
    for client in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(client) = client else {
            // A failed accept (for example, too many open files) must not spin.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        if !client.peer_addr().is_ok_and(|peer| peer.ip().is_loopback()) {
            continue;
        }
        let taken = active.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            (n < MAX_CONNECTIONS).then_some(n + 1)
        });
        if taken.is_err() {
            continue;
        }
        let slot = Slot(Arc::clone(&active));
        let spawned = std::thread::Builder::new()
            .name("relay-connection".into())
            .spawn(move || {
                let _slot = slot;
                relay(client, target);
            });
        drop(spawned);
    }
}

/// Relay one connection both ways until either side closes.
fn relay(client: TcpStream, target: SocketAddr) {
    let Ok(server) = TcpStream::connect_timeout(&target, CONNECT_TIMEOUT) else {
        return;
    };
    let _ = client.set_nodelay(true);
    let _ = server.set_nodelay(true);
    let (Ok(client_reader), Ok(server_reader)) = (client.try_clone(), server.try_clone()) else {
        return;
    };
    let upstream = std::thread::Builder::new()
        .name("relay-upstream".into())
        .spawn(move || pipe(client_reader, server));
    pipe(server_reader, client);
    if let Ok(upstream) = upstream {
        let _ = upstream.join();
    }
}

/// Copy `from` to `to` until `from` ends or `to` fails, then end writing
/// to `to`, so the other side sees the end too.
fn pipe(mut from: TcpStream, mut to: TcpStream) {
    let mut buffer = vec![0u8; BUFFER];
    loop {
        match from.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to.write_all(&buffer[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = to.shutdown(Shutdown::Write);
    let _ = from.shutdown(Shutdown::Read);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that echoes every connection until it closes.
    fn echo_server() -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut reader = stream.try_clone().unwrap();
                    let mut writer = stream;
                    let _ = std::io::copy(&mut reader, &mut writer);
                    let _ = writer.shutdown(Shutdown::Write);
                });
            }
        });
        address
    }

    fn connect(relay: &Relay) -> TcpStream {
        let stream = TcpStream::connect(relay.address()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
    }

    #[test]
    fn bytes_go_both_ways_unchanged_until_a_side_closes() {
        let relay = Relay::start(0, echo_server()).unwrap();
        assert_eq!(*relay.address().ip(), Ipv4Addr::LOCALHOST);
        let mut client = connect(&relay);
        let message: Vec<u8> = (0..=255u8).cycle().take(100_000).collect();
        for piece in message.chunks(7_000) {
            client.write_all(piece).unwrap();
        }
        client.shutdown(Shutdown::Write).unwrap();
        let mut echoed = Vec::new();
        client.read_to_end(&mut echoed).unwrap();
        assert_eq!(echoed, message, "the end passes through too");
    }

    #[test]
    fn a_server_that_does_not_answer_closes_the_connection() {
        // A port that was just free: nothing listens there.
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap();
        let relay = Relay::start(0, free).unwrap();
        let mut client = connect(&relay);
        let mut rest = Vec::new();
        assert_eq!(
            client
                .read_to_end(&mut rest)
                .map(|_| rest.len())
                .unwrap_or(0),
            0
        );
    }

    #[test]
    fn connections_are_bounded_and_dropping_stops_listening() {
        let relay = Relay::start(0, echo_server()).unwrap();
        let mut held: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| connect(&relay)).collect();
        // Each held connection works, so all of them hold a slot.
        for stream in &mut held {
            stream.write_all(b"ping").unwrap();
            let mut answer = [0u8; 4];
            stream.read_exact(&mut answer).unwrap();
            assert_eq!(&answer, b"ping");
        }
        let mut extra = connect(&relay);
        let mut rest = Vec::new();
        let _ = extra.write_all(b"ping");
        assert_eq!(
            extra
                .read_to_end(&mut rest)
                .map(|_| rest.len())
                .unwrap_or(0),
            0
        );
        // A closed connection frees its slot.
        drop(held.pop());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let mut again = connect(&relay);
            let _ = again.write_all(b"pong");
            let mut answer = [0u8; 4];
            if again.read_exact(&mut answer).is_ok() {
                assert_eq!(&answer, b"pong");
                break;
            }
            assert!(std::time::Instant::now() < deadline, "no slot came free");
            std::thread::sleep(Duration::from_millis(50));
        }
        let address = relay.address();
        drop(relay);
        let refused = match TcpStream::connect(address) {
            Err(_) => true,
            Ok(mut stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut rest = Vec::new();
                stream.read_to_end(&mut rest).map_or(true, |n| n == 0)
            }
        };
        assert!(refused, "a dropped relay no longer relays");
    }
}
