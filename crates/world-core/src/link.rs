// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Link layer above the transport.
//!
//! Every datagram carries one channel-0 part:
//!
//! ```text
//! u32 count << 28 | newest sequence (24 bits)
//! u32 acknowledgement: newest in-order reliable sequence received from the peer
//!... envelopes, oldest first; each older envelope is followed by a length byte
//! ```
//!
//! Reliable sequences run 256..=0xFF_FFFF and wrap to 256; unreliable ones run
//! 128..=255 and wrap to 128. `count` older reliable envelopes may be repeated in
//! front of the newest one. An envelope ends with a kind byte (2..=11, or 0 for
//! sync-only); bit 0x40 marks a 17-byte sync trailer (echo, sent tick, latency,
//! four counters, reserved, the constant 16, kind byte).
//!
//! Observed use: reliable envelopes carry sync only; application traffic (kind 6,
//! holding the kind-20 application envelope) travels unreliably, because the
//! application layer has its own sequence, acknowledgement and history.
//!
//! The host sends one reliable sync immediately after the handshake (sequence 256,
//! acknowledgement 0xFF_FFFF), answers the client's first sync with an empty
//! acknowledgement, then sends an idle sync after more than 500 ms without any
//! send, and adds a sync trailer to an application envelope when more than 250 ms
//! passed since the last sync.
use crate::{
    handshake::Cursors,
    transport::{self, Codec, Header, Part},
};

pub const MIN_RELIABLE: u32 = 256;
pub const MAX_RELIABLE: u32 = 0x00ff_ffff;
pub const MIN_UNRELIABLE: u32 = 128;
pub const MAX_UNRELIABLE: u32 = 255;
pub const IDLE_SYNC_MS: u32 = 500;
pub const OPTIONAL_SYNC_MS: u32 = 250;
/// Resend an unacknowledged reliable sync after this long.
pub const RESEND_MS: u64 = 1000;
/// Kind of the envelope that carries the application (kind-20) layer.
pub const APPLICATION_KIND: u8 = 6;
/// Transport control code the client sends when it closes the connection.
pub const CLOSE_CODE: u32 = 3;
const SYNC_LEN: usize = 17;

pub fn next_reliable(sequence: u32) -> u32 {
    if sequence == MAX_RELIABLE {
        MIN_RELIABLE
    } else {
        sequence + 1
    }
}

pub fn previous_reliable(sequence: u32) -> u32 {
    if sequence == MIN_RELIABLE {
        MAX_RELIABLE
    } else {
        sequence - 1
    }
}

fn next_unreliable(sequence: u32) -> u32 {
    if sequence == MAX_UNRELIABLE {
        MIN_UNRELIABLE
    } else {
        sequence + 1
    }
}

const RELIABLE_SPAN: u32 = MAX_RELIABLE - MIN_RELIABLE + 1;

/// Forward distance from `from` to `to` on the reliable ring.
fn distance(from: u32, to: u32) -> u32 {
    (to + RELIABLE_SPAN - from) % RELIABLE_SPAN
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SyncFields {
    pub echo: u32,
    pub sent: u32,
    pub latency: u16,
    pub counters: [u8; 4],
}

impl SyncFields {
    fn encode(self, kind: u8) -> [u8; SYNC_LEN] {
        let mut out = [0; SYNC_LEN];
        out[..4].copy_from_slice(&self.echo.to_be_bytes());
        out[4..8].copy_from_slice(&self.sent.to_be_bytes());
        out[8..10].copy_from_slice(&self.latency.to_be_bytes());
        out[10..14].copy_from_slice(&self.counters);
        out[15] = 16;
        out[16] = kind | 0x40;
        out
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Transport(transport::Error),
    /// Malformed link framing.
    Framing,
    /// Unknown envelope kind.
    Kind,
}

/// One envelope in a received datagram.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope<'a> {
    pub sequence: u32,
    pub kind: u8,
    pub body: &'a [u8],
    pub sync: Option<SyncFields>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parsed<'a> {
    pub acknowledgement: u32,
    pub reliable: bool,
    pub envelopes: Vec<Envelope<'a>>,
}

fn word(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn split_trailer(raw: &[u8]) -> Result<(u8, &[u8], Option<SyncFields>), Error> {
    let tail = *raw.last().ok_or(Error::Framing)?;
    let kind = tail & !0x40;
    if kind != 0 && !(2..=11).contains(&kind) {
        return Err(Error::Kind);
    }
    if tail & 0x40 == 0 {
        if kind == 0 {
            return Err(Error::Framing);
        }
        return Ok((kind, &raw[..raw.len() - 1], None));
    }
    if raw.len() < SYNC_LEN {
        return Err(Error::Framing);
    }
    let split = raw.len() - SYNC_LEN;
    let t = &raw[split..];
    if t[15] != 16 {
        return Err(Error::Framing);
    }
    Ok((
        kind,
        &raw[..split],
        Some(SyncFields {
            echo: word(t),
            sent: word(&t[4..]),
            latency: u16::from_be_bytes([t[8], t[9]]),
            counters: [t[10], t[11], t[12], t[13]],
        }),
    ))
}

/// Parse a channel-0 link part.
pub fn parse(inner: &[u8]) -> Result<Parsed<'_>, Error> {
    if inner.len() < 8 {
        return Err(Error::Framing);
    }
    let header = word(inner);
    if header & 0x0f00_0000 != 0 {
        return Err(Error::Framing);
    }
    let count = (header >> 28) as u8;
    let newest = header & MAX_RELIABLE;
    let acknowledgement = word(&inner[4..]);
    let reliable = newest >= MIN_RELIABLE;
    if newest < MIN_UNRELIABLE || (!reliable && count != 0) {
        return Err(Error::Framing);
    }
    let body = &inner[8..];
    if body.is_empty() {
        if count != 0 {
            return Err(Error::Framing);
        }
        return Ok(Parsed {
            acknowledgement,
            reliable,
            envelopes: Vec::new(),
        });
    }
    let mut sequence = newest;
    for _ in 0..count {
        sequence = previous_reliable(sequence);
    }
    let mut end = body.len();
    let mut pieces = Vec::with_capacity(usize::from(count) + 1);
    for _ in 0..count {
        end = end.checked_sub(1).ok_or(Error::Framing)?;
        let len = usize::from(body[end]);
        let start = end
            .checked_sub(len)
            .filter(|_| len > 0)
            .ok_or(Error::Framing)?;
        pieces.push(&body[start..end]);
        end = start;
    }
    if end == 0 {
        return Err(Error::Framing);
    }
    pieces.push(&body[..end]);
    let mut envelopes = Vec::with_capacity(pieces.len());
    for raw in pieces {
        let (kind, payload, sync) = split_trailer(raw)?;
        envelopes.push(Envelope {
            sequence,
            kind,
            body: payload,
            sync,
        });
        if reliable {
            sequence = next_reliable(sequence);
        }
    }
    Ok(Parsed {
        acknowledgement,
        reliable,
        envelopes,
    })
}

/// Round-trip and peer-clock state for sync fields .
#[derive(Clone, Debug, Default)]
struct Clock {
    last_send: u32,
    last_sync: u32,
    latest_peer_sent: u32,
    peer_receive: u32,
    last_rtt: u32,
    average: u32,
    variation: u32,
    peer_sent_total: u32,
    received_at_peer_sync: u32,
    sent: u32,
    received: u32,
    baseline_sent: u32,
    baseline_received: u32,
}

impl Clock {
    fn accepted_sync(&mut self, fields: SyncFields, now: u32) {
        let candidate = (now as u16)
            .wrapping_sub(fields.echo as u16)
            .wrapping_add(1);
        if candidate <= 2500 {
            let dt = (now.wrapping_sub(self.last_rtt) as i32).max(10) as u32;
            self.last_rtt = now;
            if dt >= 1024 {
                self.average = u32::from(candidate);
                self.variation = 0;
            } else {
                let old = self.average;
                self.variation =
                    (u32::from(candidate).abs_diff(old) * dt + self.variation * (1024 - dt)) / 1024;
                self.average = (u32::from(candidate) * dt + old * (1024 - dt)) / 1024;
            }
        }
        self.latest_peer_sent = fields.sent;
        self.peer_receive = now;
        if self.peer_sent_total == 0 {
            self.peer_sent_total = 1;
        }
        self.peer_sent_total = self
            .peer_sent_total
            .wrapping_add(u32::from(fields.counters[0]));
        self.received_at_peer_sync = self.received;
    }

    fn fields(&self, now: u32) -> SyncFields {
        let derived = self
            .peer_sent_total
            .saturating_sub(self.received_at_peer_sync);
        SyncFields {
            echo: self
                .latest_peer_sent
                .wrapping_add(now.wrapping_sub(self.peer_receive)),
            sent: now,
            latency: (self.average + self.variation).div_ceil(2) as u16,
            counters: [
                self.sent.wrapping_sub(self.baseline_sent) as u8,
                self.received.wrapping_sub(self.baseline_received) as u8,
                derived as u8,
                0,
            ],
        }
    }

    fn synced(&mut self, now: u32) {
        self.last_sync = now;
        self.baseline_sent = self.sent;
        self.baseline_received = self.received;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// Initial sync sent; waiting for the client's first sync.
    AwaitPeerSync,
    Running,
}

/// What a received datagram delivered.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Delivery {
    /// Kind-6 application bodies, in arrival order.
    pub applications: Vec<Vec<u8>>,
    /// Envelopes of other kinds (kind, body), retained for diagnostics.
    pub other: Vec<(u8, Vec<u8>)>,
    /// Datagrams to send in response (for example the first empty acknowledgement).
    pub send: Vec<Vec<u8>>,
    /// Duplicate reliable envelopes that were dropped.
    pub duplicates: usize,
    /// The client closed the connection (transport control code 3).
    pub closed: bool,
}

struct Outstanding {
    sequence: u32,
    body: Vec<u8>,
    sent_ms: u64,
}

/// One established world connection, host side.
pub struct Link {
    // Fixed-size per-direction cipher state; one allocation per connection,
    // keeping the session stage small without allocating on each datagram.
    streams: Box<Streams>,
    cursors: Cursors,
    phase: Phase,
    next_reliable: u32,
    expected_reliable: u32,
    next_unreliable: u32,
    outstanding: Option<Outstanding>,
    clock: Clock,
}

struct Streams {
    receive: transport::Stream,
    send: transport::Stream,
}

impl Link {
    fn new(codec: Codec, cursors: Cursors, now_ms: u64) -> Result<Self, Error> {
        let tick = now_ms as u32;
        Ok(Self {
            streams: Box::new(Streams {
                receive: codec.stream(cursors.rx).map_err(Error::Transport)?,
                send: codec.stream(cursors.tx).map_err(Error::Transport)?,
            }),
            cursors,
            phase: Phase::AwaitPeerSync,
            next_reliable: MIN_RELIABLE,
            expected_reliable: MIN_RELIABLE,
            next_unreliable: MIN_UNRELIABLE,
            outstanding: None,
            clock: Clock {
                last_send: tick,
                last_sync: tick,
                ..Clock::default()
            },
        })
    }

    /// Take over an established handshake and produce the initial host sync.
    pub fn start(codec: Codec, cursors: Cursors, now_ms: u64) -> Result<(Self, Vec<u8>), Error> {
        let mut link = Self::new(codec, cursors, now_ms)?;
        let tick = now_ms as u32;
        let initial = SyncFields {
            echo: tick,
            sent: tick,
            ..SyncFields::default()
        }
        .encode(0);
        let wire = link.initial_sync(&initial, MAX_RELIABLE, now_ms)?;
        Ok((link, wire))
    }

    /// Answer the first reliable sync after externally validated admission.
    /// `cursors` is relative to this endpoint: `host_index` is our receiving
    /// index, `client_index` is the peer's, and rx/tx are our cipher positions.
    /// This is a fresh link, not a restore API. The supplied datagram must contain
    /// exactly the peer's initial reliable sync; application data is rejected.
    pub fn respond(
        codec: Codec,
        cursors: Cursors,
        first_sync: &[u8],
        now_ms: u64,
    ) -> Result<(Self, Vec<u8>), Error> {
        let mut link = Self::new(codec, cursors, now_ms)?;
        // Consume the peer's sync before building ours, so the first outbound
        // reliable packet acknowledges what was actually received. Suppress the
        // host-only initial empty ACK until our own sync has been acknowledged.
        link.phase = Phase::Running;
        let received = link.receive(first_sync, now_ms)?;
        if received.closed
            || received.duplicates != 0
            || !received.applications.is_empty()
            || !received.other.is_empty()
            || link.expected_reliable != next_reliable(MIN_RELIABLE)
        {
            return Err(Error::Framing);
        }
        link.phase = Phase::AwaitPeerSync;
        let body = link.clock.fields(now_ms as u32).encode(0);
        let wire = link.initial_sync(&body, link.acknowledgement(), now_ms)?;
        Ok((link, wire))
    }

    fn initial_sync(&mut self, body: &[u8], ack: u32, now_ms: u64) -> Result<Vec<u8>, Error> {
        let wire = self.datagram(MIN_RELIABLE, ack, body)?;
        self.outstanding = Some(Outstanding {
            sequence: MIN_RELIABLE,
            body: body.to_vec(),
            sent_ms: now_ms,
        });
        self.next_reliable = next_reliable(MIN_RELIABLE);
        self.clock.sent = 1;
        Ok(wire)
    }

    pub fn cursors(&self) -> Cursors {
        self.cursors
    }

    fn acknowledgement(&self) -> u32 {
        previous_reliable(self.expected_reliable)
    }

    /// Encode one datagram with `body` (possibly empty) and advance the tx cursor.
    fn datagram(&mut self, sequence: u32, ack: u32, body: &[u8]) -> Result<Vec<u8>, Error> {
        let mut inner = Vec::with_capacity(8 + body.len());
        inner.extend_from_slice(&sequence.to_be_bytes());
        inner.extend_from_slice(&ack.to_be_bytes());
        inner.extend_from_slice(body);
        let header = Header::Ordinary {
            index: self.cursors.client_index,
            cursor: self.cursors.tx,
        };
        let wire = self
            .streams
            .send
            .encode(
                header,
                &[Part {
                    channel: 0,
                    bytes: inner,
                }],
            )
            .map_err(Error::Transport)?;
        self.cursors.tx =
            transport::advance(self.cursors.tx, wire.len() - 4).map_err(Error::Transport)?;
        Ok(wire)
    }

    fn note_send(&mut self, now_ms: u64) {
        self.clock.last_send = now_ms as u32;
        self.clock.sent = self.clock.sent.wrapping_add(1);
    }

    /// Process one datagram from the client.
    pub fn receive(&mut self, raw: &[u8], now_ms: u64) -> Result<Delivery, Error> {
        let header = Header::parse(raw).map_err(Error::Transport)?;
        let Header::Ordinary { index, .. } = header else {
            return Err(Error::Framing);
        };
        if index != self.cursors.host_index {
            return Err(Error::Framing);
        }
        let mut candidate = self.streams.receive.clone();
        let decoded = match candidate.decode(raw) {
            Ok(decoded) => decoded,
            Err(transport::Error::Stale) => {
                return Ok(Delivery {
                    duplicates: 1,
                    ..Delivery::default()
                });
            }
            Err(error) => return Err(Error::Transport(error)),
        };
        let [part] = decoded.parts.as_slice() else {
            return Err(Error::Framing);
        };
        if part.channel != 0 {
            return Err(Error::Framing);
        }
        self.streams.receive = candidate;
        self.cursors.rx = decoded.next_cursor().map_err(Error::Transport)?;
        // Transport control messages share channel 0: [code, token] with code < 128,
        // which can never be a data header (sequences start at 128).
        if part.bytes.len() == 8 && word(&part.bytes) < MIN_UNRELIABLE {
            if word(&part.bytes[4..]) != self.cursors.token {
                return Err(Error::Framing);
            }
            return Ok(Delivery {
                closed: word(&part.bytes) == CLOSE_CODE,
                ..Delivery::default()
            });
        }
        let parsed = parse(&part.bytes)?;
        self.clock.received = self.clock.received.wrapping_add(1);
        let now = now_ms as u32;
        if self
            .outstanding
            .as_ref()
            .is_some_and(|o| o.sequence == parsed.acknowledgement)
        {
            self.outstanding = None;
        }
        let mut delivery = Delivery::default();
        for envelope in parsed.envelopes {
            if parsed.reliable {
                let gap = distance(self.expected_reliable, envelope.sequence);
                if gap != 0 {
                    // Behind us: duplicate. Ahead: a lost envelope we wait to be repeated.
                    delivery.duplicates += 1;
                    continue;
                }
                self.expected_reliable = next_reliable(self.expected_reliable);
            }
            if let Some(fields) = envelope.sync {
                self.clock.accepted_sync(fields, now);
            }
            match envelope.kind {
                0 => {}
                APPLICATION_KIND => delivery.applications.push(envelope.body.to_vec()),
                kind => delivery.other.push((kind, envelope.body.to_vec())),
            }
        }
        if self.phase == Phase::AwaitPeerSync && self.outstanding.is_none() && parsed.reliable {
            // The client's first sync acknowledged ours: answer with an empty ACK.
            self.phase = Phase::Running;
            let ack = self.acknowledgement();
            let wire = self.datagram(self.next_reliable, ack, &[])?;
            self.note_send(now_ms);
            delivery.send.push(wire);
        }
        Ok(delivery)
    }

    /// True once the initial sync exchange completed.
    pub fn running(&self) -> bool {
        self.phase == Phase::Running
    }

    /// Send one kind-6 application body (unreliable at link level).
    pub fn send_application(&mut self, application: &[u8], now_ms: u64) -> Result<Vec<u8>, Error> {
        let now = now_ms as u32;
        let mut body = application.to_vec();
        if now.wrapping_sub(self.clock.last_sync) > OPTIONAL_SYNC_MS {
            body.extend_from_slice(&self.clock.fields(now).encode(APPLICATION_KIND));
            self.clock.synced(now);
        } else {
            body.push(APPLICATION_KIND);
        }
        let sequence = self.next_unreliable;
        self.next_unreliable = next_unreliable(sequence);
        let ack = self.acknowledgement();
        let wire = self.datagram(sequence, ack, &body)?;
        self.note_send(now_ms);
        Ok(wire)
    }

    /// Timer: an idle reliable sync, or a resend of an unacknowledged one.
    pub fn poll(&mut self, now_ms: u64) -> Result<Option<Vec<u8>>, Error> {
        if let Some(o) = &self.outstanding {
            if now_ms.saturating_sub(o.sent_ms) < RESEND_MS {
                return Ok(None);
            }
            let (sequence, body) = (o.sequence, o.body.clone());
            let ack = if self.phase == Phase::Running {
                self.acknowledgement()
            } else {
                MAX_RELIABLE
            };
            let wire = self.datagram(sequence, ack, &body)?;
            if let Some(o) = self.outstanding.as_mut() {
                o.sent_ms = now_ms;
            }
            self.note_send(now_ms);
            return Ok(Some(wire));
        }
        if self.phase != Phase::Running
            || (now_ms as u32).wrapping_sub(self.clock.last_send) <= IDLE_SYNC_MS
        {
            return Ok(None);
        }
        let now = now_ms as u32;
        let body = self.clock.fields(now).encode(0);
        self.clock.synced(now);
        let sequence = self.next_reliable;
        let ack = self.acknowledgement();
        let wire = self.datagram(sequence, ack, &body)?;
        self.outstanding = Some(Outstanding {
            sequence,
            body: body.to_vec(),
            sent_ms: now_ms,
        });
        self.next_reliable = next_reliable(sequence);
        self.note_send(now_ms);
        Ok(Some(wire))
    }
}

#[cfg(test)]
mod tests;
