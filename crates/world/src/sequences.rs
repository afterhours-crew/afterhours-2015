// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    bits::BitWriter,
    replication::{self, Error, Record, entity, players::Players},
};
use entity::creation::{Asset, Content, Creation, Prefix};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile, Serial},
};
use std::collections::BTreeMap;

pub const MAX_INSTANCES: usize = 128;
pub const FALLBACK_MS: u64 = 300_000;
pub const FIRST_READY_MS: u64 = 16_000;

pub const STREAMING_LOADED: u32 = 3_648_004;
pub const STREAMING_UNLOADED: u32 = 12_540_227;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Component {
    pub scene_key: u32,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Definition {
    pub asset: Asset,
    pub manager: Component,
    pub embedded_bus: u32,
    pub channel: Component,
}

pub fn profile() -> entity::Profile {
    entity::Profile::new(&[
        entity::Kind::SequenceRoot,
        entity::Kind::RpcOptionalReference,
        entity::Kind::RpcReference,
    ])
    .expect("fixed sequence profile")
}

impl Definition {
    pub fn creation(
        self,
        world: &replication::state::World,
        participant: u16,
        number: u32,
    ) -> Result<Creation, Error> {
        if !world
            .get(participant)
            .is_some_and(|o| matches!(o.initial, replication::Initial::Participant(_)))
        {
            return Err(Error::UnknownObject);
        }
        let (manager, manager_selector) = world
            .scene_endpoint(self.manager.scene_key, self.manager.index)
            .ok_or(Error::UnknownObject)?;
        let (channel, channel_selector) = world
            .scene_endpoint(self.channel.scene_key, self.channel.index)
            .ok_or(Error::UnknownObject)?;
        let rpc = |selector| entity::Rpc {
            selector,
            serial: Serial::new(1).expect("fresh lifetime"),
        };
        Ok(Creation {
            prefix: Prefix {
                parent: None,
                blueprint: manager,
                sub_id: self.embedded_bus,
                owner: None,
                asset: self.asset,
            },
            body: entity::Body {
                initial: Some(vec![
                    entity::Initial::SequenceRoot {
                        value: 1,
                        rpc: rpc(0),
                        manager,
                        manager_selector,
                        sequence: number,
                        participants: vec![participant],
                        stopping: false,
                    },
                    entity::Initial::RpcOptionalReference {
                        rpc: rpc(1),
                        reference: Some(participant),
                    },
                    entity::Initial::RpcReference {
                        rpc: rpc(2),
                        reference: channel,
                        target_selector: channel_selector,
                    },
                ]),
                updates: vec![Some(entity::Update::Noop); 3],
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Owner {
    pub connection: u8,
    pub persona: u64,
    pub participant: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stop {
    pub sequence: u16,
    pub serial: Serial,
}
impl Stop {
    pub fn encode(self) -> Result<BitWriter, Error> {
        if self.sequence == 0 || self.sequence > 8191 {
            return Err(Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload
            .put(0, 9)
            .put(self.serial.value().into(), 10)
            .put(0, 32)
            .align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(1, 8)
            .put(self.sequence.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
    pub fn decode(body: BitSpan<'_>) -> Result<Self, Error> {
        let e = Envelope::decode(
            body,
            Limits {
                max_input_bits: 150,
                max_references: 1,
                max_payload_bytes: 7,
            },
        )
        .map_err(|_| Error::Shape)?;
        let r = e
            .route(RouteProfile::ClientReceive)
            .map_err(|_| Error::Shape)?;
        if e.words() != [0, 0]
            || e.references().len() != 1
            || !e.remaining().is_empty()
            || r.selector() != 0
            || r.method_index() != 0
            || r.arguments().len() != 5
        {
            return Err(Error::Shape);
        }
        let value = Self {
            sequence: e.references()[0],
            serial: r.serial().ok_or(Error::Shape)?,
        };
        value.encode()?;
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Completed {
    pub sequence: u16,
    pub participant: u16,
}
impl Completed {
    pub fn encode(self) -> Result<BitWriter, Error> {
        if [self.sequence, self.participant]
            .into_iter()
            .any(|id| id == 0 || id > 8191)
        {
            return Err(Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload.put(0, 9).put(1, 32).align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(self.sequence.into(), 13)
            .put(self.participant.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
    pub fn decode(body: BitSpan<'_>) -> Result<Self, Error> {
        let e = Envelope::decode(
            body,
            Limits {
                max_input_bits: 155,
                max_references: 2,
                max_payload_bytes: 6,
            },
        )
        .map_err(|_| Error::Shape)?;
        let r = e
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if e.words() != [0, 0]
            || e.references().len() != 2
            || !e.remaining().is_empty()
            || r.selector() != 0
            || r.method_index() != 1
            || r.arguments().len() != 7
        {
            return Err(Error::Shape);
        }
        let value = Self {
            sequence: e.references()[0],
            participant: e.references()[1],
        };
        value.encode()?;
        Ok(value)
    }
}

#[derive(Clone, Debug)]
struct Instance {
    owner: Owner,
    ghost: u16,
    phase: Phase,
    streaming: Option<Streaming>,
    garage_loaded: bool,
    ready_at: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct Streaming {
    ghost: u16,
    slot: u8,
    loaded: bool,
}
impl Instance {
    fn deadline(&self, world_ms: u64, streaming: bool, garage: bool) -> Result<Option<u64>, Error> {
        if self.ready_at.is_none()
            && matches!(self.phase, Phase::Running { .. })
            && streaming
            && garage
        {
            Ok(Some(
                world_ms.checked_add(FIRST_READY_MS).ok_or(Error::Bound)?,
            ))
        } else {
            Ok(self.ready_at)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Running { fallback_at: u64 },
    Stopping,
    Complete,
}

#[derive(Clone, Debug)]
pub struct Sequences {
    definition: Definition,
    content: Content,
    entries: BTreeMap<u16, Instance>,
    world_ms: u64,
}
impl Sequences {
    pub fn new(definition: Definition, content: Content) -> Result<Self, Error> {
        if definition.embedded_bus == 0 || content.profile(definition.asset)? != &profile() {
            return Err(Error::Shape);
        }
        Ok(Self {
            definition,
            content,
            entries: BTreeMap::new(),
            world_ms: 0,
        })
    }
    pub fn start(&mut self, players: &mut Players, owner: Owner) -> Result<Option<Record>, Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        if let Some(old) = self.entries.get(&owner.participant) {
            return if old.owner == owner {
                Ok(None)
            } else {
                Err(Error::DuplicateObject)
            };
        }
        if self.entries.len() >= MAX_INSTANCES {
            return Err(Error::Bound);
        }
        let fallback_at = self.world_ms.checked_add(FALLBACK_MS).ok_or(Error::Bound)?;
        let creation = self.definition.creation(
            players.objects(),
            owner.participant,
            self.entries.len() as u32,
        )?;
        let record = players
            .spawn_entities(vec![creation], &self.content)?
            .remove(0);
        self.entries.insert(
            owner.participant,
            Instance {
                owner,
                ghost: record.id,
                phase: Phase::Running { fallback_at },
                streaming: None,
                garage_loaded: false,
                ready_at: None,
            },
        );
        Ok(Some(record))
    }
    pub fn has_endpoint(&self, ghost: u16, selector: u16) -> bool {
        selector == 0 && self.entries.values().any(|i| i.ghost == ghost)
    }
    pub fn bind_streaming(
        &mut self,
        players: &Players,
        owner: Owner,
        ghost: u16,
        streaming_asset: Asset,
    ) -> Result<(), Error> {
        let slot = players
            .local_slot(owner.connection, owner.persona, owner.participant)
            .ok_or(Error::UnknownObject)?;
        if !matches!(&players.objects().get(ghost).ok_or(Error::UnknownObject)?.initial,
            replication::Initial::Entity { prefix, fields } if prefix.asset == streaming_asset
                && matches!(fields.first(), Some(entity::Initial::Root { reference, .. }) if *reference == owner.participant))
        {
            return Err(Error::TypeMismatch);
        }
        let instance = self
            .entries
            .get_mut(&owner.participant)
            .ok_or(Error::UnknownObject)?;
        if instance.owner != owner {
            return Err(Error::UnknownObject);
        }
        if let Some(old) = instance.streaming {
            return if old.ghost == ghost && old.slot == slot {
                Ok(())
            } else {
                Err(Error::DuplicateObject)
            };
        }
        instance.streaming = Some(Streaming {
            ghost,
            slot,
            loaded: false,
        });
        Ok(())
    }
    pub fn streaming_event(&mut self, message: &crate::logic::Message) -> Result<bool, Error> {
        let crate::logic::Message::Reached {
            event,
            target,
            player,
        } = message
        else {
            return Ok(false);
        };
        let Some(instance) = self
            .entries
            .values_mut()
            .find(|i| i.streaming.is_some_and(|s| s.ghost == target.ghost))
        else {
            return Ok(false);
        };
        if target.entity != 1 || !matches!(*event, STREAMING_LOADED | STREAMING_UNLOADED) {
            return Ok(false);
        }
        let streaming = instance.streaming.as_ref().expect("matched binding");
        if *player != streaming.slot {
            return Err(Error::UnknownObject);
        }
        let loaded = *event == STREAMING_LOADED;
        let ready_at = instance.deadline(self.world_ms, loaded, instance.garage_loaded)?;
        instance.streaming.as_mut().expect("matched binding").loaded = loaded;
        instance.ready_at = ready_at;
        Ok(true)
    }
    pub fn garage_loaded(&mut self, owner: Owner, loaded: bool) -> Result<(), Error> {
        let instance = self
            .entries
            .get_mut(&owner.participant)
            .ok_or(Error::UnknownObject)?;
        if instance.owner != owner {
            return Err(Error::UnknownObject);
        }
        let ready_at = instance.deadline(
            self.world_ms,
            instance.streaming.is_some_and(|s| s.loaded),
            loaded,
        )?;
        instance.garage_loaded = loaded;
        instance.ready_at = ready_at;
        Ok(())
    }
    pub fn poll(&mut self, world_ms: u64, capacity: usize) -> Result<Vec<Stop>, Error> {
        if world_ms < self.world_ms {
            return Err(Error::Shape);
        }
        self.world_ms = world_ms;
        let mut stops = Vec::new();
        for instance in self.entries.values_mut() {
            if stops.len() < capacity
                && matches!(instance.phase, Phase::Running { fallback_at } if world_ms >= fallback_at || instance.ready_at.is_some_and(|at| world_ms >= at))
            {
                stops.push(Stop {
                    sequence: instance.ghost,
                    serial: Serial::new(1).expect("fresh lifetime"),
                });
                instance.phase = Phase::Stopping;
            }
        }
        Ok(stops)
    }
    pub fn complete(
        &mut self,
        players: &Players,
        owner: Owner,
        call: Completed,
    ) -> Result<bool, Error> {
        call.encode()?;
        if owner.participant != call.participant
            || !players.owns_participant(owner.connection, owner.persona, owner.participant)
        {
            return Err(Error::UnknownObject);
        }
        let instance = self
            .entries
            .get_mut(&call.participant)
            .ok_or(Error::UnknownObject)?;
        if instance.owner != owner || instance.ghost != call.sequence {
            return Err(Error::UnknownObject);
        }
        if matches!(instance.phase, Phase::Running { .. }) {
            return Err(Error::Shape);
        }
        let changed = instance.phase != Phase::Complete;
        instance.phase = Phase::Complete;
        Ok(changed)
    }
    pub fn ghost(&self, participant: u16) -> Option<u16> {
        self.entries.get(&participant).map(|i| i.ghost)
    }
    pub fn is_complete(&self, participant: u16) -> bool {
        self.entries
            .get(&participant)
            .is_some_and(|i| i.phase == Phase::Complete)
    }
}

#[cfg(test)]
mod tests;
