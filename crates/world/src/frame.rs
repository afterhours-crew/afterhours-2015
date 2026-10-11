// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::bits::BitWriter;
use nfs_protocol::world::{
    BitSpan,
    ghost::{self, Prefix},
};
use std::fmt;

pub const HANDLERS: usize = 6;
pub const TIME_SYNC: u8 = 0;
pub const MESSAGES: u8 = 1;
pub const MOVEMENT: u8 = 2;
pub const GHOST: u8 = 3;
pub const EX_CHUNK: u8 = 77;
pub const EX_CREATE_PLAYER: u8 = 34;
pub const EX_TIME: u8 = 80;
pub const EX_SENTINEL: u8 = 56;
pub const REGISTRY_SIZE: u8 = 107;
pub const INITIAL_CHANNEL_STATE: u16 = 1728;
pub const MAX_GROUPS: usize = 15;
pub const MAX_MESSAGES: usize = 64;
pub const MAX_STRING_BYTES: usize = 1023;
pub const MAX_MOVEMENT_RECORDS: usize = 64;
pub const MAX_RPC_REFERENCES: usize = 255;
pub const MAX_GHOST_RECORDS: u32 = 8192;
pub const MAX_GHOST_DELETIONS: usize = 1024;
pub const MAX_COLLECTION_RECORDS: usize = 1024;
pub const EX_STATE: u8 = 88;
pub const EX_COLLECTION: u8 = 63;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Truncated,
    Unsupported(u8),
    File(crate::files::Error),
    Bound,
    Shape,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "handler frame {self:?}")
    }
}
impl std::error::Error for Error {}

impl From<nfs_protocol::world::Error> for Error {
    fn from(_: nfs_protocol::world::Error) -> Self {
        Self::Truncated
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    FromClient,
    FromHost,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeSyncInit {
    pub flag: bool,
    pub rate: f32,
    pub retries: i8,
    pub interval: u8,
    pub window: f32,
}

impl TimeSyncInit {
    pub const DEFAULT: Self = Self {
        flag: false,
        rate: 60.0,
        retries: 16,
        interval: 1,
        window: 5.0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimeSync {
    Sample(u64),
    Init(TimeSyncInit),
    Times([u64; 3]),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message<'a> {
    Chunk {
        ordinal: u8,
        target: Option<u8>,
        last: bool,
        bytes: Vec<u8>,
    },
    CreatePlayer {
        name: Vec<u8>,
        flag: bool,
        slot: u8,
    },
    Time {
        flag: bool,
        tick: u32,
        time: f32,
    },
    Sentinel(i32),
    Empty,
    State(u8),
    LauncherEnabled(crate::launchers::Enabled),
    ParticipantNotification(crate::participants::Notification),
    GarageBinding(crate::garage::Binding),
    ActorBinding(crate::actors::Binding),
    VehicleBinding(crate::garage::vehicle::Binding),
    GaragePresence(crate::garage::presence::Notification),
    SequenceStop(crate::sequences::Stop),
    LogicEvent(crate::logic::Fire),
    Opaque {
        index: u8,
        body: BitSpan<'a>,
    },
}

impl Message<'_> {
    pub fn index(&self) -> u8 {
        match self {
            Self::Chunk { .. } => EX_CHUNK,
            Self::CreatePlayer { .. } => EX_CREATE_PLAYER,
            Self::Time { .. } => EX_TIME,
            Self::Sentinel(_) => EX_SENTINEL,
            Self::Empty => 26,
            Self::State(_) => EX_STATE,
            Self::LauncherEnabled(_)
            | Self::ParticipantNotification(_)
            | Self::GarageBinding(_)
            | Self::ActorBinding(_)
            | Self::VehicleBinding(_)
            | Self::GaragePresence(_)
            | Self::SequenceStop(_) => 47,
            Self::LogicEvent(v) => v.index(),
            Self::Opaque { index, .. } => *index,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group<'a> {
    pub channel: u8,
    pub sequence: Option<u8>,
    pub messages: Vec<Message<'a>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Messages<'a> {
    pub state: Option<u16>,
    pub groups: Vec<Group<'a>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovementRecord<'a> {
    pub id: u16,
    pub component: u8,
    pub data: BitSpan<'a>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Movement<'a> {
    pub header: Option<BitSpan<'a>>,
    pub records: Vec<MovementRecord<'a>>,
}

pub struct Ghost<'a> {
    pub prefix: Prefix<'a>,
    pub records: BitSpan<'a>,
}

impl fmt::Debug for Ghost<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ghost")
            .field("records", &self.prefix.remaining_records())
            .field("bits", &self.records.len())
            .finish()
    }
}

#[derive(Debug)]
pub struct Frame<'a> {
    pub mask: u8,
    pub time_sync: Option<TimeSync>,
    pub messages: Option<Messages<'a>>,
    pub movement: Option<Movement<'a>>,
    pub ghost: Option<Ghost<'a>>,
    pub files: Vec<(u8, crate::files::Record)>,
    pub complete: bool,
    pub rest: usize,
}

struct Reader<'a> {
    span: BitSpan<'a>,
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, width: u8) -> Result<u32, Error> {
        let v = self.span.read_u32(self.pos, width)?;
        self.pos += usize::from(width);
        Ok(v)
    }
    fn take64(&mut self, width: u8) -> Result<u64, Error> {
        if width <= 32 {
            return self.take(width).map(u64::from);
        }
        let high = self.take(width - 32)?;
        let low = self.take(32)?;
        Ok((u64::from(high) << 32) | u64::from(low))
    }
    fn bytes(&mut self, count: usize) -> Result<Vec<u8>, Error> {
        (0..count).map(|_| self.take(8).map(|v| v as u8)).collect()
    }
    fn slice(&mut self, bits: usize) -> Result<BitSpan<'a>, Error> {
        let s = self.span.slice(self.pos, bits)?;
        self.pos += bits;
        Ok(s)
    }
    fn rest(&self) -> Result<BitSpan<'a>, Error> {
        Ok(self.span.after(self.pos)?)
    }
}

fn string(r: &mut Reader<'_>) -> Result<Vec<u8>, Error> {
    let len = r.take(10)? as usize;
    if len > MAX_STRING_BYTES {
        return Err(Error::Bound);
    }
    r.bytes(len)
}

fn message<'a>(r: &mut Reader<'a>, index: u8) -> Result<Message<'a>, Error> {
    Ok(match index {
        EX_CHUNK => {
            let ordinal = r.take(8)? as u8;
            let target = if ordinal == 0 {
                let t = r.take(7)? as u8;
                if t >= REGISTRY_SIZE {
                    return Err(Error::Shape);
                }
                Some(t)
            } else {
                None
            };
            let last = r.take(1)? != 0;
            let count = if last { r.take(5)? as usize + 1 } else { 32 };
            Message::Chunk {
                ordinal,
                target,
                last,
                bytes: r.bytes(count)?,
            }
        }
        EX_CREATE_PLAYER => {
            let name = string(r)?;
            let flag = r.take(1)? != 0;
            let slot = r.take(8)? as u8;
            if slot > 7 && slot != 255 {
                return Err(Error::Shape);
            }
            Message::CreatePlayer { name, flag, slot }
        }
        EX_TIME => Message::Time {
            flag: r.take(1)? != 0,
            tick: r.take(32)?,
            time: f32::from_bits(r.take(32)?),
        },
        EX_SENTINEL => Message::Sentinel(r.take(32)? as i32),
        26 => Message::Empty,
        70 => Message::Opaque {
            index,
            body: r.slice(32)?,
        },
        8 | 60 | 95 => {
            let start = r.pos;
            r.take(32)?;
            let count = r.take(32)? as usize;
            if count > MAX_MESSAGES * 64 {
                return Err(Error::Bound);
            }
            r.slice(count * 16)?;
            let body = r.span.slice(start, r.pos - start)?;
            Message::Opaque { index, body }
        }
        EX_STATE => {
            let value = r.take(4)? as u8;
            if value > 10 {
                return Err(Error::Shape);
            }
            Message::State(value)
        }
        EX_COLLECTION => {
            let start = r.pos;
            collection(r, |_| {})?;
            let body = r.span.slice(start, r.pos - start)?;
            Message::Opaque { index, body }
        }
        35 => {
            let start = r.pos;
            if r.take(5)? > 17 {
                return Err(Error::Shape);
            }
            string(r)?;
            let body = r.span.slice(start, r.pos - start)?;
            Message::Opaque { index, body }
        }
        47 => {
            let start = r.pos;
            r.take64(64)?;
            let references = r.take(8)? as usize;
            if references > MAX_RPC_REFERENCES {
                return Err(Error::Bound);
            }
            r.slice(references * 13)?;
            let size = r.take(9)? as usize;
            if size > 256 {
                return Err(Error::Shape);
            }
            r.slice(size * 8)?;
            let body = r.span.slice(start, r.pos - start)?;
            Message::Opaque { index, body }
        }
        82 | 86 | 91 => {
            let start = r.pos;
            let size = r.take(32)? as usize;
            if size < 32 {
                return Err(Error::Shape);
            }
            r.slice(size - 32)?;
            let body = r.span.slice(start, r.pos - start)?;
            Message::Opaque { index, body }
        }
        89 => {
            let start = r.pos;
            r.take64(64)?;
            if r.take(32)? != 0 {
                return Err(Error::Unsupported(index));
            }
            let body = r.span.slice(start, r.pos - start)?;
            Message::Opaque { index, body }
        }
        other => return Err(Error::Unsupported(other)),
    })
}

fn collection(r: &mut Reader<'_>, mut each: impl FnMut(u16)) -> Result<(), Error> {
    r.take64(34)?;
    let count = r.take(32)? as usize;
    if count > MAX_COLLECTION_RECORDS {
        return Err(Error::Bound);
    }
    for _ in 0..count {
        let handle = r.take(16)? as u16;
        r.slice(16 + 16 + 32 + 8 + 1)?;
        if r.take(3)? > 6 {
            return Err(Error::Shape);
        }
        r.slice(32 + 1)?;
        if r.take(2)? > 2 {
            return Err(Error::Shape);
        }
        r.take(32)?;
        string(r)?;
        each(handle);
    }
    Ok(())
}

pub fn collection_handles(bytes: &[u8]) -> Option<Vec<u16>> {
    let span = BitSpan::new(bytes, 0, bytes.len() * 8).ok()?;
    let mut r = Reader { span, pos: 0 };
    let mut handles = Vec::new();
    collection(&mut r, |h| handles.push(h)).ok()?;
    Some(handles)
}

fn messages<'a>(r: &mut Reader<'a>) -> Result<Messages<'a>, Error> {
    let count = r.take(4)? as usize;
    let state = if r.take(1)? != 0 {
        Some(r.take(12)? as u16)
    } else {
        None
    };
    let mut groups = Vec::new();
    let mut decoded = 0;
    while decoded < count {
        if r.take(1)? == 0 {
            return Err(Error::Shape);
        }
        let channel = r.take(3)? as u8;
        if channel >= 4 {
            return Err(Error::Shape);
        }
        let sequence = if channel.is_multiple_of(2) {
            Some(r.take(7)? as u8)
        } else {
            None
        };
        let mut group = Group {
            channel,
            sequence,
            messages: Vec::new(),
        };
        loop {
            let index = r.take(7)? as u8;
            if index >= REGISTRY_SIZE {
                return Err(Error::Shape);
            }
            group.messages.push(message(r, index)?);
            decoded += 1;
            if r.take(1)? == 0 {
                break;
            }
            if group.messages.len() >= MAX_GROUPS || decoded >= count {
                return Err(Error::Shape);
            }
        }
        groups.push(group);
        if groups.len() > MAX_GROUPS {
            return Err(Error::Bound);
        }
    }
    Ok(Messages { state, groups })
}

fn movement<'a>(r: &mut Reader<'a>, direction: Direction) -> Result<Movement<'a>, Error> {
    let mut records = Vec::new();
    let header = match direction {
        Direction::FromClient => {
            let header = r.slice(96)?;
            let count = r.take(6)? as usize;
            for _ in 0..count {
                let id = r.take(16)? as u16;
                let component = r.take(4)? as u8;
                let bits = r.take(16)? as usize;
                records.push(MovementRecord {
                    id,
                    component,
                    data: r.slice(bits)?,
                });
            }
            Some(header)
        }
        Direction::FromHost => {
            let count = r.take(8)? as usize;
            if count > MAX_MOVEMENT_RECORDS {
                return Err(Error::Bound);
            }
            for _ in 0..count {
                let id = r.take(16)? as u16;
                let n = r.take(4)? as usize;
                records.push(MovementRecord {
                    id,
                    component: 0,
                    data: r.slice(4 * n)?,
                });
            }
            None
        }
    };
    Ok(Movement { header, records })
}

pub fn parse(data: BitSpan<'_>, direction: Direction) -> Result<Frame<'_>, Error> {
    parse_inner(data, direction, [None; 2], false)
}

pub fn parse_with_files(
    data: BitSpan<'_>,
    direction: Direction,
    sizes: [Option<usize>; 2],
) -> Result<Frame<'_>, Error> {
    parse_inner(data, direction, sizes, false)
}

pub fn parse_prefix(data: BitSpan<'_>, direction: Direction) -> Result<Frame<'_>, Error> {
    parse_inner(data, direction, [None; 2], true)
}

fn parse_inner(
    data: BitSpan<'_>,
    direction: Direction,
    sizes: [Option<usize>; 2],
    prefix_only: bool,
) -> Result<Frame<'_>, Error> {
    let mut r = Reader { span: data, pos: 0 };
    let mask = r.take(6)? as u8;
    let mut frame = Frame {
        mask,
        time_sync: None,
        messages: None,
        movement: None,
        ghost: None,
        files: Vec::new(),
        complete: true,
        rest: 6,
    };
    for handler in 0..HANDLERS as u8 {
        if mask & (1 << handler) == 0 {
            continue;
        }
        if prefix_only && handler > MESSAGES {
            frame.complete = false;
            return Ok(frame);
        }
        if handler <= MESSAGES {
            frame.rest = r.pos;
        }
        match handler {
            TIME_SYNC => {
                frame.time_sync = Some(match direction {
                    Direction::FromClient => TimeSync::Sample(r.take64(38)?),
                    Direction::FromHost => {
                        if r.take(1)? != 0 {
                            TimeSync::Init(TimeSyncInit {
                                flag: r.take(1)? != 0,
                                rate: f32::from_bits(r.take(32)?),
                                retries: r.take(8)? as u8 as i8,
                                interval: r.take(4)? as u8,
                                window: f32::from_bits(r.take(32)?),
                            })
                        } else {
                            TimeSync::Times([r.take64(38)?, r.take64(38)?, r.take64(38)?])
                        }
                    }
                });
                frame.rest = r.pos;
            }
            MESSAGES => {
                frame.messages = Some(messages(&mut r)?);
                frame.rest = r.pos;
            }
            MOVEMENT => frame.movement = Some(movement(&mut r, direction)?),
            GHOST => {
                let rest = r.rest()?;
                let prefix = Prefix::decode(
                    rest,
                    ghost::Profile::NFS16_92AA6FF4,
                    ghost::Limits {
                        max_input_bits: rest.len(),
                        max_records: MAX_GHOST_RECORDS,
                        max_deletions: MAX_GHOST_DELETIONS,
                    },
                )
                .map_err(|_| Error::Truncated)?;
                let records = prefix.remaining();
                if prefix.remaining_records() == 0 {
                    r.pos += rest.len() - records.len();
                    frame.ghost = Some(Ghost {
                        prefix,
                        records: records.slice(0, 0)?,
                    });
                    continue;
                }
                frame.ghost = Some(Ghost { prefix, records });
                frame.complete = mask >> (handler + 1) == 0;
                return Ok(frame);
            }
            4 | 5 => {
                let (record, consumed) =
                    crate::files::decode(r.rest()?, sizes[usize::from(handler - 4)])
                        .map_err(Error::File)?;
                r.pos += consumed;
                frame.files.push((handler, record));
            }
            other => return Err(Error::Unsupported(other)),
        }
    }
    if !frame.files.is_empty() && r.rest()?.len() >= 8 {
        return Err(Error::Shape);
    }
    Ok(frame)
}

pub const CHUNK_BYTES: usize = 32;
pub const CHUNKS_PER_FRAME: usize = 15;

pub fn chunks(target: u8, bytes: &[u8]) -> Option<Vec<Message<'static>>> {
    if bytes.is_empty() || bytes.len() > CHUNK_BYTES * 64 || target >= REGISTRY_SIZE {
        return None;
    }
    let count = bytes.len().div_ceil(CHUNK_BYTES);
    Some(
        bytes
            .chunks(CHUNK_BYTES)
            .enumerate()
            .map(|(i, part)| Message::Chunk {
                ordinal: i as u8,
                target: (i == 0).then_some(target),
                last: i + 1 == count,
                bytes: part.to_vec(),
            })
            .collect(),
    )
}

pub fn reassemble(chunks: &[&Message<'_>]) -> Option<(u8, Vec<u8>)> {
    let mut target = None;
    let mut bytes = Vec::new();
    for (expected, chunk) in chunks.iter().enumerate() {
        let Message::Chunk {
            ordinal,
            target: first,
            last,
            bytes: part,
        } = chunk
        else {
            return None;
        };
        if usize::from(*ordinal) != expected || (expected == 0) != first.is_some() {
            return None;
        }
        if let Some(t) = first {
            target = Some(*t);
        }
        bytes.extend_from_slice(part);
        if *last != (expected + 1 == chunks.len()) {
            return None;
        }
    }
    Some((target?, bytes))
}

pub const EX_CLIENT_STATE: u8 = EX_STATE;
pub const EX_LEVEL_READY: u8 = 70;
pub const EX_SUBLEVEL_REPORT: u8 = 8;

pub fn sublevel_report_ids(body: BitSpan<'_>) -> Vec<u16> {
    let Ok(count) = body.read_u32(32, 32) else {
        return Vec::new();
    };
    (0..count as usize)
        .filter_map(|i| body.read_u32(64 + 16 * i, 16).ok().map(|v| v as u16))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimeSyncOut {
    Init(TimeSyncInit),
    Times([u64; 3]),
}

#[derive(Clone, Debug, Default)]
pub struct Builder {
    time_sync: Option<TimeSyncOut>,
    messages: Option<(Option<u16>, Vec<Group<'static>>)>,
    movement: Option<Vec<(u16, Vec<u8>)>>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn time_sync_init(mut self, init: TimeSyncInit) -> Self {
        self.time_sync = Some(TimeSyncOut::Init(init));
        self
    }

    pub fn time_sync_times(mut self, times: [u64; 3]) -> Self {
        self.time_sync = Some(TimeSyncOut::Times(times));
        self
    }

    pub fn messages(mut self, state: Option<u16>, groups: Vec<Group<'static>>) -> Self {
        self.messages = Some((state, groups));
        self
    }

    /// Host AuthoritativeMove section: per object, the component selectors the
    /// client is to report. Official hosts list every client-owned car with
    /// selector 0 in each regular frame; the client starts its movement records
    /// for an object 13-16 ms after the object's first listing (E101).
    pub fn movement(mut self, grants: Vec<(u16, Vec<u8>)>) -> Self {
        self.movement = Some(grants);
        self
    }

    fn put_message(w: &mut BitWriter, m: &Message<'_>) -> Result<(), Error> {
        w.put(u64::from(m.index()), 7);
        match m {
            Message::Chunk {
                ordinal,
                target,
                last,
                bytes,
            } => {
                w.put(u64::from(*ordinal), 8);
                match (ordinal, target) {
                    (0, Some(t)) => {
                        w.put(u64::from(*t), 7);
                    }
                    (0, None) | (_, Some(_)) => return Err(Error::Shape),
                    _ => {}
                }
                w.put_bool(*last);
                if *last {
                    if bytes.is_empty() || bytes.len() > 32 {
                        return Err(Error::Shape);
                    }
                    w.put(bytes.len() as u64 - 1, 5);
                } else if bytes.len() != 32 {
                    return Err(Error::Shape);
                }
                w.put_bytes(bytes);
            }
            Message::CreatePlayer { name, flag, slot } => {
                if name.len() > MAX_STRING_BYTES || (*slot > 7 && *slot != 255) {
                    return Err(Error::Shape);
                }
                w.put(name.len() as u64, 10).put_bytes(name);
                w.put_bool(*flag).put(u64::from(*slot), 8);
            }
            Message::Time { flag, tick, time } => {
                w.put_bool(*flag).put(u64::from(*tick), 32).put_f32(*time);
            }
            Message::Sentinel(v) => {
                w.put(u64::from(*v as u32), 32);
            }
            Message::Empty => {}
            Message::State(value) => {
                if *value > 10 {
                    return Err(Error::Shape);
                }
                w.put(u64::from(*value), 4);
            }
            Message::Opaque { body, .. } => {
                w.put_span(*body);
            }
            Message::LauncherEnabled(value) => {
                w.put_span(value.encode().map_err(|_| Error::Shape)?.span());
            }
            Message::ParticipantNotification(value) => {
                w.put_span(value.encode().map_err(|_| Error::Shape)?.span());
            }
            Message::GarageBinding(value) => {
                w.put_span(value.encode().map_err(|_| Error::Shape)?.span());
            }
            Message::ActorBinding(value) => {
                w.put_span(value.encode().map_err(|_| Error::Shape)?.span());
            }
            Message::VehicleBinding(value) => {
                w.put_span(value.encode().map_err(|_| Error::Shape)?.span());
            }
            Message::GaragePresence(value) => {
                w.put_span(value.encode().map_err(|_| Error::Shape)?.span());
            }
            Message::SequenceStop(value) => {
                w.put_span(value.encode().map_err(|_| Error::Shape)?.span());
            }
            Message::LogicEvent(value) => {
                w.put_span(value.encode().map_err(|_| Error::Shape)?.span());
            }
        }
        Ok(())
    }

    pub fn mask(&self) -> u8 {
        u8::from(self.time_sync.is_some())
            | (u8::from(self.messages.is_some()) << MESSAGES)
            | (u8::from(self.movement.is_some()) << MOVEMENT)
    }

    pub fn build(&self) -> Result<BitWriter, Error> {
        let mut w = BitWriter::new();
        w.put(u64::from(self.mask()), 6);
        self.append(&mut w)?;
        w.align();
        Ok(w)
    }

    pub fn append(&self, w: &mut BitWriter) -> Result<(), Error> {
        match &self.time_sync {
            Some(TimeSyncOut::Init(init)) => {
                w.put(1, 1)
                    .put_bool(init.flag)
                    .put_f32(init.rate)
                    .put(u64::from(init.retries as u8), 8)
                    .put(u64::from(init.interval & 0xf), 4)
                    .put_f32(init.window);
            }
            Some(TimeSyncOut::Times(times)) => {
                w.put(0, 1);
                for t in times {
                    w.put(t & ((1 << 38) - 1), 38);
                }
            }
            None => {}
        }
        if let Some((state, groups)) = &self.messages {
            let count: usize = groups.iter().map(|g| g.messages.len()).sum();
            if count > 15 || groups.len() > MAX_GROUPS {
                return Err(Error::Bound);
            }
            w.put(count as u64, 4);
            w.put_bool(state.is_some());
            if let Some(s) = state {
                w.put(u64::from(*s), 12);
            }
            for group in groups {
                if group.messages.is_empty() || group.channel >= 4 {
                    return Err(Error::Shape);
                }
                w.put(1, 1).put(u64::from(group.channel), 3);
                if group.channel.is_multiple_of(2) {
                    w.put(u64::from(group.sequence.ok_or(Error::Shape)?), 7);
                }
                for (i, m) in group.messages.iter().enumerate() {
                    Self::put_message(w, m)?;
                    w.put_bool(i + 1 < group.messages.len());
                }
            }
        }
        if let Some(grants) = &self.movement {
            if grants.len() > MAX_MOVEMENT_RECORDS {
                return Err(Error::Bound);
            }
            w.put(grants.len() as u64, 8);
            for (id, selectors) in grants {
                if selectors.is_empty() || selectors.len() > 15 || selectors.iter().any(|s| *s > 15)
                {
                    return Err(Error::Shape);
                }
                w.put(u64::from(*id), 16).put(selectors.len() as u64, 4);
                for selector in selectors {
                    w.put(u64::from(*selector), 4);
                }
            }
        }
        Ok(())
    }
}

pub fn initializer(tick: u32, time: f32, sequence: u8) -> BitWriter {
    Builder::new()
        .time_sync_init(TimeSyncInit::DEFAULT)
        .messages(
            Some(INITIAL_CHANNEL_STATE),
            vec![Group {
                channel: 0,
                sequence: Some(sequence),
                messages: vec![
                    Message::Time {
                        flag: true,
                        tick,
                        time,
                    },
                    Message::Sentinel(-1),
                ],
            }],
        )
        .build()
        .expect("fixed shape")
}

#[cfg(test)]
mod tests;
