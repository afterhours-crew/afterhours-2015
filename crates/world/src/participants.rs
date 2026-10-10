// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    bits::BitWriter,
    replication::{Error, Initial, Record, sublevel},
};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile, Serial},
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_PARTICIPANTS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub scene: u16,
    pub selector: u16,
    pub serial: Serial,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Call {
    Add,
    Enter,
    Leave,
    LoadInventory,
    /// Leave carrying a nonzero 5-bit tail (observed 31 when the garage exit
    /// leaves startup state 4).
    LeaveTagged(u8),
    /// Method-2 call on a participant-scoped garage endpoint at garage exit.
    Notify,
    /// Method 0 carrying a u32 argument (SpawnPoints assignment id).
    Assign(u32),
}
impl Call {
    pub fn method(self) -> u32 {
        match self {
            Self::Add | Self::Enter | Self::Assign(_) => 0,
            Self::Leave | Self::LeaveTagged(_) => 1,
            Self::LoadInventory | Self::Notify => 2,
        }
    }
    pub fn argument(self) -> Option<u32> {
        match self {
            Self::Assign(value) => Some(value),
            _ => None,
        }
    }
    /// The 5 bits written after the method word and argument; zero except
    /// `LeaveTagged`.
    pub fn tail(self) -> u8 {
        match self {
            Self::LeaveTagged(tail) => tail,
            _ => 0,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Notification {
    pub endpoint: Endpoint,
    pub participant: u16,
    pub call: Call,
}
impl Notification {
    pub fn encode(self) -> Result<BitWriter, Error> {
        if self.endpoint.scene == 0
            || self.endpoint.scene > 8191
            || self.endpoint.selector > 511
            || self.participant == 0
            || self.participant > 8191
            || self.call.tail() > 31
        {
            return Err(Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload
            .put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(self.call.method().into(), 32);
        if let Some(argument) = self.call.argument() {
            payload.put(argument.into(), 32);
        }
        payload.put(self.call.tail().into(), 5);
        payload.align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(self.endpoint.scene.into(), 13)
            .put(self.participant.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bindings {
    pub manager: Endpoint,
    pub preparing: Endpoint,
    pub inventory: Endpoint,
    pub persistent: Endpoint,
    pub check_player: Endpoint,
    pub persistent_loaded: Endpoint,
    pub garage: [Endpoint; 5],
    pub recovery: Endpoint,
    pub progression_loaded: Endpoint,
    pub entered_game: Endpoint,
    pub enter_garage_from_boot: Endpoint,
    pub begin_garage: Endpoint,
    pub loading_garage: Endpoint,
    pub composite: Endpoint,
    pub initial_teleport: Endpoint,
    pub customization: Endpoint,
}
impl Bindings {
    pub fn from_records(
        records: &[Record],
        gameplay_key: u32,
        startup_key: u32,
    ) -> Result<Option<Self>, Error> {
        if gameplay_key <= 1 || startup_key <= 1 || gameplay_key == startup_key {
            return Err(Error::Shape);
        }
        fn endpoint(
            records: &[Record],
            key: u32,
            index: usize,
            gameplay_key: u32,
        ) -> Result<Endpoint, Error> {
            let mut found = records.iter().filter_map(|record| match &record.initial {
                Some(Initial::SubLevel { prefix, fields }) if prefix.content_key == key => {
                    Some((record.id, fields))
                }
                _ => None,
            });
            let (scene, fields) = found.next().ok_or(Error::UnknownObject)?;
            if found.next().is_some() {
                return Err(Error::DuplicateObject);
            }
            let rpc = if key == gameplay_key && (8..=12).contains(&index) {
                match fields.get(index) {
                    Some(sublevel::Initial::Rpc(rpc)) => rpc,
                    _ => return Err(Error::TypeMismatch),
                }
            } else {
                match fields.get(index) {
                    Some(sublevel::Initial::RpcReferences { rpc, value }) if value.is_empty() => {
                        rpc
                    }
                    _ => return Err(Error::TypeMismatch),
                }
            };
            Ok(Endpoint {
                scene,
                selector: rpc.selector,
                serial: rpc.serial,
            })
        }
        if !records.iter().any(|r| matches!(&r.initial, Some(Initial::SubLevel { prefix, .. }) if prefix.content_key == startup_key)) { return Ok(None); }
        Ok(Some(Self {
            manager: endpoint(records, gameplay_key, 1, gameplay_key)?,
            preparing: endpoint(records, startup_key, 6, gameplay_key)?,
            inventory: endpoint(records, startup_key, 11, gameplay_key)?,
            persistent: endpoint(records, startup_key, 7, gameplay_key)?,
            check_player: endpoint(records, startup_key, 8, gameplay_key)?,
            persistent_loaded: endpoint(records, startup_key, 25, gameplay_key)?,
            garage: [8, 9, 10, 11, 12]
                .map(|index| endpoint(records, gameplay_key, index, gameplay_key))
                .into_iter()
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| Error::Shape)?,
            recovery: endpoint(records, startup_key, 21, gameplay_key)?,
            progression_loaded: endpoint(records, startup_key, 18, gameplay_key)?,
            entered_game: endpoint(records, startup_key, 19, gameplay_key)?,
            enter_garage_from_boot: endpoint(records, startup_key, 114, gameplay_key)?,
            begin_garage: endpoint(records, startup_key, 95, gameplay_key)?,
            loading_garage: endpoint(records, startup_key, 90, gameplay_key)?,
            composite: endpoint(records, startup_key, 89, gameplay_key)?,
            initial_teleport: endpoint(records, startup_key, 86, gameplay_key)?,
            customization: endpoint(records, startup_key, 87, gameplay_key)?,
        }))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    LoadingInventory,
    LoadingPersistentData,
    PersistentDataLoaded,
    Progressing,
    LoadingGarage,
    EntryBlocked,
    Customization,
    /// Garage exit requested; the client loads the world (startup state 77).
    ExitingGarage,
    /// The client reported the world loaded (startup state 78).
    EnteringWorld,
    /// Startup state 2 after the exit chain: driving in the open world.
    FreeRoam,
}
/// Garage-exit endpoints of the startup and garage sublevels, by field index:
/// startup 97 and garage 9 are the client's exit requests, startup 81 its
/// world-ready request, the rest host states and garage calls (E747).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitBindings {
    pub exit_request: Endpoint,
    pub garage_exit_request: Endpoint,
    pub state_94: Endpoint,
    pub state_4: Endpoint,
    pub state_3: Endpoint,
    pub state_77: Endpoint,
    pub state_83: Endpoint,
    pub state_78: Endpoint,
    pub ready_request: Endpoint,
    pub state_79: Endpoint,
    pub state_2: Endpoint,
    pub garage_calls: [Endpoint; 5],
}
type Scene<'a> = (u16, &'a [sublevel::Initial]);
impl ExitBindings {
    pub fn from_records(
        records: &[Record],
        startup_key: u32,
        garage_key: u32,
    ) -> Result<Option<Self>, Error> {
        fn scene(records: &[Record], key: u32) -> Result<Option<Scene<'_>>, Error> {
            let mut found = records.iter().filter_map(|record| match &record.initial {
                Some(Initial::SubLevel { prefix, fields }) if prefix.content_key == key => {
                    Some((record.id, fields.as_slice()))
                }
                _ => None,
            });
            let first = found.next();
            if found.next().is_some() {
                return Err(Error::DuplicateObject);
            }
            Ok(first)
        }
        fn endpoint((scene, fields): Scene<'_>, index: usize) -> Result<Endpoint, Error> {
            let rpc = match fields.get(index) {
                Some(sublevel::Initial::Rpc(rpc)) => rpc,
                Some(sublevel::Initial::RpcReferences { rpc, .. }) => rpc,
                _ => return Err(Error::TypeMismatch),
            };
            Ok(Endpoint {
                scene,
                selector: rpc.selector,
                serial: rpc.serial,
            })
        }
        if startup_key == garage_key {
            return Err(Error::Shape);
        }
        let (Some(startup), Some(garage)) =
            (scene(records, startup_key)?, scene(records, garage_key)?)
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            exit_request: endpoint(startup, 97)?,
            garage_exit_request: endpoint(garage, 9)?,
            state_94: endpoint(startup, 94)?,
            state_4: endpoint(startup, 4)?,
            state_3: endpoint(startup, 3)?,
            state_77: endpoint(startup, 77)?,
            state_83: endpoint(startup, 83)?,
            state_78: endpoint(startup, 78)?,
            ready_request: endpoint(startup, 81)?,
            state_79: endpoint(startup, 79)?,
            state_2: endpoint(startup, 2)?,
            garage_calls: [
                endpoint(garage, 1)?,
                endpoint(garage, 4)?,
                endpoint(garage, 5)?,
                endpoint(garage, 6)?,
                endpoint(garage, 7)?,
            ],
        }))
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitRoute {
    Exit,
    GarageExit,
    WorldLoaded,
    WorldReady,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Entry {
    pub pending_videos: u16,
    pub pending_item_updates: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lifecycle {
    bindings: Option<Bindings>,
    stages: BTreeMap<u16, Stage>,
    garage: Option<crate::garage::Slots>,
    garage_sent: BTreeSet<u16>,
    entries: BTreeMap<u16, Entry>,
    exit: Option<ExitBindings>,
    exited: Vec<u16>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Unsupported,
    Repeated,
    Advanced(Vec<Notification>),
}
impl Lifecycle {
    pub fn waiting_garage(&self) -> Vec<u16> {
        self.stages
            .iter()
            .filter_map(|(&id, stage)| (*stage == Stage::LoadingGarage).then_some(id))
            .collect()
    }
    pub fn set_garage(&mut self, ids: [Option<u64>; 5]) -> Result<(), Error> {
        let slots = crate::garage::Slots::new(ids)?;
        if self.garage.is_some_and(|old| old != slots) {
            return Err(Error::Unsupported);
        }
        self.garage = Some(slots);
        Ok(())
    }
    pub fn take_garage_bindings(&mut self) -> Vec<crate::garage::Binding> {
        let (Some(bindings), Some(slots)) = (self.bindings, self.garage) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (&participant, &stage) in &self.stages {
            if stage != Stage::LoadingPersistentData || !self.garage_sent.insert(participant) {
                continue;
            }
            for (endpoint, item) in bindings.garage.into_iter().zip(slots.values()) {
                if let Some(item) = item {
                    out.push(crate::garage::Binding {
                        endpoint,
                        participant,
                        item,
                    });
                }
            }
        }
        out
    }
    pub fn finish_persistent(&mut self, connected: impl Fn(u16) -> bool) -> Vec<Notification> {
        let Some(bindings) = self.bindings else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (&participant, stage) in &mut self.stages {
            if *stage == Stage::LoadingPersistentData
                && self.garage_sent.contains(&participant)
                && connected(participant)
            {
                *stage = Stage::PersistentDataLoaded;
                out.extend(
                    [
                        (bindings.persistent, Call::Leave),
                        (bindings.check_player, Call::Enter),
                        (bindings.check_player, Call::Leave),
                        (bindings.persistent_loaded, Call::Enter),
                    ]
                    .into_iter()
                    .map(|(endpoint, call)| Notification {
                        endpoint,
                        participant,
                        call,
                    }),
                );
            }
        }
        out
    }
    pub fn waiting_progression(&self) -> Vec<u16> {
        self.stages
            .iter()
            .filter(|(_, stage)| **stage == Stage::PersistentDataLoaded)
            .map(|(id, _)| *id)
            .collect()
    }
    pub fn begin_progression(&mut self, participant: u16) -> bool {
        match self.stages.get_mut(&participant) {
            Some(stage) if *stage == Stage::PersistentDataLoaded => {
                *stage = Stage::Progressing;
                true
            }
            _ => false,
        }
    }
    pub fn enter_garage_loading(&mut self, participant: u16) -> Vec<Notification> {
        let Some(b) = self.bindings else {
            return Vec::new();
        };
        let Some(stage) = self.stages.get_mut(&participant) else {
            return Vec::new();
        };
        if *stage != Stage::Progressing {
            return Vec::new();
        }
        *stage = Stage::LoadingGarage;
        [
            (b.persistent_loaded, Call::Leave),
            (b.recovery, Call::Enter),
            (b.recovery, Call::Leave),
            (b.progression_loaded, Call::Enter),
            (b.progression_loaded, Call::Leave),
            (b.entered_game, Call::Enter),
            (b.entered_game, Call::Leave),
            (b.enter_garage_from_boot, Call::Enter),
            (b.enter_garage_from_boot, Call::Leave),
            (b.begin_garage, Call::Enter),
            (b.begin_garage, Call::Leave),
            (b.loading_garage, Call::Enter),
        ]
        .into_iter()
        .map(|(endpoint, call)| Notification {
            endpoint,
            participant,
            call,
        })
        .collect()
    }
    pub fn enter_garage_with_actor(
        &mut self,
        binding: crate::actors::Binding,
    ) -> Result<Vec<HostRpc>, Error> {
        binding.encode()?;
        let b = self.bindings.ok_or(Error::UnknownObject)?;
        let chain = self.enter_garage_loading(binding.participant);
        let mut out = Vec::with_capacity(chain.len() + 1);
        for notification in chain {
            if notification.endpoint == b.begin_garage && notification.call == Call::Leave {
                out.push(HostRpc::Actor(binding));
            }
            out.push(HostRpc::Participant(notification));
        }
        Ok(out)
    }
    pub fn in_customization(&self) -> Vec<u16> {
        self.stages
            .iter()
            .filter_map(|(&id, stage)| (*stage == Stage::Customization).then_some(id))
            .collect()
    }
    pub fn set_entry(&mut self, participant: u16, entry: Entry) -> Result<(), Error> {
        if participant == 0 || participant > 8191 {
            return Err(Error::Bound);
        }
        if matches!(
            self.stages.get(&participant),
            Some(Stage::EntryBlocked | Stage::Customization)
        ) {
            return Err(Error::Unsupported);
        }
        self.entries.insert(participant, entry);
        Ok(())
    }
    pub fn entry(&self, participant: u16) -> Entry {
        self.entries.get(&participant).copied().unwrap_or_default()
    }
    pub fn enter_customization(&mut self, participant: u16) -> Outcome {
        let Some(b) = self.bindings else {
            return Outcome::Repeated;
        };
        let Some(stage) = self.stages.get_mut(&participant) else {
            return Outcome::Repeated;
        };
        if *stage != Stage::LoadingGarage {
            return Outcome::Repeated;
        }
        let entry = self.entries.get(&participant).copied().unwrap_or_default();
        if entry.pending_videos != 0 || entry.pending_item_updates {
            *stage = Stage::EntryBlocked;
            return Outcome::Unsupported;
        }
        *stage = Stage::Customization;
        Outcome::Advanced(
            [
                (b.loading_garage, Call::Leave),
                (b.composite, Call::Enter),
                (b.composite, Call::Leave),
                (b.initial_teleport, Call::Enter),
                (b.initial_teleport, Call::Leave),
                (b.customization, Call::Enter),
            ]
            .into_iter()
            .map(|(endpoint, call)| Notification {
                endpoint,
                participant,
                call,
            })
            .collect(),
        )
    }
    pub fn bind(
        &mut self,
        records: &[Record],
        gameplay_key: u32,
        startup_key: u32,
    ) -> Result<(), Error> {
        if self.bindings.is_some() || !self.stages.is_empty() {
            return Err(Error::DuplicateObject);
        }
        self.bindings = Bindings::from_records(records, gameplay_key, startup_key)?;
        Ok(())
    }
    /// Bind the garage-exit endpoints after `bind`. `Ok(false)` when the
    /// startup or garage scene is absent; the exit then stays unsupported.
    pub fn bind_exit(
        &mut self,
        records: &[Record],
        startup_key: u32,
        garage_key: u32,
    ) -> Result<bool, Error> {
        if self.exit.is_some() {
            return Err(Error::DuplicateObject);
        }
        self.exit = ExitBindings::from_records(records, startup_key, garage_key)?;
        Ok(self.exit.is_some())
    }
    pub fn exit_bindings(&self) -> Option<ExitBindings> {
        self.exit
    }
    /// Participants whose garage exit was granted since the last call; the
    /// caller turns their garage presence off.
    pub fn take_exited(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.exited)
    }
    pub fn in_free_roam(&self) -> Vec<u16> {
        self.stages
            .iter()
            .filter_map(|(&id, stage)| (*stage == Stage::FreeRoam).then_some(id))
            .collect()
    }
    pub fn available(&self) -> bool {
        self.bindings.is_some()
    }
    /// Whether any participant has begun.
    pub fn has_participants(&self) -> bool {
        !self.stages.is_empty()
    }
    pub fn stage(&self, id: u16) -> Option<Stage> {
        self.stages.get(&id).copied()
    }
    pub fn begin(&mut self, participant: u16) -> Result<Vec<Notification>, Error> {
        let b = self.bindings.ok_or(Error::UnknownObject)?;
        if participant == 0 || participant > 8191 {
            return Err(Error::Bound);
        }
        if self.stages.contains_key(&participant) {
            return Ok(Vec::new());
        }
        if self.stages.len() >= MAX_PARTICIPANTS {
            return Err(Error::Bound);
        }
        self.stages.insert(participant, Stage::LoadingInventory);
        Ok([
            (b.manager, Call::Add),
            (b.preparing, Call::Enter),
            (b.preparing, Call::Leave),
            (b.inventory, Call::Enter),
            (b.inventory, Call::LoadInventory),
        ]
        .into_iter()
        .map(|(endpoint, call)| Notification {
            endpoint,
            participant,
            call,
        })
        .collect())
    }
    pub fn receive(
        &mut self,
        body: BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Outcome, Error> {
        let Some(b) = self.bindings else {
            return Ok(Outcome::Unsupported);
        };
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if let Some(outcome) = self.exit_route(&envelope, &route, &owns)? {
            return Ok(outcome);
        }
        if envelope.references().first() != Some(&b.inventory.scene)
            || route.selector() != b.inventory.selector
            || route.method_index() != 3
        {
            return Ok(Outcome::Unsupported);
        }
        if envelope.words() != [0, 0]
            || envelope.references().len() != 2
            || !envelope.remaining().is_empty()
            || route.arguments().len() != 7
        {
            return Err(Error::Shape);
        }
        let participant = envelope.references()[1];
        if !owns(participant) {
            return Err(Error::UnknownObject);
        }
        let stage = self
            .stages
            .get_mut(&participant)
            .ok_or(Error::UnknownObject)?;
        if *stage != Stage::LoadingInventory {
            return if matches!(
                *stage,
                Stage::LoadingPersistentData
                    | Stage::PersistentDataLoaded
                    | Stage::Progressing
                    | Stage::LoadingGarage
                    | Stage::EntryBlocked
                    | Stage::Customization
                    | Stage::ExitingGarage
                    | Stage::EnteringWorld
                    | Stage::FreeRoam
            ) {
                Ok(Outcome::Repeated)
            } else {
                Err(Error::Shape)
            };
        }
        *stage = Stage::LoadingPersistentData;
        Ok(Outcome::Advanced(
            [(b.inventory, Call::Leave), (b.persistent, Call::Enter)]
                .into_iter()
                .map(|(endpoint, call)| Notification {
                    endpoint,
                    participant,
                    call,
                })
                .collect(),
        ))
    }
}

impl Lifecycle {
    /// Garage exit and world entry (E747). `Ok(None)` is "not an exit route".
    fn exit_route(
        &mut self,
        envelope: &Envelope<'_>,
        route: &nfs_protocol::world::rpc::Route<'_>,
        owns: &impl Fn(u16) -> bool,
    ) -> Result<Option<Outcome>, Error> {
        let (Some(b), Some(x)) = (self.bindings, self.exit) else {
            return Ok(None);
        };
        let &[scene, participant] = envelope.references() else {
            return Ok(None);
        };
        let is = |e: Endpoint, method: u32| {
            scene == e.scene && route.selector() == e.selector && route.method_index() == method
        };
        let kind = if is(x.exit_request, 0) {
            ExitRoute::Exit
        } else if is(x.garage_exit_request, 0) {
            ExitRoute::GarageExit
        } else if is(x.state_77, 3) {
            ExitRoute::WorldLoaded
        } else if is(x.ready_request, 0) {
            ExitRoute::WorldReady
        } else {
            return Ok(None);
        };
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || route.arguments().len() != 7
        {
            return Err(Error::Shape);
        }
        if !owns(participant) {
            return Err(Error::UnknownObject);
        }
        let stage = self
            .stages
            .get_mut(&participant)
            .ok_or(Error::UnknownObject)?;
        let chain: Vec<(Endpoint, Call)> = match (kind, *stage) {
            (ExitRoute::GarageExit, Stage::Customization | Stage::ExitingGarage) => {
                return Ok(Some(Outcome::Repeated));
            }
            (ExitRoute::Exit, Stage::Customization) => {
                *stage = Stage::ExitingGarage;
                self.exited.push(participant);
                let mut chain = vec![
                    (b.customization, Call::Leave),
                    (x.state_94, Call::Enter),
                    (x.state_94, Call::Leave),
                ];
                chain.extend(x.garage_calls.map(|e| (e, Call::Notify)));
                chain.extend([
                    (x.state_4, Call::Enter),
                    (x.state_4, Call::LeaveTagged(31)),
                    (x.state_3, Call::Enter),
                    (x.state_3, Call::Leave),
                    (x.state_77, Call::Enter),
                ]);
                chain
            }
            (ExitRoute::WorldLoaded, Stage::ExitingGarage) => {
                *stage = Stage::EnteringWorld;
                vec![
                    (x.state_77, Call::Leave),
                    (x.state_83, Call::Enter),
                    (x.state_83, Call::Leave),
                    (x.state_78, Call::Enter),
                ]
            }
            (ExitRoute::WorldReady, Stage::EnteringWorld) => {
                *stage = Stage::FreeRoam;
                vec![
                    (x.state_78, Call::Leave),
                    (x.state_79, Call::Enter),
                    (x.state_79, Call::Leave),
                    (x.state_2, Call::Enter),
                ]
            }
            (
                ExitRoute::Exit | ExitRoute::WorldLoaded | ExitRoute::WorldReady,
                Stage::ExitingGarage | Stage::EnteringWorld | Stage::FreeRoam,
            ) => return Ok(Some(Outcome::Repeated)),
            _ => return Ok(Some(Outcome::Unsupported)),
        };
        Ok(Some(Outcome::Advanced(
            chain
                .into_iter()
                .map(|(endpoint, call)| Notification {
                    endpoint,
                    participant,
                    call,
                })
                .collect(),
        )))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostRpc {
    Launcher(crate::launchers::Enabled),
    Participant(Notification),
    Garage(crate::garage::Binding),
    Actor(crate::actors::Binding),
    Vehicle(crate::garage::vehicle::Binding),
    GaragePresence(crate::garage::presence::Notification),
    /// SpawnPoints occupied flag (same scene rpc_bool wire shape as presence).
    SpawnOccupied(crate::garage::presence::Notification),
    /// Level root poll (scene-scoped method 0, same wire shape as presence).
    LevelPoll(crate::garage::presence::Notification),
    SequenceStop(crate::sequences::Stop),
    Event(crate::logic::Fire),
}
impl HostRpc {
    pub fn scene(self) -> u16 {
        match self {
            Self::Launcher(v) => v.scene,
            Self::Participant(v) => v.endpoint.scene,
            Self::Garage(v) => v.endpoint.scene,
            Self::Actor(v) => v.endpoint.scene,
            Self::Vehicle(v) => v.endpoint.scene,
            Self::GaragePresence(v) | Self::SpawnOccupied(v) | Self::LevelPoll(v) => {
                v.endpoint.scene
            }
            Self::SequenceStop(v) => v.sequence,
            Self::Event(v) => v.target.ghost,
        }
    }
    pub fn encode(self) -> Result<BitWriter, Error> {
        match self {
            Self::Launcher(v) => v.encode(),
            Self::Participant(v) => v.encode(),
            Self::Garage(v) => v.encode(),
            Self::Actor(v) => v.encode(),
            Self::Vehicle(v) => v.encode(),
            Self::GaragePresence(v) | Self::SpawnOccupied(v) | Self::LevelPoll(v) => v.encode(),
            Self::SequenceStop(v) => v.encode(),
            Self::Event(v) => v.encode().map_err(|_| Error::Shape),
        }
    }
}

#[cfg(test)]
mod tests;
