// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket},
    thread,
    time::{Duration, Instant},
};

use nfs_protocol::qos::LatencyProbe;

use crate::qos_tls::{self, Identity};

pub const MAX_CONNECTIONS: usize = 4;
pub const MAX_DATAGRAMS: usize = 128;
pub const MAX_DATAGRAM_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stop {
    Deadline,
    ConnectionLimit,
    DatagramLimit,
    NonLoopback,
    Io,
    Allocation,
    WorkerFailure,
}

pub struct Datagram {
    pub peer: SocketAddr,
    pub request: Vec<u8>,
    pub response: Vec<u8>,
}

pub struct UdpObservation {
    pub status: Stop,
    pub datagrams: Vec<Datagram>,
    pub received_datagrams: usize,
    pub invalid_datagrams: usize,
    pub oversized_datagrams: usize,
    pub rejected_peers: usize,
    pub io_errors: usize,
    pub replies_sent: usize,
}

impl UdpObservation {
    fn empty(status: Stop) -> Self {
        Self {
            status,
            datagrams: Vec::new(),
            received_datagrams: 0,
            invalid_datagrams: 0,
            oversized_datagrams: 0,
            rejected_peers: 0,
            io_errors: 0,
            replies_sent: 0,
        }
    }
}

pub struct Observation {
    pub status: Stop,
    pub connections: Vec<qos_tls::Observation>,
    pub udp: UdpObservation,
    pub accepted_connections: usize,
    pub rejected_connections: usize,
    pub connection_errors: usize,
}

impl Observation {
    fn empty(status: Stop) -> Self {
        Self {
            status,
            connections: Vec::new(),
            udp: UdpObservation::empty(status),
            accepted_connections: 0,
            rejected_connections: 0,
            connection_errors: 0,
        }
    }
}

pub fn run(
    listener: TcpListener,
    udp: UdpSocket,
    deadline: Instant,
    identity: &Identity,
) -> Observation {
    let mut result = Observation::empty(Stop::Io);
    let addresses = listener
        .local_addr()
        .and_then(|tls| udp.local_addr().map(|datagram| (tls, datagram)));
    let advertised = match addresses {
        Ok((tls, datagram)) if owned_loopback(tls) && owned_loopback(datagram) => {
            let SocketAddr::V4(datagram) = datagram else {
                unreachable!("owned_loopback accepts only IPv4");
            };
            datagram
        }
        Ok(_) => return Observation::empty(Stop::NonLoopback),
        Err(_) => return result,
    };
    if remaining(deadline).is_none() {
        return Observation::empty(Stop::Deadline);
    }
    if listener.set_nonblocking(true).is_err() || udp.set_nonblocking(false).is_err() {
        return result;
    }
    if result
        .connections
        .try_reserve_exact(MAX_CONNECTIONS)
        .is_err()
    {
        return Observation::empty(Stop::Allocation);
    }
    thread::scope(|scope| {
        let mut workers = Vec::new();
        if workers.try_reserve_exact(MAX_CONNECTIONS).is_err() {
            result.status = Stop::Allocation;
            result.udp.status = Stop::Allocation;
            return;
        }
        let Ok(udp_worker) = thread::Builder::new()
            .name("qos-udp".into())
            .spawn_scoped(scope, move || observe_udp(udp, deadline))
        else {
            result.status = Stop::WorkerFailure;
            result.udp.status = Stop::WorkerFailure;
            return;
        };
        result.status = Stop::ConnectionLimit;
        while result.accepted_connections < MAX_CONNECTIONS {
            let Some(left) = remaining(deadline) else {
                result.status = Stop::Deadline;
                break;
            };
            match listener.accept() {
                Ok((socket, peer)) => {
                    result.accepted_connections += 1;
                    if !peer.ip().is_loopback() {
                        result.rejected_connections += 1;
                        continue;
                    }
                    match thread::Builder::new()
                        .name("qos-tls".into())
                        .spawn_scoped(scope, move || {
                            qos_tls::serve_latency(socket, deadline, identity, advertised)
                        }) {
                        Ok(worker) => workers.push(worker),
                        Err(_) => {
                            result.connection_errors += 1;
                            result.status = Stop::WorkerFailure;
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(left.min(Duration::from_millis(5)));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    result.connection_errors += 1;
                    result.status = Stop::Io;
                    break;
                }
            }
        }
        drop(listener);
        for worker in workers {
            match worker.join() {
                Ok(observation) => result.connections.push(observation),
                Err(_) => {
                    result.connection_errors += 1;
                    result.status = Stop::WorkerFailure;
                }
            }
        }
        result.udp = udp_worker
            .join()
            .unwrap_or_else(|_| UdpObservation::empty(Stop::WorkerFailure));
    });
    result
}

fn owned_loopback(address: SocketAddr) -> bool {
    matches!(address, SocketAddr::V4(address)
        if *address.ip() == Ipv4Addr::LOCALHOST && address.port() != 0)
}

fn remaining(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

fn observe_udp(socket: UdpSocket, deadline: Instant) -> UdpObservation {
    let mut result = UdpObservation::empty(Stop::DatagramLimit);
    if result.datagrams.try_reserve_exact(MAX_DATAGRAMS).is_err() {
        result.status = Stop::Allocation;
        return result;
    }
    let mut bytes = [0_u8; MAX_DATAGRAM_BYTES + 1];
    while result.received_datagrams < MAX_DATAGRAMS {
        let Some(left) = remaining(deadline) else {
            result.status = Stop::Deadline;
            break;
        };
        if socket.set_read_timeout(Some(left)).is_err() {
            result.status = Stop::Io;
            result.io_errors += 1;
            break;
        }
        let (size, peer) = match socket.recv_from(&mut bytes) {
            Ok(received) => received,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(10040) => {
                result.received_datagrams += 1;
                result.invalid_datagrams += 1;
                result.oversized_datagrams += 1;
                result.io_errors += 1;
                continue;
            }
            Err(e) => {
                result.status = io_stop(&e);
                result.io_errors += usize::from(result.status == Stop::Io);
                break;
            }
        };
        result.received_datagrams += 1;
        let mut record = Datagram {
            peer,
            request: Vec::new(),
            response: Vec::new(),
        };
        let captured = size.min(MAX_DATAGRAM_BYTES);
        if record.request.try_reserve_exact(captured).is_err() {
            result.status = Stop::Allocation;
            break;
        }
        record.request.extend_from_slice(&bytes[..captured]);
        let response = if size > MAX_DATAGRAM_BYTES {
            result.invalid_datagrams += 1;
            result.oversized_datagrams += 1;
            None
        } else if !peer.ip().is_loopback() {
            result.rejected_peers += 1;
            None
        } else {
            match peer {
                SocketAddr::V4(peer) => LatencyProbe::parse(&record.request)
                    .and_then(|probe| probe.encode_reply(peer))
                    .ok(),
                SocketAddr::V6(_) => None,
            }
            .or_else(|| {
                result.invalid_datagrams += 1;
                None
            })
        };
        if let Some(response) = response {
            if record.response.try_reserve_exact(response.len()).is_err() {
                result.status = Stop::Allocation;
                result.datagrams.push(record);
                break;
            }
            match send_datagram(&socket, &response, peer, deadline) {
                Ok(size) => {
                    record.response.extend_from_slice(&response[..size]);
                    if size == response.len() {
                        result.replies_sent += 1;
                    } else {
                        result.status = Stop::Io;
                        result.io_errors += 1;
                        result.datagrams.push(record);
                        break;
                    }
                }
                Err(error) => {
                    result.status = io_stop(&error);
                    result.io_errors += usize::from(result.status == Stop::Io);
                    result.datagrams.push(record);
                    break;
                }
            }
        }
        result.datagrams.push(record);
    }
    result
}

fn send_datagram(
    socket: &UdpSocket,
    response: &[u8],
    peer: SocketAddr,
    deadline: Instant,
) -> io::Result<usize> {
    loop {
        let left = remaining(deadline).ok_or(io::ErrorKind::TimedOut)?;
        socket.set_write_timeout(Some(left))?;
        match socket.send_to(response, peer) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

fn io_stop(error: &io::Error) -> Stop {
    if matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        Stop::Deadline
    } else {
        Stop::Io
    }
}
