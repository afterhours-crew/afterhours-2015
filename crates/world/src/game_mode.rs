// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Game-mode remote calls of an owned event (E994/E995). Argument layouts follow the
//! client's own method readers: every state takes a participant reference for its
//! enter (method 0) and exit (method 1); the countdown's method 2 carries the start
//! time; the waypoints state's method 2 carries a checkpoint result and method 3 the
//! participant's car ID; the DNF transition's method 0 carries a deadline. Times are
//! f64 seconds on the host replication clock. Which state a selector names and when
//! a call is due belong to the caller. The same envelope carries the event's scene
//! calls: a checkpoint volume's enable (method 0) and disable (method 1), a client
//! notification's set (0) and clear (1), and the launcher's launched-game-mode call
//! (method 4 with the game mode as the second reference).
use crate::bits::BitWriter;
pub mod authored;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile, Serial},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Bound,
    Shape,
}

/// Inline arguments after the 32-bit method index. Times are held as IEEE-754 bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arguments {
    None,
    /// One 32-bit value: a car ID, a local position notification, a racer count.
    Word(u32),
    /// Volume-participant-set method 0: a full word followed by one boolean.
    /// Preserve both fields without assigning them unverified meanings.
    VolumeEntry {
        value: u32,
        flag: bool,
    },
    /// Event-info collector method 0: position and membership in the active
    /// standings. Clearing membership removes the participant from the client's
    /// event-info service; it is not padding after the position.
    EventPosition {
        position: u32,
        active: bool,
    },
    /// Two 32-bit reads, high word first: the bits of an f64 time.
    Time(u64),
    /// ScoreKeeper method 0 (`rpcInformClientOfScoreAward`, client handler
    /// RVA 0x042c9fe0): the participant's running total in that keeper,
    /// then this award's amount (official E1026 923.725 s: s58 72, 39).
    ScoreAward {
        total: u32,
        amount: u32,
    },
    /// Waypoints method 2 (client reader `0x14445e5d0`): passed and next checkpoint
    /// ordinals, the bits of the f32 race time in seconds and the lap.
    Progress {
        passed: u32,
        next: u32,
        seconds: u32,
        lap: u32,
    },
    /// Message-manager methods 0 (messageRestored) and 2 (messageAdded),
    /// client readers `0x14445a8b0`/`0x14445a7d0`: the asset reference (mode
    /// 2, an 11-bit registry bundle, a 12-bit flat index), an 8-bit
    /// attachment length (no attachment bytes here) and the message flag.
    Message {
        bundle: u16,
        flat: u16,
        flag: bool,
    },
}
impl Arguments {
    pub fn time(seconds: f64) -> Self {
        Self::Time(seconds.to_bits())
    }
}

/// A host call on one serializer of an entity or scene.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Call {
    pub target: u16,
    pub selector: u16,
    pub serial: Serial,
    pub method: u32,
    /// The second reference: a participant, or the game mode for a launcher.
    pub reference: Option<u16>,
    pub arguments: Arguments,
}
impl Call {
    pub fn objects(&self) -> Vec<u16> {
        std::iter::once(self.target).chain(self.reference).collect()
    }
    pub fn encode(&self) -> Result<BitWriter, Error> {
        if self.objects().iter().any(|id| *id == 0 || *id > 8191) || self.selector > 511 {
            return Err(Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload
            .put(self.selector.into(), 9)
            .put(self.serial.value().into(), 10)
            .put(self.method.into(), 32);
        match self.arguments {
            Arguments::None => (),
            Arguments::Word(v) => {
                payload.put(v.into(), 32);
            }
            Arguments::VolumeEntry { value, flag } => {
                if self.method != 0 || self.reference.is_none() {
                    return Err(Error::Shape);
                }
                payload.put(value.into(), 32).put(u64::from(flag), 1);
            }
            Arguments::EventPosition { position, active } => {
                // The collector's initial-state snapshot stores positions in
                // eight bits, even though its remote update reads a full word.
                if position > u32::from(u8::MAX) {
                    return Err(Error::Bound);
                }
                payload.put(position.into(), 32).put(u64::from(active), 1);
            }
            Arguments::Time(bits) => {
                let seconds = f64::from_bits(bits);
                if !seconds.is_finite() || seconds < 0. {
                    return Err(Error::Shape);
                }
                payload.put(bits >> 32, 32).put(bits & 0xffff_ffff, 32);
            }
            Arguments::ScoreAward { total, amount } => {
                if self.method != 0 || self.reference.is_none() {
                    return Err(Error::Shape);
                }
                payload.put(total.into(), 32).put(amount.into(), 32);
            }
            Arguments::Progress {
                passed,
                next,
                seconds,
                lap,
            } => {
                let value = f32::from_bits(seconds);
                if !value.is_finite() || value < 0. {
                    return Err(Error::Shape);
                }
                payload
                    .put(passed.into(), 32)
                    .put(next.into(), 32)
                    .put(seconds.into(), 32)
                    .put(lap.into(), 32);
            }
            Arguments::Message { bundle, flat, flag } => {
                if !matches!(self.method, 0 | 2) || self.reference.is_some() {
                    return Err(Error::Shape);
                }
                if bundle == 0 || bundle > 2047 || flat > 4095 {
                    return Err(Error::Bound);
                }
                payload
                    .put(2, 2)
                    .put(bundle.into(), 11)
                    .put(flat.into(), 12)
                    .put(0, 8)
                    .put(u64::from(flag), 1);
            }
        }
        payload.align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(self.objects().len() as u64, 8);
        for id in self.objects() {
            body.put(id.into(), 13);
        }
        body.put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
}

/// Client method 0 on the game-mode root with the participant: the request to leave
/// the game mode after a restart or leave choice, and the final leave after the outro.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Leave {
    pub game_mode: u16,
    pub participant: u16,
}
impl Leave {
    pub fn decode(body: BitSpan<'_>) -> Result<Option<Self>, Error> {
        Ok(RootRequest::decode(body)?.and_then(|request| {
            (request.kind == RootRequestKind::Leave).then_some(Self {
                game_mode: request.game_mode,
                participant: request.participant,
            })
        }))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootRequestKind {
    Leave,
    /// Method 8 selects the first occupied registered state other than Final.
    /// It does not remove the game mode or acknowledge client shutdown.
    GoToFinal,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootRequest {
    pub game_mode: u16,
    pub participant: u16,
    pub kind: RootRequestKind,
}
impl RootRequest {
    pub fn decode(body: BitSpan<'_>) -> Result<Option<Self>, Error> {
        let e = Envelope::decode(
            body,
            Limits {
                max_input_bits: 160,
                max_references: 2,
                max_payload_bytes: 8,
            },
        )
        .map_err(|_| Error::Shape)?;
        let r = e
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if r.selector() != 0 || !matches!(r.method_index(), 0 | 8) {
            return Ok(None);
        }
        let &[game_mode, participant] = e.references() else {
            return Err(Error::Shape);
        };
        if e.words() != [0, 0] || !e.remaining().is_empty() || r.arguments().len() != 7 {
            return Err(Error::Shape);
        }
        if [game_mode, participant]
            .into_iter()
            .any(|id| id == 0 || id > 8191)
        {
            return Err(Error::Bound);
        }
        Ok(Some(Self {
            game_mode,
            participant,
            kind: if r.method_index() == 0 {
                RootRequestKind::Leave
            } else {
                RootRequestKind::GoToFinal
            },
        }))
    }
}

/// A client crossing report on a scene volume: method 2 (inside) or 3 (outside) with
/// the scene and car references, the f64 crossing time and seven padding bits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossing {
    pub scene: u16,
    pub selector: u16,
    pub vehicle: u16,
    pub inside: bool,
    pub seconds: f64,
}
impl Crossing {
    pub fn decode(body: BitSpan<'_>) -> Result<Option<Self>, Error> {
        let e = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let r = e
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if !matches!(r.method_index(), 2 | 3) {
            return Ok(None);
        }
        let &[scene, vehicle] = e.references() else {
            return Ok(None);
        };
        if e.words() != [0, 0] || !e.remaining().is_empty() || r.arguments().len() != 71 {
            return Ok(None);
        }
        let high = r.arguments().read_u32(0, 32).map_err(|_| Error::Shape)?;
        let low = r.arguments().read_u32(32, 32).map_err(|_| Error::Shape)?;
        let seconds = f64::from_bits((u64::from(high) << 32) | u64::from(low));
        if !seconds.is_finite() || seconds < 0. {
            return Err(Error::Shape);
        }
        Ok(Some(Self {
            scene,
            selector: r.selector(),
            vehicle,
            inside: r.method_index() == 2,
            seconds,
        }))
    }
}

#[cfg(test)]
mod tests;
