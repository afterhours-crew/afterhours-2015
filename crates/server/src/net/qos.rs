// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Long-lived loopback QoS edge. Bounds apply to concurrent requests and their
//! deadlines, not to the number of sessions or the time spent at activation.
use crate::Failure;
use nfs_protocol::qos::LatencyProbe;
use nfs_server_support::{qos_service::MAX_DATAGRAM_BYTES, qos_tls};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::watch,
    task::JoinSet,
};

const MAX_TLS: usize = 4;

// Cancellation also reaches blocking workers when this future is dropped.
#[derive(Default)]
struct Cancellation(Arc<AtomicBool>);
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub(super) async fn serve(
    tcp: std::net::TcpListener,
    udp: std::net::UdpSocket,
    identity: Arc<qos_tls::Identity>,
    request_timeout: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Failure> {
    let io = |_| Failure::Io;
    let SocketAddr::V4(advertised) = udp.local_addr().map_err(io)? else {
        return Err(Failure::Io);
    };
    if *advertised.ip() != Ipv4Addr::LOCALHOST
        || !tcp.local_addr().map_err(io)?.ip().is_loopback()
        || request_timeout.is_zero()
        || request_timeout > Duration::from_secs(86_400)
    {
        return Err(Failure::Io);
    }
    tcp.set_nonblocking(true).map_err(io)?;
    udp.set_nonblocking(true).map_err(io)?;
    let tcp = TcpListener::from_std(tcp).map_err(io)?;
    let udp = UdpSocket::from_std(udp).map_err(io)?;
    let mut workers = JoinSet::new();
    let cancellation = Cancellation::default();
    let mut bytes = [0; MAX_DATAGRAM_BYTES + 1];
    let result = loop {
        if *shutdown.borrow() {
            break Ok(());
        }
        tokio::select! {
            _ = shutdown.changed() => break Ok(()),
            result = workers.join_next(), if !workers.is_empty() => {
                match result {
                    Some(Ok(status)) => {
                        tracing::debug!(?status, "qos request ended");
                    }
                    _ => break Err(Failure::Io),
                }
            }
            accepted = tcp.accept(), if workers.len() < MAX_TLS => {
                let (socket, peer) = match accepted { Ok(v) => v, Err(_) => break Err(Failure::Io) };
                if !peer.ip().is_loopback() { continue; }
                let socket = match socket.into_std() { Ok(v) => v, Err(_) => break Err(Failure::Io) };
                let identity = identity.clone();
                let cancelled = cancellation.0.clone();
                let deadline = Instant::now() + request_timeout;
                workers.spawn_blocking(move || {
                    qos_tls::serve_latency_cancellable(socket, deadline, &identity, advertised, cancelled).status
                });
            }
            received = udp.recv_from(&mut bytes) => {
                let (size, peer) = match received {
                    Ok(v) => v,
                    Err(e) if cfg!(windows) && e.raw_os_error() == Some(10040) => continue,
                    Err(_) => break Err(Failure::Io),
                };
                let SocketAddr::V4(peer) = peer else { continue };
                if !peer.ip().is_loopback() || size > MAX_DATAGRAM_BYTES { continue; }
                let Ok(reply) = LatencyProbe::parse(&bytes[..size]).and_then(|p| p.encode_reply(peer)) else { continue };
                // Local UDP has no reason to hold shutdown hostage if its send
                // buffer is full; the client can retry a dropped probe.
                let _ = udp.try_send_to(&reply, SocketAddr::V4(peer));
            }
        }
    };
    drop(tcp);
    drop(udp);
    drop(cancellation);
    while workers.join_next().await.is_some() {}
    result
}

#[cfg(test)]
mod tests;
