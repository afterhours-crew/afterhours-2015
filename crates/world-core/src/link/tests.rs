// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{crypto::session_key, handshake::Cursors};

const HOST_INDEX: u16 = 0;
const CLIENT_INDEX: u16 = 9;

fn peer_cursors() -> Cursors {
    let c = cursors();
    Cursors {
        rx: c.tx,
        tx: c.rx,
        client_index: c.host_index,
        host_index: c.client_index,
        token: c.token,
    }
}

#[test]
fn responder_acknowledges_received_sync_then_exchanges_real_application_datagrams() {
    let (mut host, first) = Link::start(codec(), cursors(), 100).unwrap();
    let (mut peer, response) = Link::respond(codec(), peer_cursors(), &first, 103).unwrap();
    assert!(!peer.running());
    let decoded = codec().decode(&response).unwrap();
    let parsed = parse(&decoded.parts[0].bytes).unwrap();
    assert_eq!(parsed.acknowledgement, MIN_RELIABLE);
    let sync = parsed.envelopes[0].sync.unwrap();
    assert_eq!((sync.echo, sync.sent), (100, 103));
    let acknowledged = host.receive(&response, 105).unwrap();
    assert!(host.running());
    assert_eq!(acknowledged.send.len(), 1);
    let confirmation = peer.receive(&acknowledged.send[0], 107).unwrap();
    assert!(peer.running());
    for wire in confirmation.send {
        host.receive(&wire, 109).unwrap();
    }
    let from_host = host.send_application(b"host", 110).unwrap();
    assert_eq!(
        peer.receive(&from_host, 111).unwrap().applications,
        [b"host"]
    );
    let from_peer = peer.send_application(b"peer", 112).unwrap();
    assert_eq!(
        host.receive(&from_peer, 113).unwrap().applications,
        [b"peer"]
    );
    assert_eq!(host.receive(&from_peer, 114).unwrap().duplicates, 1);
    assert_eq!(peer.cursors().rx, host.cursors().tx);
    assert_eq!(host.cursors().rx, peer.cursors().tx);
}

#[test]
fn responder_refuses_corrupt_foreign_or_noninitial_input() {
    let (mut host, first) = Link::start(codec(), cursors(), 0).unwrap();
    let mut corrupt = first.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(matches!(
        Link::respond(codec(), peer_cursors(), &corrupt, 1),
        Err(Error::Transport(transport::Error::Authentication))
    ));
    let mut foreign = peer_cursors();
    foreign.host_index += 1;
    assert!(matches!(
        Link::respond(codec(), foreign, &first, 1),
        Err(Error::Framing)
    ));
    let application = host.send_application(b"not a first sync", 1).unwrap();
    assert!(matches!(
        Link::respond(codec(), peer_cursors(), &application, 2),
        Err(Error::Framing)
    ));
    let empty = host.datagram(MIN_RELIABLE, MAX_RELIABLE, &[]).unwrap();
    assert!(matches!(
        Link::respond(codec(), peer_cursors(), &empty, 2),
        Err(Error::Framing)
    ));
}

fn codec() -> Codec {
    let key = session_key(
        "11111111-2222-3333-4444-555555555555",
        "66666666-7777-8888-9999-aaaaaaaaaaaa",
    )
    .unwrap();
    Codec::new(key, std::array::from_fn(|i| (i * 3) as u8)).unwrap()
}

fn cursors() -> Cursors {
    Cursors {
        rx: 7,
        tx: 4,
        client_index: CLIENT_INDEX,
        host_index: HOST_INDEX,
        token: 0x5151_0001,
    }
}

/// The client's side of the link, built with the same codec.
struct Client {
    codec: Codec,
    tx: u16,
}

impl Client {
    fn new() -> Self {
        Self {
            codec: codec(),
            tx: 7,
        }
    }
    fn send(&mut self, sequence: u32, ack: u32, body: &[u8]) -> Vec<u8> {
        let mut inner = sequence.to_be_bytes().to_vec();
        inner.extend_from_slice(&ack.to_be_bytes());
        inner.extend_from_slice(body);
        let wire = self
            .codec
            .encode(
                Header::Ordinary {
                    index: HOST_INDEX,
                    cursor: self.tx,
                },
                &[Part {
                    channel: 0,
                    bytes: inner,
                }],
            )
            .unwrap();
        self.tx = transport::advance(self.tx, wire.len() - 4).unwrap();
        wire
    }
    fn open(&self, wire: &[u8]) -> (Header, Vec<u8>) {
        let d = self.codec.decode(wire).unwrap();
        (d.header, d.parts[0].bytes.clone())
    }
}

fn sync(echo: u32, sent: u32) -> Vec<u8> {
    SyncFields {
        echo,
        sent,
        ..SyncFields::default()
    }
    .encode(0)
    .to_vec()
}

fn running_link(now: u64) -> (Link, Client) {
    let (mut link, _) = Link::start(codec(), cursors(), now).unwrap();
    let mut client = Client::new();
    let first = client.send(256, 256, &sync(now as u32, now as u32 + 3));
    let d = link.receive(&first, now + 5).unwrap();
    assert_eq!(d.send.len(), 1);
    assert!(link.running());
    (link, client)
}

#[test]
fn initial_sync_matches_the_observed_shape() {
    let (link, wire) = Link::start(codec(), cursors(), 1_000).unwrap();
    assert_eq!(wire.len(), 43, "observed 43-byte initial sync");
    let client = Client::new();
    let (header, inner) = client.open(&wire);
    assert_eq!(
        header,
        Header::Ordinary {
            index: CLIENT_INDEX,
            cursor: 4
        }
    );
    let parsed = parse(&inner).unwrap();
    assert_eq!(parsed.acknowledgement, MAX_RELIABLE);
    assert!(parsed.reliable);
    let [envelope] = parsed.envelopes.as_slice() else {
        panic!()
    };
    assert_eq!((envelope.sequence, envelope.kind), (256, 0));
    let s = envelope.sync.unwrap();
    assert_eq!((s.echo, s.sent), (1_000, 1_000));
    assert_eq!(link.cursors().tx, 4 + 5);
    assert!(!link.running());
}

#[test]
fn link_keeps_both_cipher_directions_across_wrap_and_ignores_old_datagrams() {
    let c = codec();
    let mut client_tx = c.stream(32763).unwrap();
    let mut client_rx = c.stream(32763).unwrap();
    let (mut link, initial) = Link::start(
        codec(),
        Cursors {
            rx: 32763,
            tx: 32763,
            ..cursors()
        },
        1000,
    )
    .unwrap();
    assert_eq!(client_rx.decode(&initial).unwrap().header.cursor(), 32763);
    assert_eq!(link.cursors().tx, 0);
    let mut inner = 256u32.to_be_bytes().to_vec();
    inner.extend_from_slice(&256u32.to_be_bytes());
    inner.extend(sync(1000, 1003));
    let first = client_tx
        .encode(
            Header::Ordinary {
                index: HOST_INDEX,
                cursor: client_tx.cursor(),
            },
            &[Part {
                channel: 0,
                bytes: inner,
            }],
        )
        .unwrap();
    let out = link.receive(&first, 1005).unwrap();
    assert!(link.running());
    assert_eq!(link.cursors().rx, 0);
    for wire in out.send {
        client_rx.decode(&wire).unwrap();
    }
    let before = link.cursors();
    assert_eq!(link.receive(&first, 1006).unwrap().duplicates, 1);
    assert_eq!(link.cursors(), before);
    let mut inner = 128u32.to_be_bytes().to_vec();
    inner.extend_from_slice(&256u32.to_be_bytes());
    inner.extend([0x12, 0x34, APPLICATION_KIND]);
    let second = client_tx
        .encode(
            Header::Ordinary {
                index: HOST_INDEX,
                cursor: client_tx.cursor(),
            },
            &[Part {
                channel: 0,
                bytes: inner,
            }],
        )
        .unwrap();
    let mut bad = second.clone();
    *bad.last_mut().unwrap() ^= 1;
    assert!(link.receive(&bad, 1007).is_err());
    assert_eq!(link.cursors(), before);
    assert_eq!(
        link.receive(&second, 1008).unwrap().applications,
        vec![vec![0x12, 0x34]]
    );
    let reply = link.send_application(&[0x56], 1009).unwrap();
    let decoded = client_rx.decode(&reply).unwrap();
    assert_eq!(
        parse(&decoded.parts[0].bytes).unwrap().envelopes[0].body,
        [0x56]
    );
}

#[test]
fn first_client_sync_gets_the_observed_empty_acknowledgement() {
    let (mut link, _) = Link::start(codec(), cursors(), 1_000).unwrap();
    let mut client = Client::new();
    let first = client.send(256, 256, &sync(1_000, 50_000));
    assert_eq!(first.len(), 43);
    let d = link.receive(&first, 1_020).unwrap();
    let [ack] = d.send.as_slice() else { panic!() };
    assert_eq!(ack.len(), 26, "observed 26-byte empty ACK");
    let inner = client.open(ack).1;
    let parsed = parse(&inner).unwrap();
    assert_eq!(parsed.acknowledgement, 256);
    assert!(parsed.reliable && parsed.envelopes.is_empty());
    assert_eq!(word(&client.open(ack).1), 257);
    assert!(link.running());
}

#[test]
fn applications_without_and_with_a_sync_trailer_have_the_observed_sizes() {
    let (mut link, client) = running_link(1_000);
    // Within 250 ms of the last sync: kind byte only (13-byte body -> 40 bytes).
    let short = link.send_application(&[0xaa; 13], 1_100).unwrap();
    assert_eq!(short.len(), 40);
    let inner = client.open(&short).1;
    let parsed = parse(&inner).unwrap();
    assert!(!parsed.reliable);
    assert_eq!(parsed.envelopes[0].sequence, 128);
    assert_eq!(parsed.envelopes[0].kind, APPLICATION_KIND);
    assert_eq!(parsed.envelopes[0].body, &[0xaa; 13]);
    assert!(parsed.envelopes[0].sync.is_none());
    // More than 250 ms later: sync trailer added (13-byte body -> 56 bytes).
    let long = link.send_application(&[0xbb; 13], 1_400).unwrap();
    assert_eq!(long.len(), 56);
    let inner = client.open(&long).1;
    let parsed = parse(&inner).unwrap();
    assert_eq!(parsed.envelopes[0].sequence, 129);
    assert_eq!(parsed.envelopes[0].sync.unwrap().sent, 1_400);
}

#[test]
fn client_applications_are_delivered_and_duplicates_dropped() {
    let (mut link, mut client) = running_link(1_000);
    let mut app = vec![1, 2, 3];
    app.push(APPLICATION_KIND);
    let d = link.receive(&client.send(128, 256, &app), 1_050).unwrap();
    assert_eq!(d.applications, vec![vec![1, 2, 3]]);
    // A reliable sync already delivered (256) arriving again is a duplicate.
    let d = link
        .receive(&client.send(256, 256, &sync(1, 2)), 1_060)
        .unwrap();
    assert_eq!(d.duplicates, 1);
    // A datagram older than the consumed cursor is stale.
    let mut stale = Client::new();
    let old = stale.send(129, 256, &app);
    assert_eq!(link.receive(&old, 1_070).unwrap().duplicates, 1);
}

#[test]
fn idle_sync_is_reliable_and_resent_until_acknowledged() {
    let (mut link, mut client) = running_link(1_000);
    assert_eq!(link.poll(1_400).unwrap(), None, "not idle yet");
    let idle = link.poll(1_600).unwrap().unwrap();
    assert_eq!(idle.len(), 43);
    let inner = client.open(&idle).1;
    let parsed = parse(&inner).unwrap();
    assert_eq!(parsed.envelopes[0].sequence, 257);
    assert_eq!(parsed.envelopes[0].kind, 0);
    // Not acknowledged within RESEND_MS: the same sequence is sent again.
    assert_eq!(link.poll(2_000).unwrap(), None);
    let resent = link.poll(2_700).unwrap().unwrap();
    let inner = client.open(&resent).1;
    assert_eq!(parse(&inner).unwrap().envelopes[0].sequence, 257);
    // The client acknowledges 257: no more resends.
    link.receive(&client.send(257, 257, &sync(2_700, 9)), 2_710)
        .unwrap();
    assert_eq!(link.poll(3_200).unwrap(), None);
}

#[test]
fn packed_older_envelopes_parse_oldest_first() {
    // Newest 300 first, then older pieces each followed by its length byte;
    // the parser walks lengths from the end, so the last piece is the oldest.
    let oldest = [0xa1, 0xa2, 2];
    let middle = [0xb1, 3];
    let newest = [0xc1, 0xc2, 0xc3, 4];
    let mut inner = ((2u32 << 28) | 300).to_be_bytes().to_vec();
    inner.extend_from_slice(&7u32.to_be_bytes());
    inner.extend_from_slice(&newest);
    inner.extend_from_slice(&middle);
    inner.push(middle.len() as u8);
    inner.extend_from_slice(&oldest);
    inner.push(oldest.len() as u8);
    let parsed = parse(&inner).unwrap();
    let seen: Vec<_> = parsed
        .envelopes
        .iter()
        .map(|e| (e.sequence, e.kind, e.body.to_vec()))
        .collect();
    assert_eq!(
        seen,
        vec![
            (298, 2, vec![0xa1, 0xa2]),
            (299, 3, vec![0xb1]),
            (300, 4, vec![0xc1, 0xc2, 0xc3]),
        ]
    );
    assert_eq!(parsed.acknowledgement, 7);
}

#[test]
fn malformed_link_parts_are_rejected() {
    assert_eq!(parse(&[0; 7]), Err(Error::Framing));
    // Unreliable sequence below 128.
    let mut inner = 5u32.to_be_bytes().to_vec();
    inner.extend_from_slice(&[0; 4]);
    inner.push(6);
    assert_eq!(parse(&inner), Err(Error::Framing));
    // Unknown kind 12.
    let mut inner = 300u32.to_be_bytes().to_vec();
    inner.extend_from_slice(&[0; 4]);
    inner.push(12);
    assert_eq!(parse(&inner), Err(Error::Kind));
}

#[test]
fn transport_close_is_reported_and_foreign_tokens_rejected() {
    let (mut link, mut client) = running_link(1_000);
    let mut close = CLOSE_CODE.to_be_bytes().to_vec();
    close.extend_from_slice(&0x5151_0001u32.to_be_bytes());
    let wire = {
        let w = client
            .codec
            .encode(
                Header::Ordinary {
                    index: HOST_INDEX,
                    cursor: client.tx,
                },
                &[Part {
                    channel: 0,
                    bytes: close.clone(),
                }],
            )
            .unwrap();
        client.tx = transport::advance(client.tx, w.len() - 4).unwrap();
        w
    };
    assert_eq!(wire.len(), 26, "observed 26-byte close");
    assert!(link.receive(&wire, 1_100).unwrap().closed);
    let mut foreign = CLOSE_CODE.to_be_bytes().to_vec();
    foreign.extend_from_slice(&7u32.to_be_bytes());
    let w = client
        .codec
        .encode(
            Header::Ordinary {
                index: HOST_INDEX,
                cursor: client.tx,
            },
            &[Part {
                channel: 0,
                bytes: foreign,
            }],
        )
        .unwrap();
    assert_eq!(link.receive(&w, 1_200), Err(Error::Framing));
}
