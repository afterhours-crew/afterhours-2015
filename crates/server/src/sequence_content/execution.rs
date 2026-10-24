// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Transactional execution of an installed sequence. Commands are requests;
//! only the enclosing owner may persist awards or change garage state.
use nfs_content::prologue::sequences::program::{Definition, Service};
use nfs_world::{
    logic::{EntityRef, Message},
    scoring::{
        self,
        events::{
            Endpoint,
            native::{self, Host},
        },
        interpreter::{self, hosted::Action},
        nodes::OUT,
    },
};
use std::{collections::BTreeMap, sync::Arc};

pub const STARTED: u32 = 216_863_469;
pub const STOP: u32 = 2_089_401_213;
pub const AWARD: u32 = 0xa939_2790;
pub const AWARDED: u32 = 3_137_775_975;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Stop,
    Award { node: u16, definition: [u8; 16] },
    Select { node: u16 },
    Unload { node: u16 },
    ParticipantEvent { node: u16, event: u32 },
}
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub remote: Vec<Message>,
    pub commands: Vec<Command>,
}
#[derive(Clone, Debug)]
struct Services {
    definitions: BTreeMap<u16, Service>,
    commands: Vec<Command>,
    awards: BTreeMap<u16, bool>,
    stopping: bool,
}
impl Host for Services {
    fn invoke(
        &mut self,
        request: native::Request,
        _: &mut dyn native::Callbacks,
    ) -> Result<(), scoring::Error> {
        let service = self
            .definitions
            .get(&request.node)
            .ok_or(scoring::Error::InvalidGraph)?;
        let command = match request.call {
            native::Call::Bind | native::Call::Activate | native::Call::Retire => return Ok(()),
            native::Call::Event { event, .. } => match (service, event) {
                (Service::Root, STOP) => Command::Stop,
                (Service::AwardItem { definition }, AWARD) => Command::Award {
                    node: request.node,
                    definition: *definition,
                },
                (Service::ItemSelector, 0xc6f6_466d) => Command::Select { node: request.node },
                (Service::VehicleSlotLoader, 2_968_292_408) => {
                    Command::Unload { node: request.node }
                }
                (Service::ParticipantEvent, 3_135_534_132 | 1_055_657_837) => {
                    Command::ParticipantEvent {
                        node: request.node,
                        event,
                    }
                }
                _ => return Err(scoring::Error::UnsupportedEvent),
            },
            _ => return Err(scoring::Error::UnsupportedEvent),
        };
        if self.commands.len() >= 64 {
            return Err(scoring::Error::Bound);
        }
        match command {
            Command::Award { node, .. } => {
                if !self.awards.is_empty() {
                    return Ok(());
                }
                self.awards.insert(node, false);
            }
            Command::Stop => {
                if self.stopping {
                    return Ok(());
                }
                self.stopping = true;
            }
            _ => {}
        }
        self.commands.push(command);
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Execution {
    definition: Arc<Definition>,
    graph: interpreter::Session,
    services: Services,
    ghost: u16,
    participant: u16,
    slot: u8,
}
impl Execution {
    pub fn definition(&self) -> &Definition {
        &self.definition
    }
    pub fn start(
        definition: Arc<Definition>,
        ghost: u16,
        participant: u16,
        slot: u8,
        now: f64,
    ) -> Result<(Self, Output), scoring::Error> {
        if ghost == 0 || ghost > 8191 || participant == 0 || participant > 8191 || slot > 7 {
            return Err(scoring::Error::Bound);
        }
        let graph = interpreter::Session::new(Arc::new(definition.program.clone()))?;
        let mut this = Self {
            services: Services {
                definitions: definition.services.clone(),
                commands: vec![],
                awards: BTreeMap::new(),
                stopping: false,
            },
            graph,
            definition,
            ghost,
            participant,
            slot,
        };
        let mut output = this.apply(Action::Start, now)?;
        let started = this.apply(
            Action::Graph(native::Operation::Emit {
                source: Endpoint {
                    node: this.definition.root,
                    event: STARTED,
                },
                context: None,
            }),
            now,
        )?;
        output.remote.extend(started.remote);
        output.commands.extend(started.commands);
        Ok((this, output))
    }
    fn apply(&mut self, action: Action, now: f64) -> Result<Output, scoring::Error> {
        let mut next = self.clone();
        let input = interpreter::Inputs {
            participant: u64::from(self.participant),
            vehicle: None,
            others: vec![],
            road: None,
            stats: None,
            now,
            time_scale: 1.0,
        };
        let output = next.graph.hosted(
            action,
            &input,
            native::Scope {
                instance: u64::from(self.ghost),
                host: &mut next.services,
                upstream: None,
            },
        )?;
        // This sequence subset must not silently lose other domain effects.
        if !output.ai.is_empty()
            || !output.awards.is_empty()
            || !output.bank_requests.is_empty()
            || !output.transitions.is_empty()
            || !output.channel_events.is_empty()
            || !output.participant_properties.is_empty()
            || !output.participant_sets.is_empty()
            || !output.commands.is_empty()
            || !output.synced.is_empty()
            || !output.teleport_locations.is_empty()
            || !output.participant_names.is_empty()
            || !output.rank_queries.is_empty()
            || !output.burnout_overrides.is_empty()
        {
            return Err(scoring::Error::UnsupportedEvent);
        }
        let mut remote = vec![];
        for emission in output.remote {
            if emission.context.is_some() {
                return Err(scoring::Error::UnsupportedEvent);
            }
            for row in emission.connections {
                let entity = self
                    .definition
                    .composition
                    .bus_entities
                    .get(usize::from(row.bus))
                    .copied()
                    .flatten()
                    .ok_or(scoring::Error::InvalidGraph)?;
                let message = Message::Fire {
                    events: vec![u32::from(row.index)],
                    target: EntityRef {
                        ghost: self.ghost,
                        entity: u32::from(entity),
                    },
                    player: None,
                };
                message.encode().map_err(|_| scoring::Error::Bound)?;
                remote.push(message);
            }
        }
        let commands = std::mem::take(&mut next.services.commands);
        *self = next;
        Ok(Output { remote, commands })
    }
    pub fn signal(
        &mut self,
        message: &Message,
        now: f64,
    ) -> Result<Option<Output>, scoring::Error> {
        if self.services.stopping {
            return Ok(None);
        }
        let Message::Reached { player, .. } = message else {
            return Ok(None);
        };
        if *player != self.slot {
            return Ok(None);
        }
        let router = scoring::event_sync::Router::new(self.ghost, &self.definition.signals)?;
        let mut players = [None; 8];
        players[usize::from(self.slot)] = Some(self.participant);
        let Some(route) = router.route(message, &players) else {
            return Ok(None);
        };
        self.apply(
            Action::Graph(native::Operation::Emit {
                source: Endpoint {
                    node: route.node,
                    event: OUT,
                },
                context: route.context.map(scoring::events::Context::Entity),
            }),
            now,
        )
        .map(Some)
    }
    /// Only the owner calls this after the matching durable grant has committed.
    pub fn awarded(&mut self, node: u16, now: f64) -> Result<Output, scoring::Error> {
        if !matches!(
            self.services.definitions.get(&node),
            Some(Service::AwardItem { .. })
        ) {
            return Err(scoring::Error::InvalidGraph);
        }
        match self.services.awards.get(&node) {
            Some(true) => return Ok(Output::default()),
            Some(false) => {}
            None => return Err(scoring::Error::InvalidGraph),
        }
        let mut next = self.clone();
        next.services.awards.insert(node, true);
        let out = next.apply(
            Action::Graph(native::Operation::Emit {
                source: Endpoint {
                    node,
                    event: AWARDED,
                },
                context: None,
            }),
            now,
        )?;
        *self = next;
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests;
