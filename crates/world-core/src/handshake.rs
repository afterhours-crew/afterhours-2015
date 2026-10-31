// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Host side of the CommUDP connection handshake.
//!
//! 1. The client sends an initial-header datagram from its tunnel `index` with
//!    `sender = client id`, `peer_known = false` and one channel-0 part
//!    `[1u32, token, client id]`.
//! 2. The host answers with an initial header `sender = host id`, its own tunnel
//!    index, `peer_known = true` and `[5u32, token, host id]`.
//! 3. The client confirms with an ordinary header addressed to the host's tunnel
//!    index and `[2u32, token]`. The connection is then established and the link
//!    layer takes over the codec and cipher cursors.
//!
//! The client and host ids are the low 32 bits of the connection group ids from
//! the G2 world setup. Retransmitted requests get the identical reply again;
//! stale or foreign datagrams are ignored rather than ending the session.
use crate::transport::{self, Codec, Header, Part};

mod client;
pub use client::ClientHandshake;
mod control;
pub use control::Control;

/// Largest forward jump of the client's cipher cursor accepted (lost datagrams).
pub const MAX_FORWARD_UNITS: u16 = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ignored {
    /// Not decodable with this session's key, or failed authentication.
    Transport(transport::Error),
    /// Cursor older than already-consumed input (duplicate or reordered).
    Stale,
    /// Cursor too far ahead of the expected position.
    Gap,
    /// Authenticated but not the expected message for this step.
    Unexpected,
    /// Sender id, tunnel index or token does not match this session.
    Identity,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Step {
    /// Send these bytes to the client.
    Send(Vec<u8>),
    /// The client confirmed; call [`HostHandshake::established`].
    Established,
    Ignored(Ignored),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    AwaitRequest,
    Replied,
    Established,
}

/// Cipher cursors and tunnel indices handed to the link layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cursors {
    /// Next cursor expected from the client.
    pub rx: u16,
    /// Next cursor the host will send with.
    pub tx: u16,
    /// Client's tunnel index (used in headers the host sends).
    pub client_index: u16,
    /// Host's tunnel index (used in headers the client sends).
    pub host_index: u16,
    /// The client's connection token, echoed by transport control messages.
    pub token: u32,
}

pub struct HostHandshake {
    codec: Codec,
    client: u32,
    host: u32,
    host_index: u16,
    client_index: Option<u16>,
    token: Option<u32>,
    rx: u16,
    tx: u16,
    reply: Option<Vec<u8>>,
    state: State,
}

impl HostHandshake {
    /// `client`/`host` are the connection ids; they must differ and be non-zero.
    pub fn new(codec: Codec, client: u32, host: u32, host_index: u16) -> Option<Self> {
        if client == 0 || host == 0 || client == host || host_index > transport::MAX_CURSOR {
            return None;
        }
        Some(Self {
            codec,
            client,
            host,
            host_index,
            client_index: None,
            token: None,
            rx: 0,
            tx: 0,
            reply: None,
            state: State::AwaitRequest,
        })
    }

    pub fn is_established(&self) -> bool {
        self.state == State::Established
    }

    fn check_cursor(&self, header: Header) -> Option<Ignored> {
        let cursor = header.cursor();
        if cursor < self.rx {
            Some(Ignored::Stale)
        } else if cursor - self.rx > MAX_FORWARD_UNITS {
            Some(Ignored::Gap)
        } else {
            None
        }
    }

    /// Process one datagram from the client.
    pub fn receive(&mut self, raw: &[u8]) -> Step {
        let decoded = match self.codec.decode(raw) {
            Ok(d) => d,
            Err(e) => return Step::Ignored(Ignored::Transport(e)),
        };
        let [part] = decoded.parts.as_slice() else {
            return Step::Ignored(Ignored::Unexpected);
        };
        if part.channel != 0 {
            return Step::Ignored(Ignored::Unexpected);
        }
        match (self.state, decoded.header, Control::decode(&part.bytes)) {
            (
                State::AwaitRequest,
                Header::Initial {
                    sender,
                    index,
                    peer_known: false,
                    ..
                },
                Some(Control::Request { token, sender: id }),
            ) => {
                if sender != self.client || id != self.client {
                    return Step::Ignored(Ignored::Identity);
                }
                if let Some(ignored) = self.check_cursor(decoded.header) {
                    return Step::Ignored(ignored);
                }
                let reply = Control::Reply {
                    token,
                    sender: self.host,
                }
                .encode();
                let header = Header::Initial {
                    sender: self.host,
                    index: self.host_index,
                    peer_known: true,
                    cursor: self.tx,
                };
                let bytes = match self.codec.encode(
                    header,
                    &[Part {
                        channel: 0,
                        bytes: reply,
                    }],
                ) {
                    Ok(b) => b,
                    Err(e) => return Step::Ignored(Ignored::Transport(e)),
                };
                let Ok(rx) = decoded.next_cursor() else {
                    return Step::Ignored(Ignored::Gap);
                };
                let Ok(tx) = transport::advance(self.tx, bytes.len() - 11) else {
                    return Step::Ignored(Ignored::Gap);
                };
                self.rx = rx;
                self.tx = tx;
                self.token = Some(token);
                self.client_index = Some(index);
                self.reply = Some(bytes.clone());
                self.state = State::Replied;
                Step::Send(bytes)
            }
            (
                State::Replied,
                Header::Initial { sender, .. },
                Some(Control::Request { token, .. }),
            ) if Some(token) == self.token && sender == self.client => {
                // The client did not get our reply: send the identical bytes again.
                Step::Send(self.reply.clone().unwrap_or_default())
            }
            (State::Replied, Header::Ordinary { index, .. }, Some(Control::Confirm { token })) => {
                if index != self.host_index || Some(token) != self.token {
                    return Step::Ignored(Ignored::Identity);
                }
                if let Some(ignored) = self.check_cursor(decoded.header) {
                    return Step::Ignored(ignored);
                }
                let Ok(rx) = decoded.next_cursor() else {
                    return Step::Ignored(Ignored::Gap);
                };
                self.rx = rx;
                self.state = State::Established;
                Step::Established
            }
            _ => Step::Ignored(Ignored::Unexpected),
        }
    }

    /// Hand the codec and cursors to the link layer once established.
    pub fn established(self) -> Option<(Codec, Cursors)> {
        if self.state != State::Established {
            return None;
        }
        let cursors = Cursors {
            rx: self.rx,
            tx: self.tx,
            client_index: self.client_index?,
            host_index: self.host_index,
            token: self.token?,
        };
        Some((self.codec, cursors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::session_key;

    const CLIENT: u32 = 0x0102_0304;
    const HOST: u32 = 0x0a0b_0c0d;

    fn codec() -> Codec {
        let key = session_key(
            "11111111-2222-3333-4444-555555555555",
            "66666666-7777-8888-9999-aaaaaaaaaaaa",
        )
        .unwrap();
        Codec::new(key, std::array::from_fn(|i| (i * 7) as u8)).unwrap()
    }

    /// Client-side datagrams built with the same codec, as the game would.
    fn request(cursor: u16, token: u32) -> Vec<u8> {
        codec()
            .encode(
                Header::Initial {
                    sender: CLIENT,
                    index: 9,
                    peer_known: false,
                    cursor,
                },
                &[Part {
                    channel: 0,
                    bytes: [1, token, CLIENT]
                        .iter()
                        .flat_map(|w| w.to_be_bytes())
                        .collect(),
                }],
            )
            .unwrap()
    }

    fn confirm(cursor: u16, token: u32, index: u16) -> Vec<u8> {
        codec()
            .encode(
                Header::Ordinary { index, cursor },
                &[Part {
                    channel: 0,
                    bytes: [2, token].iter().flat_map(|w| w.to_be_bytes()).collect(),
                }],
            )
            .unwrap()
    }

    #[test]
    fn request_reply_confirm_establishes_with_observed_sizes_and_cursors() {
        let mut h = HostHandshake::new(codec(), CLIENT, HOST, 3).unwrap();
        let req = request(0, 77);
        assert_eq!(req.len(), 37, "observed request size");
        let Step::Send(reply) = h.receive(&req) else {
            panic!("expected reply")
        };
        assert_eq!(reply.len(), 37, "observed reply size");
        let decoded = codec().decode(&reply).unwrap();
        assert_eq!(
            decoded.header,
            Header::Initial {
                sender: HOST,
                index: 3,
                peer_known: true,
                cursor: 0
            }
        );
        assert_eq!(
            decoded.parts[0].bytes,
            [5, 77, HOST]
                .iter()
                .flat_map(|w| w.to_be_bytes())
                .collect::<Vec<_>>()
        );
        // Request consumed 26 encrypted bytes (4 units); confirmation is 26 bytes.
        let conf = confirm(4, 77, 3);
        assert_eq!(conf.len(), 26, "observed confirmation size");
        assert_eq!(h.receive(&conf), Step::Established);
        let (_, cursors) = h.established().unwrap();
        assert_eq!(
            cursors,
            Cursors {
                rx: 7,
                tx: 4,
                client_index: 9,
                host_index: 3,
                token: 77
            }
        );
    }

    #[test]
    fn retransmitted_request_gets_the_identical_reply() {
        let mut h = HostHandshake::new(codec(), CLIENT, HOST, 3).unwrap();
        let Step::Send(first) = h.receive(&request(0, 5)) else {
            panic!()
        };
        assert_eq!(h.receive(&request(0, 5)), Step::Send(first));
    }

    #[test]
    fn malformed_retransmission_cannot_hide_trailing_partial_words() {
        let mut host = HostHandshake::new(codec(), CLIENT, HOST, 3).unwrap();
        let request = request(0, 5);
        let Step::Send(reply) = host.receive(&request) else {
            panic!("reply")
        };
        let decoded = codec().decode(&request).unwrap();
        for length in 1..=4 {
            let mut parts = decoded.parts.clone();
            parts[0].bytes.extend(vec![0; length]);
            let malformed = codec().encode(decoded.header, &parts).unwrap();
            assert_eq!(host.receive(&malformed), Step::Ignored(Ignored::Unexpected));
        }
        assert_eq!(host.receive(&request), Step::Send(reply));
        assert_eq!(host.receive(&confirm(4, 5, 3)), Step::Established);
    }

    #[test]
    fn both_transport_endpoints_negotiate_without_prescribed_reply_bytes() {
        let mut host = HostHandshake::new(codec(), CLIENT, HOST, 3).unwrap();
        let mut peer = ClientHandshake::new(codec(), CLIENT, HOST, 9, 77).unwrap();
        assert_eq!(peer.request(), request(0, 77));
        let Step::Send(reply) = host.receive(peer.request()) else {
            panic!("host reply")
        };
        let Step::Send(confirmation) = peer.receive(&reply) else {
            panic!("peer confirmation")
        };
        assert_eq!(confirmation, confirm(4, 77, 3));
        assert!(peer.is_established());
        assert_eq!(peer.receive(&reply), Step::Send(confirmation.clone()));
        assert_eq!(host.receive(&confirmation), Step::Established);
        let (host_codec, host_cursors) = host.established().unwrap();
        let (peer_codec, peer_cursors) = peer.established().unwrap();
        assert_eq!(
            (peer_cursors.rx, peer_cursors.tx),
            (host_cursors.tx, host_cursors.rx)
        );
        let (mut host, sync) = crate::link::Link::start(host_codec, host_cursors, 10).unwrap();
        let (mut peer, answer) =
            crate::link::Link::respond(peer_codec, peer_cursors, &sync, 11).unwrap();
        let confirmation = host.receive(&answer, 12).unwrap();
        peer.receive(&confirmation.send[0], 13).unwrap();
        assert!(host.running() && peer.running());
        let message = peer.send_application(b"application", 14).unwrap();
        assert_eq!(
            host.receive(&message, 15).unwrap().applications,
            [b"application"]
        );
    }

    #[test]
    fn requesting_endpoint_rejects_foreign_reply_without_losing_retry_state() {
        assert!(ClientHandshake::new(codec(), HOST, HOST, 9, 77).is_none());
        assert!(ClientHandshake::new(codec(), 0, HOST, 9, 77).is_none());
        assert!(ClientHandshake::new(codec(), CLIENT, HOST, 0x8000, 77).is_none());
        let mut peer = ClientHandshake::new(codec(), CLIENT, HOST, 9, 77).unwrap();
        let mut other = HostHandshake::new(codec(), CLIENT, HOST, 3).unwrap();
        let Step::Send(foreign) = other.receive(&request(0, 78)) else {
            panic!("other reply")
        };
        assert_eq!(peer.receive(&foreign), Step::Ignored(Ignored::Identity));
        assert!(!peer.is_established());
        let mut host = HostHandshake::new(codec(), CLIENT, HOST, 3).unwrap();
        let Step::Send(reply) = host.receive(peer.request()) else {
            panic!("host reply")
        };
        let mut corrupt = reply.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(matches!(
            peer.receive(&corrupt),
            Step::Ignored(Ignored::Transport(_))
        ));
        assert!(!peer.is_established());
        assert!(matches!(peer.receive(&reply), Step::Send(_)));
        assert_eq!(peer.receive(&foreign), Step::Ignored(Ignored::Unexpected));
    }

    #[test]
    fn wrong_identity_token_index_and_garbage_are_ignored() {
        let mut h = HostHandshake::new(codec(), CLIENT, HOST, 3).unwrap();
        assert!(matches!(
            h.receive(&[0u8; 37]),
            Step::Ignored(Ignored::Transport(_))
        ));
        let Step::Send(_) = h.receive(&request(0, 5)) else {
            panic!()
        };
        assert_eq!(
            h.receive(&confirm(4, 6, 3)),
            Step::Ignored(Ignored::Identity)
        );
        assert_eq!(
            h.receive(&confirm(4, 5, 2)),
            Step::Ignored(Ignored::Identity)
        );
        assert_eq!(h.receive(&confirm(2, 5, 3)), Step::Ignored(Ignored::Stale));
        assert_eq!(h.receive(&confirm(4, 5, 3)), Step::Established);
    }

    #[test]
    fn foreign_sender_is_ignored_and_ids_must_differ() {
        let mut h = HostHandshake::new(codec(), CLIENT + 1, HOST, 3).unwrap();
        assert_eq!(h.receive(&request(0, 1)), Step::Ignored(Ignored::Identity));
        assert!(HostHandshake::new(codec(), HOST, HOST, 3).is_none());
        assert!(HostHandshake::new(codec(), 0, HOST, 3).is_none());
    }

    #[test]
    fn a_wrong_key_is_rejected_by_authentication() {
        let other = Codec::new(
            session_key(
                "21111111-2222-3333-4444-555555555555",
                "66666666-7777-8888-9999-aaaaaaaaaaaa",
            )
            .unwrap(),
            std::array::from_fn(|i| (i * 7) as u8),
        )
        .unwrap();
        let mut h = HostHandshake::new(other, CLIENT, HOST, 3).unwrap();
        // Garbage descriptors or a bad MAC: either way the datagram is rejected.
        assert!(matches!(
            h.receive(&request(0, 1)),
            Step::Ignored(Ignored::Transport(
                transport::Error::Authentication | transport::Error::Descriptor
            ))
        ));
    }
}
