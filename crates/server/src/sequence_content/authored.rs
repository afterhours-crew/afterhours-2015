// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bind installed sequence channels to one live participant. The caller stages
//! this owner alongside Players and publishes its records/RPCs transactionally.
use nfs_content::{
    events::{self, Binding as ChannelBinding},
    participant_logic::Channel,
    prologue::sequences::Sequence,
};
use nfs_world::{
    logic::Message,
    replication::{
        self, Error, Record,
        entity::{
            self,
            creation::{Asset, Catalog, Content},
        },
        players::Players,
    },
    scoring::event_sync::Binding,
    sequences::{
        Completed, Owner, Stop,
        authored::{Blueprint, Instance},
    },
};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Authored {
    data: Sequence,
    channels: BTreeMap<usize, ChannelBinding>,
    game_mode: Option<Asset>,
    parent: Asset,
    blueprint: Blueprint,
    content: Content,
    stop_signals: Vec<Binding>,
}
impl Authored {
    pub fn new(data: Sequence, parent: Asset, stop_signals: Vec<Binding>) -> Result<Self, Error> {
        let channels = data
            .channels
            .iter()
            .map(|(&i, c)| {
                (
                    i,
                    match *c {
                        Channel::Participant { serializer } => {
                            ChannelBinding::Participant { serializer }
                        }
                        Channel::Scene {
                            content_key,
                            serializer,
                        } => ChannelBinding::Scene {
                            content_key,
                            serializer,
                        },
                    },
                )
            })
            .collect();
        Self::build(data, channels, parent, None, stop_signals)
    }
    /// Sequence spawned by a mode that also binds channels from its parent story.
    pub fn from_mode(
        data: &events::Sequence,
        parent: Asset,
        mode: Asset,
        stop_signals: Vec<Binding>,
    ) -> Result<Self, Error> {
        let channels = data
            .layout
            .fields
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.binding.map(|b| (i, b)))
            .collect();
        Self::build(
            Sequence {
                blueprint: data.layout.blueprint.clone(),
                asset: data.layout.asset,
                catalog: data.layout.catalog.clone(),
                manager: nfs_world::sequences::Component {
                    scene_key: data.manager.0,
                    index: data.manager.1,
                },
                embedded_bus: data.layout.sub_id,
                bus_count: data.layout.bus_count,
                fields: data.layout.fields.iter().map(|f| f.kind).collect(),
                channels: BTreeMap::new(),
            },
            channels,
            parent,
            Some(mode),
            stop_signals,
        )
    }
    fn build(
        data: Sequence,
        channels: BTreeMap<usize, ChannelBinding>,
        parent: Asset,
        game_mode: Option<Asset>,
        stop_signals: Vec<Binding>,
    ) -> Result<Self, Error> {
        let blueprint =
            Blueprint::new(data.asset, data.embedded_bus, data.bus_count, &data.fields)?;
        let fields: Vec<_> = data
            .fields
            .iter()
            .enumerate()
            .filter_map(|(i, k)| (*k == entity::Kind::RpcReference).then_some(i))
            .collect();
        if fields != channels.keys().copied().collect::<Vec<_>>()
            || (game_mode.is_none()
                && channels
                    .values()
                    .any(|c| matches!(c, ChannelBinding::GameMode { .. })))
        {
            return Err(Error::Shape);
        }
        nfs_world::scoring::event_sync::Router::new(1, &stop_signals).map_err(|_| Error::Shape)?;
        let entries = data
            .catalog
            .iter()
            .map(|&(class, count)| {
                Ok((
                    u16::try_from(class).map_err(|_| Error::Bound)?,
                    u16::try_from(count).map_err(|_| Error::Bound)?,
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let content = Content::new(
            vec![Catalog::new(data.asset.bundle, &entries)?],
            vec![(data.asset, blueprint.profile().clone())],
        )?;
        Ok(Self {
            data,
            channels,
            game_mode,
            parent,
            blueprint,
            content,
            stop_signals,
        })
    }
    fn creation(
        &self,
        players: &Players,
        owner: Owner,
        game_mode: Option<u16>,
    ) -> Result<entity::creation::Creation, Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        let world = players.objects();
        let mut parents=world.iter().filter_map(|(id,object)| match &object.initial {
            replication::Initial::Entity{prefix,fields} if prefix.asset==self.parent && matches!(fields.first(),Some(entity::Initial::Root{reference,..}) if *reference==owner.participant) => Some((id,fields)),
            _ => None,
        });
        let (parent, fields) = parents.next().ok_or(Error::UnknownObject)?;
        if parents.next().is_some() {
            return Err(Error::DuplicateObject);
        }
        let manager = world
            .scene_endpoint(self.data.manager.scene_key, self.data.manager.index)
            .ok_or(Error::UnknownObject)?;
        self.blueprint.creation(
            Instance {
                manager,
                participant: owner.participant,
                number: crate::event_logic::live_sequences(world, manager),
            },
            |field| match self.channels.get(&field).ok_or(Error::Shape)? {
                ChannelBinding::GameMode { serializer } => {
                    let ghost=game_mode.ok_or(Error::UnknownObject)?;
                    let replication::Initial::Entity{prefix,fields}=&world.get(ghost).ok_or(Error::UnknownObject)?.initial else {return Err(Error::TypeMismatch)};
                    if Some(prefix.asset)!=self.game_mode || !matches!(fields.first(),Some(entity::Initial::GameModeRoot(root)) if root.participants.contains(&owner.participant)) {return Err(Error::UnknownObject)};
                    Ok((ghost, fields.get(*serializer).and_then(entity::Initial::rpc).ok_or(Error::TypeMismatch)?.selector))
                },
                ChannelBinding::Participant { serializer } => Ok((
                    parent,
                    fields
                        .get(*serializer)
                        .and_then(entity::Initial::rpc)
                        .ok_or(Error::TypeMismatch)?
                        .selector,
                )),
                ChannelBinding::Scene {
                    content_key,
                    serializer,
                } => world
                    .scene_endpoint(*content_key, *serializer)
                    .ok_or(Error::UnknownObject),
            },
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Playing,
    Stopping,
    Complete,
}
#[derive(Clone, Debug)]
struct Active {
    owner: Owner,
    ghost: u16,
    phase: Phase,
}
#[derive(Clone, Debug, Default)]
pub struct Instances {
    entries: BTreeMap<u16, Active>,
}
impl Instances {
    pub fn owner(&self, ghost: u16) -> Option<Owner> {
        self.entries
            .values()
            .find(|e| e.ghost == ghost)
            .map(|e| e.owner)
    }
    pub fn ghost(&self, participant: u16) -> Option<u16> {
        self.entries.get(&participant).map(|e| e.ghost)
    }
    pub fn has_endpoint(&self, ghost: u16, selector: u16) -> bool {
        selector == 0 && self.entries.values().any(|e| e.ghost == ghost)
    }
    pub fn start(
        &mut self,
        content: &Authored,
        players: &mut Players,
        owner: Owner,
    ) -> Result<Option<Record>, Error> {
        self.start_bound(content, players, owner, None)
    }
    pub fn start_bound(
        &mut self,
        content: &Authored,
        players: &mut Players,
        owner: Owner,
        game_mode: Option<u16>,
    ) -> Result<Option<Record>, Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        if let Some(old) = self.entries.get(&owner.participant) {
            return if old.owner == owner {
                Ok(None)
            } else {
                Err(Error::UnknownObject)
            };
        }
        if self.entries.len() >= nfs_world::sequences::MAX_INSTANCES {
            return Err(Error::Bound);
        }
        let creation = content.creation(players, owner, game_mode)?;
        let record = players
            .spawn_entities(vec![creation], &content.content)?
            .remove(0);
        self.entries.insert(
            owner.participant,
            Active {
                owner,
                ghost: record.id,
                phase: Phase::Playing,
            },
        );
        Ok(Some(record))
    }
    /// A validated EventSync stop is emitted once. Repeated/foreign local slots
    /// never advance this owner. A caller with no queue space must not commit it.
    pub fn signal(
        &mut self,
        content: &Authored,
        players: &Players,
        owner: Owner,
        message: &Message,
    ) -> Result<Option<Stop>, Error> {
        let Some(active) = self.entries.get_mut(&owner.participant) else {
            return Ok(None);
        };
        if active.owner != owner
            || !players.owns_participant(owner.connection, owner.persona, owner.participant)
        {
            return Err(Error::UnknownObject);
        }
        let Message::Reached {
            target,
            event,
            player,
        } = message
        else {
            return Ok(None);
        };
        if target.ghost != active.ghost
            || players.local_slot(owner.connection, owner.persona, owner.participant)
                != Some(*player)
            || !content
                .stop_signals
                .iter()
                .any(|b| b.entity == target.entity && b.event == *event)
        {
            return Ok(None);
        }
        self.stop(content, players, owner)
    }
    /// Stop requested by the owned server interpreter after its authored graph
    /// conditions have been met. Shares the native handshake with EventSync.
    pub fn stop(
        &mut self,
        content: &Authored,
        players: &Players,
        owner: Owner,
    ) -> Result<Option<Stop>, Error> {
        let active = self
            .entries
            .get_mut(&owner.participant)
            .ok_or(Error::UnknownObject)?;
        if active.owner != owner
            || !players.owns_participant(owner.connection, owner.persona, owner.participant)
        {
            return Err(Error::UnknownObject);
        }
        if active.phase != Phase::Playing {
            return Ok(None);
        }
        let replication::Initial::Entity { prefix, fields } = &players
            .objects()
            .get(active.ghost)
            .ok_or(Error::UnknownObject)?
            .initial
        else {
            return Err(Error::TypeMismatch);
        };
        if prefix.asset != content.data.asset {
            return Err(Error::TypeMismatch);
        }
        let Some(entity::Initial::SequenceRoot {
            rpc, participants, ..
        }) = fields.first()
        else {
            return Err(Error::TypeMismatch);
        };
        if participants != &[owner.participant] {
            return Err(Error::UnknownObject);
        }
        let stop = Stop {
            sequence: active.ghost,
            serial: rpc.serial,
        };
        stop.encode()?;
        active.phase = Phase::Stopping;
        Ok(Some(stop))
    }
    /// The native completed RPC must follow Stop. Return the deletion only once;
    /// the caller advances its participant lifecycle in the same transaction.
    pub fn complete(
        &mut self,
        players: &mut Players,
        owner: Owner,
        call: Completed,
    ) -> Result<Option<u16>, Error> {
        let active = self
            .entries
            .get_mut(&owner.participant)
            .ok_or(Error::UnknownObject)?;
        if active.owner != owner
            || call.participant != owner.participant
            || call.sequence != active.ghost
            || !players.owns_participant(owner.connection, owner.persona, owner.participant)
        {
            return Err(Error::UnknownObject);
        }
        match active.phase {
            Phase::Playing => return Err(Error::Shape),
            Phase::Complete => return Ok(None),
            Phase::Stopping => {}
        }
        players.remove_entities(&[active.ghost])?;
        active.phase = Phase::Complete;
        Ok(Some(active.ghost))
    }
}

#[cfg(test)]
mod tests;
