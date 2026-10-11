// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Application layer (kind 20) above the link: the host side of the challenge,
//! admission, timing and control exchanges, with per-connection sequence
//! history.
//!
//! Startup exchange from the host perspective:
//!
//! ```text
//! client Request13 → host Challenge15 (fresh u32, empty optional)
//! client Answer16 → host Admission1 (fresh engine token)
//! client Client2 → host Admission9, Admission4 (peer echo, assigned selector, callback 0)
//! client Client5 → host Callback6 (client fresh echoed + fresh host word)
//! then sequenced kind-20 envelopes both ways: queued listener data and controls
//! ```
//!
//! Up to Callback6 the envelopes carry no sequence header. After it, every
//! envelope has a ten-bit sequence, a ten-bit acknowledgement and 32 history
//! bits; the client sends with the host-assigned selector and the host sends
//! with the client's selector. Controls: subtype 2 is a timing request (one
//! timestamp), subtype 3 its reply (echo plus the sender's clock), subtype 1 an
//! empty control the native reader consumes silently. Queued payloads
//! carry an optional rate advertisement, a fragment header and handler-frame
//! bits; the [`Listener`] decides whether a complete frame counts as accepted.
//!
//! Wire shapes come from the pure codecs in [`nfs_protocol::world`]; this module
//! owns the state machine, the host's sequence numbers and the timers. It is
//! sans-IO: bodies and a millisecond clock in, bodies out.

use crate::bits::BitWriter;
use nfs_protocol::world::{
    BitSpan,
    admission::{
        self, Message,
        type_zero::{Client5, Host6},
    },
    challenge::{Challenge15, OpaqueAnswer16, Request13},
    envelope::{ZeroTail, checksum},
    fragment::{Assembler, Fragment, Outcome},
    history::{AdvanceError, PeerReports, ReceiveHistory, Sequence},
    payload::{Advertised, Route},
    timing::{self, Control},
};
use std::fmt;

pub mod delivery;

/// Application kind of every envelope this layer sends or accepts after admission.
pub const SEQUENCED_KIND: u8 = 20;
/// The one selector a single-peer host assigns its client. Official hosts
/// assigned 4 and kept 1 for the client's own name of the host (E762); a client
/// given 1 here would share an id with the host on owner connection fields.
pub const HOST_SELECTOR: u16 = 4;
/// Native challenge entries live ten seconds.
pub const CHALLENGE_WINDOW_MS: u64 = 10_000;
/// Timing and empty-control subtypes.
pub const CONTROL_EMPTY: u8 = 1;
pub const CONTROL_TIMING_REQUEST: u8 = 2;
pub const CONTROL_TIMING_REPLY: u8 = 3;
/// Application window: 32 sequences in flight before the native scheduler
/// emits the empty control under window pressure.
pub const WINDOW: u16 = 32;
/// Data bits per fragment the original hosts use for frames larger than one
/// datagram: 1,024-byte bodies carrying 8,056 bits, then the remainder.
pub const DEFAULT_FRAGMENT_BITS: usize = 8_056;
/// Conservative sender budget for the supported build: 2,048 bytes, the smaller
/// directional buffer. Applies to either role.
/// This is independent of this server's larger inbound safety limit.
pub const OUTBOUND_FRAME_BITS: usize = 2_048 * 8;

/// Local policy, not native constants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Policy {
    /// Largest application body accepted.
    pub max_body: usize,
    /// Largest reassembled handler frame, in bits.
    pub max_frame_bits: usize,
    /// Interval between the host's timing requests once the first was sent.
    pub timing_interval_ms: u64,
    /// Shortest interval between empty controls sent under window pressure.
    pub control_interval_ms: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_body: 1200,
            max_frame_bits: 65_536,
            timing_interval_ms: 2_000,
            control_interval_ms: 500,
        }
    }
}

/// Source of the fresh 32-bit words the host puts on the wire.
pub trait Random {
    fn next_u32(&mut self) -> u32;
}

/// A fixed sequence of words, for tests and the reference harness.
#[derive(Clone, Debug, Default)]
pub struct Fixed {
    words: std::collections::VecDeque<u32>,
}

impl Fixed {
    pub fn new(words: impl IntoIterator<Item = u32>) -> Self {
        Self {
            words: words.into_iter().collect(),
        }
    }
}

impl Random for Fixed {
    fn next_u32(&mut self) -> u32 {
        self.words.pop_front().unwrap_or(0)
    }
}

/// Receives complete queued handler frames.
pub trait Listener {
    /// Decide whether a complete frame counts as accepted in the application
    /// history. Returning `false` keeps the frame unacknowledged, so the client
    /// resends reliable messages inside it for unknown frames.
    fn frame(&mut self, frame: Queued<'_>) -> bool;
}

/// One complete queued frame delivered to a [`Listener`].
#[derive(Clone, Copy)]
pub struct Queued<'a> {
    /// The peer's rate advertisement, when present.
    pub advertised: Option<Advertised>,
    /// Fragment frame identity.
    pub frame: u16,
    /// Reassembled handler-frame bits.
    pub data: BitSpan<'a>,
}

impl fmt::Debug for Queued<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queued")
            .field("frame", &self.frame)
            .field("bits", &self.data.len())
            .finish_non_exhaustive()
    }
}

/// Keeps every frame unaccepted and counts it .
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Retain {
    pub frames: usize,
    pub bits: usize,
}

impl Listener for Retain {
    fn frame(&mut self, frame: Queued<'_>) -> bool {
        self.frames += 1;
        self.bits += frame.data.len();
        false
    }
}

/// Why an input was rejected. The state is unchanged after an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Not a well-formed body for the current phase.
    Shape,
    /// A valid body that does not belong to the current phase.
    Phase,
    /// Wrong connection selector.
    Selector,
    /// The client's echo does not match what the host sent.
    Echo,
    /// A retransmission that differs from the first copy.
    ChangedRetry,
    /// Larger than the policy allows.
    Bound,
    /// Clock went backwards.
    Clock,
    /// The application window is full: the client has not acknowledged the
    /// host's recent sends .
    Window,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "application {self:?}")
    }
}
impl std::error::Error for Error {}

/// What an accepted input was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    ChallengeRequest {
        repeated: bool,
    },
    Answer {
        repeated: bool,
    },
    Client2 {
        repeated: bool,
    },
    Client5 {
        repeated: bool,
    },
    /// A queued fragment: `complete` when it finished a frame; `accepted` is the
    /// history bit recorded for it.
    Queued {
        bits: usize,
        complete: bool,
        accepted: bool,
    },
    TimingRequest,
    TimingReply {
        matched: bool,
        round_trip_ms: Option<u64>,
    },
    EmptyControl,
    /// A control subtype this layer does not know; recorded unaccepted.
    UnknownControl(u8),
    /// A sequence already received, or outside the 32-position window.
    Duplicate,
    OutOfWindow,
}

/// Result of one accepted input: the event plus bodies to send in reply.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Received {
    pub event: Option<Event>,
    pub send: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhaseName {
    AwaitRequest,
    Challenged,
    AwaitClient2,
    AwaitClient5,
    Sequenced,
}

/// Counters for diagnostics and the reference harness.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub inputs: usize,
    pub sent: usize,
    pub duplicates: usize,
    pub out_of_window: usize,
    pub queued_inputs: usize,
    pub frames_complete: usize,
    pub frames_accepted: usize,
    pub timing_requests_sent: usize,
    pub timing_replies_sent: usize,
    pub timing_matched: usize,
    pub empty_controls_received: usize,
    pub empty_controls_sent: usize,
}

enum Phase {
    AwaitRequest,
    Challenged {
        fields: [u32; 2],
        challenge: Vec<u8>,
        since_ms: u64,
    },
    AwaitClient2 {
        answer: Vec<u8>,
        host1: Vec<u8>,
    },
    AwaitClient5 {
        client2: Vec<u8>,
        peer_token: u32,
        host9: Vec<u8>,
        host4: Vec<u8>,
    },
    Sequenced(Box<Sequenced>),
}

struct Sequenced {
    client5: Vec<u8>,
    host6: Vec<u8>,
    /// Selector on envelopes the client sends to the host.
    inbound: u16,
    /// Selector on envelopes the host sends.
    outbound: u16,
    origin_ms: u64,
    history: ReceiveHistory,
    reports: PeerReports,
    deliveries: delivery::Deliveries,
    next_number: u16,
    next_frame: u16,
    assembler: Assembler,
    pending_advertised: Option<Advertised>,
    own_request: Option<(u64, u64)>,
    last_request_ms: Option<u64>,
    last_control_ms: Option<u64>,
}

/// The application layer of one world connection, host side.
pub struct Application {
    policy: Policy,
    random: Box<dyn Random + Send>,
    phase: Phase,
    last_ms: u64,
    stats: Stats,
}

impl fmt::Debug for Application {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Application")
            .field("phase", &self.phase_name())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

fn elapsed_bits(origin_ms: u64, now_ms: u64) -> u64 {
    (now_ms.saturating_sub(origin_ms) as f64 / 1000.0).to_bits()
}

/// Sent sequences the client has not acknowledged yet.
fn outstanding(s: &Sequenced) -> u16 {
    s.next_number
        .wrapping_sub(1)
        .wrapping_sub(s.reports.frontier().value())
        & 1023
}

/// Encode one sequenced kind-20 envelope: 80-bit header, payload bits, zero
/// padding to a byte boundary and the two checksum bytes.
pub fn sequenced(
    selector: u16,
    number: u16,
    ack: u16,
    history: u32,
    payload: &BitWriter,
) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.put(((payload.len() + 96) % 8) as u64, 3)
        .put(0, 3)
        .put(u64::from(selector), 14)
        .put(u64::from(SEQUENCED_KIND), 8)
        .put(u64::from(number), 10)
        .put(u64::from(ack), 10)
        .put(u64::from(history), 32)
        .put_span(payload.span());
    w.align();
    let mut bytes = w.into_bytes();
    let check = checksum(&bytes).to_be_bytes();
    bytes.extend_from_slice(&check);
    bytes
}

fn control_payload(subtype: u8, timestamps: &[u64]) -> BitWriter {
    let mut p = BitWriter::new();
    p.put(1, 1).put(u64::from(subtype), 4);
    for t in timestamps {
        p.put(*t, 64);
    }
    p
}

impl Application {
    pub fn new(policy: Policy, random: Box<dyn Random + Send>) -> Self {
        Self {
            policy,
            random,
            phase: Phase::AwaitRequest,
            last_ms: 0,
            stats: Stats::default(),
        }
    }

    pub fn phase_name(&self) -> PhaseName {
        match &self.phase {
            Phase::AwaitRequest => PhaseName::AwaitRequest,
            Phase::Challenged { .. } => PhaseName::Challenged,
            Phase::AwaitClient2 { .. } => PhaseName::AwaitClient2,
            Phase::AwaitClient5 { .. } => PhaseName::AwaitClient5,
            Phase::Sequenced(_) => PhaseName::Sequenced,
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Selectors once admitted: (inbound from the client, outbound to it).
    pub fn selectors(&self) -> Option<(u16, u16)> {
        match &self.phase {
            Phase::Sequenced(s) => Some((s.inbound, s.outbound)),
            _ => None,
        }
    }

    /// Milliseconds of the Callback6 send, the origin of this connection's clock words.
    pub fn origin_ms(&self) -> Option<u64> {
        match &self.phase {
            Phase::Sequenced(s) => Some(s.origin_ms),
            _ => None,
        }
    }

    /// The host's receive frontier and history bits once admitted.
    pub fn receive_history(&self) -> Option<(u16, u32)> {
        match &self.phase {
            Phase::Sequenced(s) => Some((s.history.frontier().value(), s.history.bits())),
            _ => None,
        }
    }

    fn clock(&mut self, now_ms: u64) -> Result<(), Error> {
        if now_ms < self.last_ms {
            return Err(Error::Clock);
        }
        self.last_ms = now_ms;
        Ok(())
    }

    /// Process one application body from the client.
    pub fn receive(
        &mut self,
        body: &[u8],
        now_ms: u64,
        listener: &mut dyn Listener,
    ) -> Result<Received, Error> {
        if body.len() > self.policy.max_body {
            return Err(Error::Bound);
        }
        self.clock(now_ms)?;
        let received = match &mut self.phase {
            Phase::AwaitRequest => self.challenge(body, now_ms)?,
            Phase::Challenged {
                fields,
                challenge,
                since_ms,
            } => {
                let (fields, challenge, since) = (*fields, challenge.clone(), *since_ms);
                if let Ok(request) = Request13::decode(body) {
                    if now_ms.saturating_sub(since) >= CHALLENGE_WINDOW_MS {
                        self.challenge(body, now_ms)?
                    } else if request.fields() == fields {
                        Received {
                            event: Some(Event::ChallengeRequest { repeated: true }),
                            send: vec![challenge],
                        }
                    } else {
                        return Err(Error::ChangedRetry);
                    }
                } else if OpaqueAnswer16::decode(body).is_ok() {
                    let host1 = admission::Decoded::new(Message::Host1 {
                        engine_token: self.random.next_u32(),
                    })
                    .and_then(|d| d.encode())
                    .map_err(|_| Error::Shape)?;
                    self.phase = Phase::AwaitClient2 {
                        answer: body.to_vec(),
                        host1: host1.clone(),
                    };
                    Received {
                        event: Some(Event::Answer { repeated: false }),
                        send: vec![host1],
                    }
                } else {
                    return Err(Error::Shape);
                }
            }
            Phase::AwaitClient2 { answer, host1 } => {
                if body == answer.as_slice() {
                    Received {
                        event: Some(Event::Answer { repeated: true }),
                        send: vec![host1.clone()],
                    }
                } else {
                    let host1 = host1.clone();
                    self.client2(body, &host1)?
                }
            }
            Phase::AwaitClient5 {
                client2,
                peer_token,
                host9,
                host4,
            } => {
                if body == client2.as_slice() {
                    Received {
                        event: Some(Event::Client2 { repeated: true }),
                        send: vec![host9.clone(), host4.clone()],
                    }
                } else {
                    let peer_token = *peer_token;
                    self.client5(body, peer_token, now_ms)?
                }
            }
            Phase::Sequenced(_) => self.sequenced_input(body, now_ms, listener)?,
        };
        self.stats.inputs += 1;
        self.stats.sent += received.send.len();
        Ok(received)
    }

    fn challenge(&mut self, body: &[u8], now_ms: u64) -> Result<Received, Error> {
        let request = Request13::decode(body).map_err(|_| Error::Shape)?;
        let challenge = Challenge15::new(self.random.next_u32(), &[])
            .map_err(|_| Error::Shape)?
            .encode();
        self.phase = Phase::Challenged {
            fields: request.fields(),
            challenge: challenge.clone(),
            since_ms: now_ms,
        };
        Ok(Received {
            event: Some(Event::ChallengeRequest { repeated: false }),
            send: vec![challenge],
        })
    }

    fn client2(&mut self, body: &[u8], host1: &[u8]) -> Result<Received, Error> {
        let decoded = admission::Decoded::decode(body).map_err(|_| Error::Shape)?;
        let Message::Client2 {
            peer_token,
            engine_echo,
            ..
        } = decoded.message
        else {
            return Err(Error::Phase);
        };
        let Message::Host1 { engine_token } = admission::Decoded::decode(host1)
            .map_err(|_| Error::Shape)?
            .message
        else {
            return Err(Error::Shape);
        };
        if engine_echo != engine_token {
            return Err(Error::Echo);
        }
        let encode = |m| {
            admission::Decoded::new(m)
                .and_then(|d| d.encode())
                .map_err(|_| Error::Shape)
        };
        let host9 = encode(Message::Host9)?;
        let host4 = encode(Message::Host4 {
            peer_echo: peer_token,
            assigned_selector: HOST_SELECTOR,
            callback_kind: 0,
            opaque: Vec::new(),
        })?;
        self.phase = Phase::AwaitClient5 {
            client2: body.to_vec(),
            peer_token,
            host9: host9.clone(),
            host4: host4.clone(),
        };
        Ok(Received {
            event: Some(Event::Client2 { repeated: false }),
            send: vec![host9, host4],
        })
    }

    fn client5(&mut self, body: &[u8], peer_token: u32, now_ms: u64) -> Result<Received, Error> {
        let decoded = admission::Decoded::decode(body).map_err(|_| Error::Shape)?;
        let Message::Transformed { kind: 5, .. } = decoded.message else {
            return Err(Error::Phase);
        };
        let fields = Client5::decode(&decoded).map_err(|_| Error::Shape)?;
        fields
            .qualify_empty(&decoded, HOST_SELECTOR, peer_token)
            .map_err(|_| Error::Echo)?;
        let host6 = Host6 {
            client_echo: fields.client_fresh,
            host_fresh: self.random.next_u32(),
        }
        .new_body(fields.client_selector)
        .map_err(|_| Error::Shape)?;
        self.phase = Phase::Sequenced(Box::new(Sequenced {
            client5: body.to_vec(),
            host6: host6.clone(),
            inbound: HOST_SELECTOR,
            outbound: fields.client_selector,
            origin_ms: now_ms,
            history: ReceiveHistory::new(Sequence::new(0).unwrap(), 0),
            reports: PeerReports::new(Sequence::new(0).unwrap()),
            deliveries: delivery::Deliveries::default(),
            next_number: 1,
            next_frame: 0,
            assembler: Assembler::new(self.policy.max_frame_bits).map_err(|_| Error::Bound)?,
            pending_advertised: None,
            own_request: None,
            last_request_ms: None,
            last_control_ms: None,
        }));
        Ok(Received {
            event: Some(Event::Client5 { repeated: false }),
            send: vec![host6],
        })
    }

    fn sequenced_input(
        &mut self,
        body: &[u8],
        now_ms: u64,
        listener: &mut dyn Listener,
    ) -> Result<Received, Error> {
        let Phase::Sequenced(s) = &mut self.phase else {
            return Err(Error::Phase);
        };
        if body == s.client5.as_slice() {
            return Ok(Received {
                event: Some(Event::Client5 { repeated: true }),
                send: vec![s.host6.clone()],
            });
        }
        let envelope = ZeroTail::new(self.policy.max_body)
            .map_err(|_| Error::Bound)?
            .decode(body)
            .map_err(|_| Error::Shape)?;
        if envelope.kind() != SEQUENCED_KIND {
            return Err(Error::Phase);
        }
        if envelope.connection_selector() != s.inbound {
            return Err(Error::Selector);
        }
        let header = envelope.sequence().ok_or(Error::Shape)?;
        match s.history.check(header.number()) {
            Ok(_) => {}
            Err(AdvanceError::Repeated) => {
                self.stats.duplicates += 1;
                return Ok(Received {
                    event: Some(Event::Duplicate),
                    send: vec![],
                });
            }
            Err(AdvanceError::OutsideWindow) => {
                self.stats.out_of_window += 1;
                return Ok(Received {
                    event: Some(Event::OutOfWindow),
                    send: vec![],
                });
            }
        }
        // The peer's report on our sends. A stale or out-of-window report is
        // ignored rather than failing the input.
        let distance = header
            .acknowledgement()
            .value()
            .wrapping_sub(s.reports.frontier().value())
            & 1023;
        // A receipt cannot describe a sequence we have not sent. Retain every
        // accepted/nonaccepted result for the owner of a tracked frame.
        if distance <= outstanding(s)
            && let Ok(batch) = s
                .reports
                .consume(header.acknowledgement(), header.history())
        {
            s.deliveries.apply(batch);
        }
        let before = (s.history.frontier().value(), s.history.bits());
        let mut send = Vec::new();
        let (event, accepted) = match Route::decode(envelope.payload()).map_err(|_| Error::Shape)? {
            Route::Queued { advertised, body } => {
                self.stats.queued_inputs += 1;
                if advertised.is_some() {
                    s.pending_advertised = advertised;
                }
                let fragment = Fragment::decode(body).map_err(|_| Error::Shape)?;
                let bits = fragment.data().len();
                match s.assembler.push(fragment).map_err(|_| Error::Bound)? {
                    Outcome::Pending => (
                        Event::Queued {
                            bits,
                            complete: false,
                            accepted: true,
                        },
                        true,
                    ),
                    Outcome::Discarded => (
                        Event::Queued {
                            bits,
                            complete: false,
                            accepted: false,
                        },
                        false,
                    ),
                    Outcome::Complete(frame) => {
                        let accepted = listener.frame(Queued {
                            advertised: s.pending_advertised,
                            frame: frame.frame(),
                            data: frame.data(),
                        });
                        self.stats.frames_complete += 1;
                        self.stats.frames_accepted += usize::from(accepted);
                        (
                            Event::Queued {
                                bits,
                                complete: true,
                                accepted,
                            },
                            accepted,
                        )
                    }
                }
            }
            Route::Control {
                subtype: CONTROL_TIMING_REQUEST,
                timestamps: [Some(timestamp), None],
                body: rest,
            } if rest.is_empty() => {
                // The native reply writer runs before this input commits, so the
                // reply reports the frontier and history as they were.
                let reply = timing::Decoded::new(
                    s.outbound,
                    s.next_number,
                    before.0,
                    before.1,
                    Control::Reply {
                        echo: timestamp,
                        clock: elapsed_bits(s.origin_ms, now_ms),
                    },
                )
                .map_err(|_| Error::Shape)?
                .encode();
                s.deliveries.sent(s.next_number, 1);
                s.next_number = (s.next_number + 1) & 1023;
                self.stats.timing_replies_sent += 1;
                send.push(reply);
                (Event::TimingRequest, true)
            }
            Route::Control {
                subtype: CONTROL_TIMING_REPLY,
                timestamps: [Some(echo), Some(_clock)],
                body: rest,
            } if rest.is_empty() => {
                let matched = s.own_request.is_some_and(|(sent, _)| sent == echo);
                let round_trip_ms = matched.then(|| {
                    let sent_ms = s.own_request.map(|(_, ms)| ms).unwrap_or(now_ms);
                    now_ms.saturating_sub(sent_ms)
                });
                if matched {
                    s.own_request = None;
                    self.stats.timing_matched += 1;
                }
                (
                    Event::TimingReply {
                        matched,
                        round_trip_ms,
                    },
                    true,
                )
            }
            Route::Control {
                subtype: CONTROL_EMPTY,
                body: rest,
                ..
            } if rest.is_empty() => {
                self.stats.empty_controls_received += 1;
                (Event::EmptyControl, true)
            }
            Route::Control { subtype, .. } => (Event::UnknownControl(subtype), false),
        };
        s.history
            .commit(header.number(), accepted)
            .map_err(|_| Error::Shape)?;
        // First timing request once the client's queued data starts.
        if matches!(event, Event::Queued { .. }) && s.last_request_ms.is_none() {
            send.push(Self::timing_request(s, now_ms));
            self.stats.timing_requests_sent += 1;
        }
        Ok(Received {
            event: Some(event),
            send,
        })
    }

    fn timing_request(s: &mut Sequenced, now_ms: u64) -> Vec<u8> {
        let timestamp = elapsed_bits(s.origin_ms, now_ms);
        let body = timing::Decoded::new(
            s.outbound,
            s.next_number,
            s.history.frontier().value(),
            s.history.bits(),
            Control::Request { timestamp },
        )
        .expect("selector and sequences are in range")
        .encode();
        s.deliveries.sent(s.next_number, 1);
        s.next_number = (s.next_number + 1) & 1023;
        s.own_request = Some((timestamp, now_ms));
        s.last_request_ms = Some(now_ms);
        body
    }

    /// Timers: periodic timing requests and the empty control under window
    /// pressure. Returns bodies to send.
    pub fn poll(&mut self, now_ms: u64) -> Result<Vec<Vec<u8>>, Error> {
        self.clock(now_ms)?;
        let policy = self.policy;
        let Phase::Sequenced(s) = &mut self.phase else {
            return Ok(vec![]);
        };
        let mut send = Vec::new();
        if let Some(last) = s.last_request_ms
            && now_ms.saturating_sub(last) >= policy.timing_interval_ms
        {
            send.push(Self::timing_request(s, now_ms));
            self.stats.timing_requests_sent += 1;
        }
        if outstanding(s) >= WINDOW - 1
            && s.last_control_ms
                .is_none_or(|last| now_ms.saturating_sub(last) >= policy.control_interval_ms)
        {
            let body = sequenced(
                s.outbound,
                s.next_number,
                s.history.frontier().value(),
                s.history.bits(),
                &control_payload(CONTROL_EMPTY, &[]),
            );
            s.deliveries.sent(s.next_number, 1);
            s.next_number = (s.next_number + 1) & 1023;
            s.last_control_ms = Some(now_ms);
            self.stats.empty_controls_sent += 1;
            send.push(body);
        }
        self.stats.sent += send.len();
        Ok(send)
    }

    /// Send one complete handler frame as a single queued fragment with the
    /// advertisement absent .
    pub fn send_frame(&mut self, frame: BitSpan<'_>) -> Result<Vec<u8>, Error> {
        let mut bodies = self.send_frame_fragments(frame, usize::MAX)?;
        if bodies.len() != 1 {
            return Err(Error::Bound);
        }
        Ok(bodies.remove(0))
    }

    /// Reserve a bounded receipt for all fragments of this handler frame.
    /// The caller must release it on completion, cancellation or deadline.
    pub fn send_tracked_frame(
        &mut self,
        frame: BitSpan<'_>,
        fragment_bits: usize,
    ) -> Result<(delivery::Ticket, Vec<Vec<u8>>), Error> {
        let Phase::Sequenced(s) = &self.phase else {
            return Err(Error::Phase);
        };
        s.deliveries.available()?;
        let first = s.next_number;
        let bodies = self.send_frame_fragments(frame, fragment_bits)?;
        let Phase::Sequenced(s) = &mut self.phase else {
            unreachable!()
        };
        let ticket = s.deliveries.insert(first, bodies.len());
        Ok((ticket, bodies))
    }

    pub fn delivery_status(&self, ticket: delivery::Ticket) -> Option<delivery::Status> {
        let Phase::Sequenced(s) = &self.phase else {
            return None;
        };
        s.deliveries.status(ticket)
    }

    /// Cancelling a pending receipt does not undo bytes already sent.
    pub fn release_delivery(&mut self, ticket: delivery::Ticket) -> Option<delivery::Status> {
        let Phase::Sequenced(s) = &mut self.phase else {
            return None;
        };
        s.deliveries.remove(ticket)
    }

    /// Send one complete handler frame as queued fragments of at most
    /// `fragment_bits` data bits each (same frame number, ordinals 0.., the
    /// last flagged final), the advertisement absent. The original hosts split
    /// their Ghost frames into 8,056-bit fragments in 1,024-byte bodies
    /// ([`_FRAGMENT_BITS`]). All fragments need window room at once;
    /// otherwise nothing is sent and the window error is returned.
    pub fn send_frame_fragments(
        &mut self,
        frame: BitSpan<'_>,
        fragment_bits: usize,
    ) -> Result<Vec<Vec<u8>>, Error> {
        let Phase::Sequenced(s) = &mut self.phase else {
            return Err(Error::Phase);
        };
        let fragment_bits =
            fragment_bits.clamp(1, nfs_protocol::world::fragment::MAX_FRAGMENT_BITS);
        let count = frame.len().div_ceil(fragment_bits).max(1);
        if count > 64 {
            return Err(Error::Bound);
        }
        if usize::from(outstanding(s)) + count > usize::from(WINDOW) - 1 {
            return Err(Error::Window);
        }
        let mut bodies = Vec::with_capacity(count);
        for ordinal in 0..count {
            let start = ordinal * fragment_bits;
            let len = (frame.len() - start).min(fragment_bits);
            let data = frame.slice(start, len).map_err(|_| Error::Bound)?;
            let mut payload = BitWriter::new();
            payload
                .put(0, 1)
                .put(0, 1)
                .put(u64::from(s.next_frame), 16)
                .put(ordinal as u64, 6)
                .put(len as u64, 15)
                .put_bool(ordinal + 1 == count)
                .put_span(data);
            let body = sequenced(
                s.outbound,
                (s.next_number + ordinal as u16) & 1023,
                s.history.frontier().value(),
                s.history.bits(),
                &payload,
            );
            if body.len() > self.policy.max_body {
                return Err(Error::Bound);
            }
            bodies.push(body);
        }
        s.deliveries.sent(s.next_number, count);
        s.next_number = (s.next_number + count as u16) & 1023;
        s.next_frame = s.next_frame.wrapping_add(1);
        self.stats.sent += count;
        Ok(bodies)
    }
}

#[cfg(test)]
mod tests;
