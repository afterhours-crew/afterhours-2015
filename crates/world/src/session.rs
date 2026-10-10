// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    application::{self, Application, DEFAULT_FRAGMENT_BITS, Listener, Policy, Random},
    frame,
    handshake::{HostHandshake, Ignored, Step},
    link::{self, Link},
    replication::section::{Bindings, Section},
    transport::Codec,
};
use nfs_protocol::world::BitSpan;
use std::fmt;

mod entry;
mod files;

fn rpc_message(rpc: crate::participants::HostRpc) -> frame::Message<'static> {
    use crate::participants::HostRpc;
    match rpc {
        HostRpc::Launcher(v) => frame::Message::LauncherEnabled(v),
        HostRpc::Participant(v) => frame::Message::ParticipantNotification(v),
        HostRpc::Garage(v) => frame::Message::GarageBinding(v),
        HostRpc::Actor(v) => frame::Message::ActorBinding(v),
        HostRpc::Vehicle(v) => frame::Message::VehicleBinding(v),
        HostRpc::GaragePresence(v) | HostRpc::SpawnOccupied(v) | HostRpc::LevelPoll(v) => {
            frame::Message::GaragePresence(v)
        }
        HostRpc::SequenceStop(v) => frame::Message::SequenceStop(v),
        HostRpc::Event(v) => frame::Message::LogicEvent(v),
    }
}

pub fn replication_frame(
    section: &Section,
    before: &[crate::participants::HostRpc],
    sequence: u8,
) -> Result<crate::bits::BitWriter, Error> {
    if before.is_empty() {
        return section.frame().map_err(Error::Replication);
    }
    if before.len() > 15 || sequence > 127 {
        return Err(Error::Application(application::Error::Bound));
    }
    let messages = frame::Builder::new().messages(
        None,
        vec![frame::Group {
            channel: 0,
            sequence: Some(sequence),
            messages: before.iter().copied().map(rpc_message).collect(),
        }],
    );
    let mut wire = crate::bits::BitWriter::new();
    wire.put(
        u64::from(messages.mask()) | (1 << frame::GHOST),
        frame::HANDLERS,
    );
    messages
        .append(&mut wire)
        .map_err(|_| Error::Application(application::Error::Shape))?;
    wire.put_span(section.encode().map_err(Error::Replication)?.span())
        .align();
    Ok(wire)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ids {
    pub client: u32,
    pub host: u32,
    pub host_index: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageName {
    Handshake,
    Running,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    HandshakeReply,
    Established,
    Ignored(Ignored),
    Link(link::Error),
    Application(application::Event),
    Initialized,
    Content {
        chunks: usize,
        remaining: usize,
    },
    TimeSynced,
    StateSent {
        registered: usize,
        reported: usize,
    },
    TimeSent,
    FirstPlayerEntered {
        player: u16,
    },
    Replication {
        records: usize,
        fragments: usize,
        remaining: usize,
    },
    ReplicationWaiting {
        level: u16,
    },
    RpcNotifications {
        count: usize,
        remaining: usize,
    },
    FileRecord {
        handler: u8,
        kind: u8,
    },
    FileComplete {
        handler: u8,
    },
    FileError {
        handler: u8,
        error: crate::files::sender::Error,
    },

    ApplicationError(application::Error),
    Closed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Output {
    pub send: Vec<Vec<u8>>,
    pub events: Vec<Event>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Stage,
    Link(link::Error),
    Application(application::Error),
    Replication(crate::replication::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "world session {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, Default)]
struct World {
    initialized: Option<u64>,
    next_ex_sequence: u8,
    acked_inputs: usize,
    descriptor_sent: Option<u64>,
    client_state_reported: bool,
    client_level_ready: bool,
    time_sample: Option<(u64, u64)>,
    reports: usize,
    last_report_ms: u64,
    reported: std::collections::BTreeSet<u16>,
    content_done_ms: Option<u64>,
    state_reports: usize,
    state_sent: Option<(u64, usize)>,
    time_sent: Option<(u64, usize)>,
    client_level_linked: bool,
    client_time_ready: bool,
    player_entry: entry::Entry,
    last_replication_ms: Option<u64>,
    blocked_level: Option<u16>,
}

#[derive(Clone, Debug, Default)]
struct Seen {
    state_report: bool,
    state_reports: usize,
    level_ready: bool,
    reports: usize,
    reported: Vec<u16>,
    time_sample: Option<u64>,
    level_linked: bool,
    time_ready: bool,
}

struct Observing<'a> {
    inner: &'a mut dyn Listener,
    seen: Seen,
}

impl Listener for Observing<'_> {
    fn frame(&mut self, queued: application::Queued<'_>) -> bool {
        if !self.inner.frame(queued) {
            return false;
        }
        if let Ok(parsed) = frame::parse_prefix(queued.data, frame::Direction::FromClient) {
            if let Some(frame::TimeSync::Sample(t)) = parsed.time_sync {
                self.seen.time_sample = Some(t);
            }
            for message in parsed
                .messages
                .iter()
                .flat_map(|m| m.groups.iter())
                .flat_map(|g| g.messages.iter())
            {
                match message {
                    frame::Message::State(value) => {
                        self.seen.state_report = true;
                        self.seen.state_reports += 1;
                        self.seen.level_linked |= *value == 1;
                        self.seen.time_ready |= *value == 0;
                    }
                    frame::Message::Opaque {
                        index: frame::EX_LEVEL_READY,
                        ..
                    } => self.seen.level_ready = true,
                    frame::Message::Opaque {
                        index: frame::EX_SUBLEVEL_REPORT,
                        body,
                    } => {
                        self.seen.reports += 1;
                        self.seen.reported.extend(frame::sublevel_report_ids(*body));
                    }
                    _ => {}
                }
            }
        }
        true
    }
}

pub const CONTENT_DELAY_MS: u64 = 500;
pub const REPORT_COVERAGE_PERCENT: usize = 95;
pub const REPORT_QUIET_MS: u64 = 5_000;
pub const REPORT_TIMEOUT_MS: u64 = 30_000;
pub const STATE_WAIT_MS: u64 = 1_000;
pub const REPLICATION_INTERVAL_MS: u64 = 50;
/// Official hosts repeat the movement grants in every regular frame, about
/// every 100 ms (E101: 2,030 of 2,061 gaps between 90 and 110 ms).
pub const MOVEMENT_INTERVAL_MS: u64 = 100;
pub const MAX_REPLICATION_QUEUE: usize = 32;
pub const MAX_REPLICATION_QUEUE_BITS: usize = 1024 * 1024;
pub const MAX_RPC_QUEUE: usize = 2048;
pub const MAX_CONTENT_CHUNKS: usize = 4096;

fn missing_scene_level(
    section: &Section,
    reported: &std::collections::BTreeSet<u16>,
) -> Option<u16> {
    use crate::replication::{Initial, Update, sublevel};
    for record in &section.records {
        if let Some(Initial::SubLevel { prefix, .. }) = &record.initial
            && prefix.level_id != 0
            && !reported.contains(&prefix.level_id)
        {
            return Some(prefix.level_id);
        }
        if let Update::SubLevel { fields, .. } = &record.update {
            for field in fields {
                if let Some(sublevel::Update::LevelReference(Some(level))) = field
                    && ![0, u16::MAX].contains(level)
                    && !reported.contains(level)
                {
                    return Some(*level);
                }
            }
        }
    }
    None
}

enum Stage {
    Handshake(HostHandshake, Application),
    Running {
        link: Link,
        application: Application,
        world: Box<World>,
    },
    Closed(Option<Application>),
}

pub struct Host {
    stage: Stage,
    content: std::collections::VecDeque<frame::Message<'static>>,
    content_sent: usize,
    registered_handles: usize,
    replication_queue:
        std::collections::VecDeque<(Section, usize, Vec<crate::participants::HostRpc>)>,
    replication_bindings: Bindings,
    replication_bits: usize,
    replication_sent: usize,
    max_replication_frame_bits: usize,
    rpc_queue: std::collections::VecDeque<crate::participants::HostRpc>,
    files: [Option<files::Outgoing>; 2],
    /// Objects the client is told to report movement for (selector 0).
    movement: std::collections::BTreeSet<u16>,
    movement_sent_ms: Option<u64>,
    /// Objects whose creation has been sent and not deleted since: grants
    /// never name an object the client cannot know yet.
    sent_objects: std::collections::BTreeSet<u16>,
}

impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Host")
            .field("stage", &self.stage())
            .finish_non_exhaustive()
    }
}

impl Host {
    pub fn new(
        codec: Codec,
        ids: Ids,
        policy: Policy,
        random: Box<dyn Random + Send>,
    ) -> Option<Self> {
        let handshake = HostHandshake::new(codec, ids.client, ids.host, ids.host_index)?;
        Some(Self {
            stage: Stage::Handshake(handshake, Application::new(policy, random)),
            content: std::collections::VecDeque::new(),
            content_sent: 0,
            registered_handles: 0,
            replication_queue: std::collections::VecDeque::new(),
            replication_bindings: Bindings::default(),
            replication_bits: 0,
            replication_sent: 0,
            rpc_queue: std::collections::VecDeque::new(),
            files: [None, None],
            movement: std::collections::BTreeSet::new(),
            movement_sent_ms: None,
            sent_objects: std::collections::BTreeSet::new(),
            max_replication_frame_bits: policy
                .max_frame_bits
                .min(application::OUTBOUND_FRAME_BITS)
                .min((application::WINDOW as usize - 1) * DEFAULT_FRAGMENT_BITS),
        })
    }

    pub fn queue_replication(&mut self, section: Section) -> Result<(), Error> {
        self.queue_replication_after(section, Vec::new())
    }

    /// Replace the objects whose movement the client reports. The client only
    /// streams a car's movement once the host lists it (E101: 13-16 ms after
    /// each first listing); an empty set stops the grants. Objects are listed
    /// only once their creation has been sent.
    pub fn set_movement(&mut self, objects: std::collections::BTreeSet<u16>) -> Result<(), Error> {
        if objects.len() > frame::MAX_MOVEMENT_RECORDS {
            return Err(Error::Application(application::Error::Bound));
        }
        self.movement = objects;
        Ok(())
    }

    pub fn queue_replication_after(
        &mut self,
        section: Section,
        before: Vec<crate::participants::HostRpc>,
    ) -> Result<(), Error> {
        if self.stage() == StageName::Closed {
            return Err(Error::Stage);
        }
        if self.replication_queue.len() >= MAX_REPLICATION_QUEUE
            || (!before.is_empty() && !self.rpc_queue.is_empty())
        {
            return Err(Error::Application(application::Error::Window));
        }
        let bits = replication_frame(&section, &before, 0)?.len();
        if bits > self.max_replication_frame_bits {
            return Err(Error::Application(application::Error::Bound));
        }
        if bits > MAX_REPLICATION_QUEUE_BITS - self.replication_bits {
            return Err(Error::Application(application::Error::Window));
        }
        for &reply in &before {
            self.validate_rpc(reply)?;
        }
        self.replication_bindings
            .apply(&section)
            .map_err(Error::Replication)?;
        self.replication_bits += bits;
        self.replication_queue.push_back((section, bits, before));
        Ok(())
    }

    pub fn replication_progress(&self) -> (usize, usize) {
        (self.replication_sent, self.replication_queue.len())
    }

    pub fn queue_launcher_enabled(
        &mut self,
        reply: crate::launchers::Enabled,
    ) -> Result<(), Error> {
        self.queue_rpc(crate::participants::HostRpc::Launcher(reply))
    }

    pub fn queue_rpc(&mut self, reply: crate::participants::HostRpc) -> Result<(), Error> {
        if self.stage() == StageName::Closed {
            return Err(Error::Stage);
        }
        if self.rpc_queue.len() >= MAX_RPC_QUEUE {
            return Err(Error::Application(application::Error::Window));
        }
        self.validate_rpc(reply)?;
        self.rpc_queue.push_back(reply);
        Ok(())
    }

    fn validate_rpc(&self, reply: crate::participants::HostRpc) -> Result<(), Error> {
        reply.encode().map_err(Error::Replication)?;
        let endpoint_valid = match reply {
            crate::participants::HostRpc::Launcher(v) => matches!(
                self.replication_bindings.get(v.scene),
                Some(crate::replication::Kind::Entity | crate::replication::Kind::SubLevel)
            ),
            crate::participants::HostRpc::SequenceStop(v) => {
                self.replication_bindings.is_sequence(v.sequence)
            }
            crate::participants::HostRpc::Event(v) => matches!(
                self.replication_bindings.get(v.target.ghost),
                Some(crate::replication::Kind::Entity | crate::replication::Kind::SubLevel)
            ),
            _ => {
                self.replication_bindings.get(reply.scene())
                    == Some(crate::replication::Kind::SubLevel)
            }
        };
        if !endpoint_valid {
            return Err(Error::Replication(crate::replication::Error::UnknownObject));
        }
        let participant = match reply {
            crate::participants::HostRpc::Participant(v) => Some(v.participant),
            crate::participants::HostRpc::Garage(v) => Some(v.participant),
            crate::participants::HostRpc::Actor(v) => Some(v.participant),
            crate::participants::HostRpc::Vehicle(v) => Some(v.participant),
            crate::participants::HostRpc::Launcher(_)
            | crate::participants::HostRpc::GaragePresence(_)
            | crate::participants::HostRpc::SpawnOccupied(_)
            | crate::participants::HostRpc::LevelPoll(_)
            | crate::participants::HostRpc::SequenceStop(_)
            | crate::participants::HostRpc::Event(_) => None,
        };
        if participant.is_some_and(|id| {
            self.replication_bindings.get(id) != Some(crate::replication::Kind::Participant)
        }) {
            return Err(Error::Replication(crate::replication::Error::UnknownObject));
        }
        if let crate::participants::HostRpc::Actor(v) = reply
            && self.replication_bindings.get(v.actor) != Some(crate::replication::Kind::Actor)
        {
            return Err(Error::Replication(crate::replication::Error::UnknownObject));
        }
        if let crate::participants::HostRpc::Vehicle(v) = reply
            && !self.replication_bindings.is_vehicle(v.vehicle)
        {
            return Err(Error::Replication(crate::replication::Error::UnknownObject));
        }
        Ok(())
    }

    pub fn queue_content(&mut self, target: u8, bytes: &[u8]) -> Result<usize, Error> {
        if self.stage() == StageName::Closed {
            return Err(Error::Stage);
        }
        let chunks =
            frame::chunks(target, bytes).ok_or(Error::Application(application::Error::Bound))?;
        let n = chunks.len();
        if n > MAX_CONTENT_CHUNKS.saturating_sub(self.content.len()) {
            return Err(Error::Application(application::Error::Window));
        }
        let added = if target == frame::EX_COLLECTION {
            frame::collection_handles(bytes)
                .ok_or(Error::Application(application::Error::Shape))?
                .len()
        } else {
            0
        };
        self.registered_handles = self
            .registered_handles
            .checked_add(added)
            .ok_or(Error::Application(application::Error::Bound))?;
        self.content.extend(chunks);
        Ok(n)
    }

    pub fn registration_progress(&self) -> (usize, usize) {
        let reported = match &self.stage {
            Stage::Running { world, .. } => world.reported.len(),
            _ => 0,
        };
        (self.registered_handles, reported)
    }

    pub fn content_progress(&self) -> (usize, usize) {
        (self.content_sent, self.content.len())
    }

    pub fn stage(&self) -> StageName {
        match self.stage {
            Stage::Handshake(..) => StageName::Handshake,
            Stage::Running { .. } => StageName::Running,
            Stage::Closed(_) => StageName::Closed,
        }
    }

    pub fn application(&self) -> Option<&Application> {
        match &self.stage {
            Stage::Handshake(_, application) | Stage::Running { application, .. } => {
                Some(application)
            }
            Stage::Closed(application) => application.as_ref(),
        }
    }

    pub fn link(&self) -> Option<&Link> {
        match &self.stage {
            Stage::Running { link, .. } => Some(link),
            _ => None,
        }
    }

    pub fn receive(&mut self, raw: &[u8], now_ms: u64, listener: &mut dyn Listener) -> Output {
        let mut out = Output::default();
        let mut closed = false;
        match &mut self.stage {
            Stage::Closed(_) => {}
            Stage::Handshake(handshake, _) => match handshake.receive(raw) {
                Step::Send(bytes) => {
                    out.send.push(bytes);
                    out.events.push(Event::HandshakeReply);
                }
                Step::Ignored(ignored) => out.events.push(Event::Ignored(ignored)),
                Step::Established => {
                    let Stage::Handshake(handshake, application) =
                        std::mem::replace(&mut self.stage, Stage::Closed(None))
                    else {
                        unreachable!("matched above")
                    };
                    let Some((codec, cursors)) = handshake.established() else {
                        unreachable!("the handshake reported Established")
                    };
                    match Link::start(codec, cursors, now_ms) {
                        Ok((link, initial_sync)) => {
                            out.send.push(initial_sync);
                            out.events.push(Event::Established);
                            self.stage = Stage::Running {
                                link,
                                application,
                                world: Box::default(),
                            };
                        }
                        Err(e) => {
                            out.events.push(Event::Link(e));
                            self.stage = Stage::Closed(Some(application));
                        }
                    }
                }
            },
            Stage::Running {
                link,
                application,
                world,
            } => match link.receive(raw, now_ms) {
                Err(e) => out.events.push(Event::Link(e)),
                Ok(delivery) => {
                    out.send.extend(delivery.send);
                    let mut queued = false;
                    let mut observing = Observing {
                        inner: listener,
                        seen: Seen::default(),
                    };
                    for body in &delivery.applications {
                        match application.receive(body, now_ms, &mut observing) {
                            Ok(received) => {
                                if let Some(event) = received.event {
                                    queued |= matches!(event, application::Event::Queued { .. });
                                    out.events.push(Event::Application(event));
                                }
                                for reply in received.send {
                                    match link.send_application(&reply, now_ms) {
                                        Ok(wire) => out.send.push(wire),
                                        Err(e) => out.events.push(Event::Link(e)),
                                    }
                                }
                            }
                            Err(e) => out.events.push(Event::ApplicationError(e)),
                        }
                    }
                    let seen = observing.seen;
                    world.client_state_reported |= seen.state_report;
                    world.state_reports += seen.state_reports;
                    world.client_level_ready |= seen.level_ready;
                    world.client_level_linked |= seen.level_linked;
                    world.client_time_ready |= seen.time_ready && world.time_sent.is_some();
                    if seen.reports > 0 {
                        world.reports += seen.reports;
                        world.last_report_ms = now_ms;
                        world.reported.extend(seen.reported);
                    }
                    if let Some(sample) = seen.time_sample {
                        world.time_sample = Some((sample, now_ms));
                    }
                    if queued && world.initialized.is_none() {
                        match Self::initialize(link, application, world, now_ms) {
                            Ok(wire) => {
                                out.send.push(wire);
                                out.events.push(Event::Initialized);
                            }
                            Err(e) => out.events.push(e),
                        }
                    }
                    if delivery.closed {
                        out.events.push(Event::Closed);
                        closed = true;
                    }
                }
            },
        }
        if closed
            && let Stage::Running {
                mut application,
                world,
                ..
            } = std::mem::replace(&mut self.stage, Stage::Closed(None))
        {
            world.player_entry.release(&mut application);
            for file in &mut self.files {
                if let Some(file) = file.take()
                    && let Some(ticket) = file.receipt
                {
                    application.release_delivery(ticket);
                }
            }
            self.stage = Stage::Closed(Some(application));
        }
        out
    }

    fn initialize(
        link: &mut Link,
        application: &mut Application,
        world: &mut World,
        now_ms: u64,
    ) -> Result<Vec<u8>, Event> {
        let origin = application
            .origin_ms()
            .ok_or(Event::ApplicationError(application::Error::Phase))?;
        let tick = (now_ms.saturating_sub(origin) * 30 / 1000) as u32;
        let time = tick as f32 / 30.0;
        let bits = frame::initializer(tick, time, world.next_ex_sequence);
        let body = application
            .send_frame(bits.span())
            .map_err(Event::ApplicationError)?;
        let wire = link.send_application(&body, now_ms).map_err(Event::Link)?;
        world.initialized = Some(now_ms);
        world.next_ex_sequence = (world.next_ex_sequence + 2) & 0x7f;
        world.acked_inputs = application.stats().inputs;
        Ok(wire)
    }

    fn send_built(
        link: &mut Link,
        application: &mut Application,
        builder: &frame::Builder,
        now_ms: u64,
    ) -> Result<Vec<u8>, Event> {
        let bits = builder
            .build()
            .map_err(|_| Event::ApplicationError(application::Error::Shape))?;
        let body = application
            .send_frame(bits.span())
            .map_err(Event::ApplicationError)?;
        link.send_application(&body, now_ms).map_err(Event::Link)
    }

    pub fn poll(&mut self, now_ms: u64) -> Output {
        let mut out = Output::default();
        let Stage::Running {
            link,
            application,
            world,
        } = &mut self.stage
        else {
            return out;
        };
        match link.poll(now_ms) {
            Ok(Some(wire)) => out.send.push(wire),
            Ok(None) => {}
            Err(e) => out.events.push(Event::Link(e)),
        }
        match application.poll(now_ms) {
            Ok(bodies) => {
                for body in bodies {
                    match link.send_application(&body, now_ms) {
                        Ok(wire) => out.send.push(wire),
                        Err(e) => out.events.push(Event::Link(e)),
                    }
                }
            }
            Err(e) => {
                out.events.push(Event::ApplicationError(e));
                return out;
            }
        }
        Self::poll_files(&mut self.files, application, link, now_ms, &mut out);
        let Some(initialized_ms) = world.initialized else {
            return out;
        };
        let due = match world.descriptor_sent {
            None => world.client_state_reported && now_ms >= initialized_ms + CONTENT_DELAY_MS,
            Some(_) => world.client_level_ready,
        };
        if !self.content.is_empty() && due {
            let (take, state) = match world.descriptor_sent {
                None => {
                    let first_message = self
                        .content
                        .iter()
                        .position(|m| matches!(m, frame::Message::Chunk { last: true, .. }))
                        .map_or(self.content.len(), |i| i + 1);
                    (first_message.min(frame::CHUNKS_PER_FRAME), Some(0))
                }
                Some(_) => (self.content.len().min(frame::CHUNKS_PER_FRAME), None),
            };
            let messages: Vec<frame::Message<'static>> = self.content.drain(..take).collect();
            let builder = frame::Builder::new().messages(
                state,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(world.next_ex_sequence),
                    messages: messages.clone(),
                }],
            );
            match Self::send_built(link, application, &builder, now_ms) {
                Ok(wire) => {
                    out.send.push(wire);
                    world.next_ex_sequence = (world.next_ex_sequence + take as u8) & 0x7f;
                    world.acked_inputs = application.stats().inputs;
                    world.descriptor_sent.get_or_insert(now_ms);
                    self.content_sent += take;
                    if self.content.is_empty() {
                        world.content_done_ms = Some(now_ms);
                    }
                    out.events.push(Event::Content {
                        chunks: take,
                        remaining: self.content.len(),
                    });
                }
                Err(e) => {
                    for m in messages.into_iter().rev() {
                        self.content.push_front(m);
                    }
                    if e != Event::ApplicationError(application::Error::Window) {
                        out.events.push(e);
                    }
                }
            }
        }
        self.replication(now_ms, &mut out);
        let Stage::Running {
            link,
            application,
            world,
        } = &mut self.stage
        else {
            return out;
        };
        if let Some((sample, received_ms)) = world.time_sample.take() {
            let origin = application.origin_ms().unwrap_or(initialized_ms);
            let ticks = |ms: u64| ms.saturating_sub(origin) * 1024 / 1000;
            let builder =
                frame::Builder::new().time_sync_times([sample, ticks(received_ms), ticks(now_ms)]);
            match Self::send_built(link, application, &builder, now_ms) {
                Ok(wire) => {
                    out.send.push(wire);
                    world.acked_inputs = application.stats().inputs;
                    out.events.push(Event::TimeSynced);
                }
                Err(e) => out.events.push(e),
            }
        }
        let grants: Vec<(u16, Vec<u8>)> = self
            .movement
            .intersection(&self.sent_objects)
            .map(|&id| (id, vec![0]))
            .collect();
        if !grants.is_empty()
            && self
                .movement_sent_ms
                .is_none_or(|last| now_ms >= last + MOVEMENT_INTERVAL_MS)
        {
            let builder = frame::Builder::new().movement(grants);
            match Self::send_built(link, application, &builder, now_ms) {
                Ok(wire) => {
                    out.send.push(wire);
                    world.acked_inputs = application.stats().inputs;
                    self.movement_sent_ms = Some(now_ms);
                }
                Err(Event::ApplicationError(application::Error::Window)) => {}
                Err(e) => out.events.push(e),
            }
        }
        let inputs = application.stats().inputs;
        if inputs > world.acked_inputs {
            if let Ok(wire) = Self::send_built(link, application, &frame::Builder::new(), now_ms) {
                out.send.push(wire);
            }
            world.acked_inputs = inputs;
        }
        out
    }

    fn replication(&mut self, now_ms: u64, out: &mut Output) {
        let Stage::Running {
            link,
            application,
            world,
        } = &mut self.stage
        else {
            return;
        };
        let Some(content_done) = world.content_done_ms else {
            return;
        };
        if !self.content.is_empty() {
            return;
        }
        let reliable = |messages: Vec<frame::Message<'static>>, sequence: u8| {
            frame::Builder::new().messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(sequence),
                    messages,
                }],
            )
        };
        let Some((state_ms, states_then)) = world.state_sent else {
            let covered = self.registered_handles > 0
                && world.reported.len() * 100 >= self.registered_handles * REPORT_COVERAGE_PERCENT;
            let quiet = world.reports > 0 && now_ms >= world.last_report_ms + REPORT_QUIET_MS;
            let timed_out = now_ms >= content_done + REPORT_TIMEOUT_MS;
            if !(covered || quiet || timed_out) {
                return;
            }
            let builder = reliable(vec![frame::Message::State(7)], world.next_ex_sequence);
            match Self::send_built(link, application, &builder, now_ms) {
                Ok(wire) => {
                    out.send.push(wire);
                    world.next_ex_sequence = (world.next_ex_sequence + 1) & 0x7f;
                    world.acked_inputs = application.stats().inputs;
                    world.state_sent = Some((now_ms, world.state_reports));
                    out.events.push(Event::StateSent {
                        registered: self.registered_handles,
                        reported: world.reported.len(),
                    });
                }
                Err(e) => out.events.push(e),
            }
            return;
        };
        let Some((time_ms, states_then)) = world.time_sent else {
            if world.state_reports == states_then && now_ms < state_ms + STATE_WAIT_MS {
                return;
            }
            let origin = application.origin_ms().unwrap_or(state_ms);
            let tick = (now_ms.saturating_sub(origin) * 30 / 1000) as u32;
            let builder = reliable(
                vec![frame::Message::Time {
                    flag: false,
                    tick,
                    time: tick as f32 / 30.0,
                }],
                world.next_ex_sequence,
            );
            match Self::send_built(link, application, &builder, now_ms) {
                Ok(wire) => {
                    out.send.push(wire);
                    world.next_ex_sequence = (world.next_ex_sequence + 1) & 0x7f;
                    world.acked_inputs = application.stats().inputs;
                    world.time_sent = Some((now_ms, world.state_reports));
                    out.events.push(Event::TimeSent);
                }
                Err(e) => out.events.push(e),
            }
            return;
        };
        if world.state_reports == states_then && now_ms < time_ms + STATE_WAIT_MS {
            return;
        }
        if world
            .last_replication_ms
            .is_some_and(|last| now_ms < last + REPLICATION_INTERVAL_MS)
        {
            return;
        }
        if let Some((section, size, before)) = self.replication_queue.front() {
            if let Some(level) = missing_scene_level(section, &world.reported) {
                if world.blocked_level != Some(level) {
                    out.events.push(Event::ReplicationWaiting { level });
                    world.blocked_level = Some(level);
                }
                return;
            }
            world.blocked_level = None;
            let bits = match replication_frame(section, before, world.next_ex_sequence) {
                Ok(bits) => bits,
                Err(_) => {
                    out.events
                        .push(Event::ApplicationError(application::Error::Shape));
                    return;
                }
            };
            let first_player = world.player_entry.first_creation(section);
            let sent = if let Some(player) = first_player {
                application
                    .send_tracked_frame(bits.span(), DEFAULT_FRAGMENT_BITS)
                    .map(|(ticket, bodies)| {
                        world.player_entry = entry::Entry::Pending { player, ticket };
                        bodies
                    })
            } else {
                application.send_frame_fragments(bits.span(), DEFAULT_FRAGMENT_BITS)
            };
            let bodies = match sent {
                Ok(bodies) => bodies,
                Err(application::Error::Window) => return,
                Err(error) => {
                    out.events.push(Event::ApplicationError(error));
                    return;
                }
            };
            for id in &section.deleted {
                self.sent_objects.remove(id);
            }
            for record in &section.records {
                if record.initial.is_some() {
                    self.sent_objects.insert(record.id);
                }
            }
            let records = section.records.len();
            let notifications = before.len();
            let fragments = bodies.len();
            for body in bodies {
                match link.send_application(&body, now_ms) {
                    Ok(wire) => out.send.push(wire),
                    Err(error) => out.events.push(Event::Link(error)),
                }
            }
            self.replication_bits -= size;
            self.replication_queue.pop_front();
            self.replication_sent += 1;
            world.next_ex_sequence = (world.next_ex_sequence + notifications as u8) & 0x7f;
            world.acked_inputs = application.stats().inputs;
            world.last_replication_ms = Some(now_ms);
            out.events.push(Event::Replication {
                records,
                fragments,
                remaining: self.replication_queue.len(),
            });
            if notifications != 0 {
                out.events.push(Event::RpcNotifications {
                    count: notifications,
                    remaining: self.rpc_queue.len(),
                });
            }
            return;
        }
        if let Some(player) = world.player_entry.ready(
            application,
            world.client_level_linked && world.client_time_ready,
        ) {
            let builder = reliable(vec![frame::Message::Empty], world.next_ex_sequence);
            match Self::send_built(link, application, &builder, now_ms) {
                Ok(wire) => {
                    out.send.push(wire);
                    world.player_entry = entry::Entry::Entered;
                    world.next_ex_sequence = (world.next_ex_sequence + 1) & 0x7f;
                    world.acked_inputs = application.stats().inputs;
                    world.last_replication_ms = Some(now_ms);
                    out.events.push(Event::FirstPlayerEntered { player });
                }
                Err(e) => out.events.push(e),
            }
            return;
        }
        if !self.rpc_queue.is_empty() {
            let count = self.rpc_queue.len().min(15);
            let builder = reliable(
                self.rpc_queue
                    .iter()
                    .take(count)
                    .copied()
                    .map(rpc_message)
                    .collect(),
                world.next_ex_sequence,
            );
            match Self::send_built(link, application, &builder, now_ms) {
                Ok(wire) => {
                    out.send.push(wire);
                    self.rpc_queue.drain(..count);
                    world.next_ex_sequence = (world.next_ex_sequence + count as u8) & 0x7f;
                    world.acked_inputs = application.stats().inputs;
                    world.last_replication_ms = Some(now_ms);
                    out.events.push(Event::RpcNotifications {
                        count,
                        remaining: self.rpc_queue.len(),
                    });
                }
                Err(e) => out.events.push(e),
            }
        }
    }

    pub fn send_frame(&mut self, frame: BitSpan<'_>, now_ms: u64) -> Result<Vec<u8>, Error> {
        let Stage::Running {
            link, application, ..
        } = &mut self.stage
        else {
            return Err(Error::Stage);
        };
        let body = application.send_frame(frame).map_err(Error::Application)?;
        link.send_application(&body, now_ms).map_err(Error::Link)
    }
}

#[cfg(test)]
mod tests;
