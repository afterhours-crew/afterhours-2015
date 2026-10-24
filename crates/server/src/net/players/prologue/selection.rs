// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::net::items::story::{Action, Request};
use crate::sequence_content::execution::{Command as Effect, Execution, Output};
use crate::vehicle_content::population::{Inventory, Population};
use nfs_content::participant_logic::Channel;
use nfs_services::story::rewards::Receipt;
use std::{collections::BTreeSet, sync::Arc};
#[cfg(test)]
pub(super) mod tests;

#[derive(Clone, Debug)]
pub(in crate::net::players) struct State {
    execution: Execution,
    choice: Option<(u16, [u8; 16])>,
    receipt: Option<Receipt>,
    watched: bool,
    unloaded: bool,
}
pub(in crate::net::players) struct Context<'a> {
    pub content: &'a crate::prologue::Content,
    pub vehicles: &'a crate::vehicle_content::GarageContent,
    pub progression: &'a Arc<crate::progression::Content>,
    pub players: &'a mut Players,
    pub population: &'a mut Population,
    pub instances: &'a mut Instances,
    pub owner: Owner,
    pub inventory: Option<&'a Inventory>,
    pub seconds: f64,
}
#[derive(Default)]
pub(in crate::net::players) struct Effects {
    pub calls: Vec<HostRpc>,
    pub jobs: Vec<Request>,
    pub deleted: Vec<u16>,
}
pub(in crate::net::players) fn completed(
    instances: &mut Instances,
    players: &mut Players,
    persona: u64,
    body: BitSpan<'_>,
) -> Result<Option<Option<u16>>, replication::Error> {
    let envelope = Envelope::decode(
        body,
        Limits {
            max_input_bits: 4096,
            max_references: 32,
            max_payload_bytes: 256,
        },
    )
    .map_err(|_| replication::Error::Shape)?;
    let route = envelope
        .route(RouteProfile::ClientSend)
        .map_err(|_| replication::Error::Shape)?;
    let Some(&ghost) = envelope.references().first() else {
        return Ok(None);
    };
    if !instances.has_endpoint(ghost, route.selector()) || route.method_index() != 1 {
        return Ok(None);
    }
    let call = Completed::decode(body)?;
    if (0..route.arguments().len()).any(|b| route.arguments().read_u32(b, 1) != Ok(0)) {
        return Err(replication::Error::Shape);
    }
    instances
        .complete(
            players,
            Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant: call.participant,
            },
            call,
        )
        .map(Some)
}
impl State {
    pub(super) fn new(execution: Execution) -> Self {
        Self {
            execution,
            choice: None,
            receipt: None,
            watched: false,
            unloaded: false,
        }
    }
    pub(in crate::net::players) fn signal(
        &mut self,
        message: &nfs_world::logic::Message,
        context: &mut Context<'_>,
    ) -> Result<Effects, replication::Error> {
        let Some(out) = self
            .execution
            .signal(message, context.seconds)
            .map_err(|_| replication::Error::Shape)?
        else {
            return Ok(Effects::default());
        };
        self.process(out, context)
    }
    pub(super) fn awarded(
        &mut self,
        node: u16,
        receipt: Receipt,
        context: &mut Context<'_>,
    ) -> Result<Effects, replication::Error> {
        if self.choice != Some((node, receipt.definition))
            || self.receipt.is_some_and(|r| r != receipt)
        {
            return Err(replication::Error::Shape);
        }
        self.receipt = Some(receipt);
        let out = self
            .execution
            .awarded(node, context.seconds)
            .map_err(|_| replication::Error::Shape)?;
        self.process(out, context)
    }
    fn process(
        &mut self,
        out: Output,
        context: &mut Context<'_>,
    ) -> Result<Effects, replication::Error> {
        let mut effects = Effects::default();
        let definition = self.execution.definition();
        let link = |node, port| {
            definition
                .links
                .get(&node)
                .and_then(|l| l.get(&port))
                .ok_or(replication::Error::Unsupported)
        };
        let mut unload = BTreeSet::new();
        for command in out.commands {
            match command {
                Effect::Award {
                    node,
                    definition: item,
                } => {
                    if self.choice.is_some() {
                        return Err(replication::Error::Shape);
                    }
                    let service = link(node, 2_612_322_229)?;
                    if service.class != "ItemSystemEntityData" {
                        return Err(replication::Error::TypeMismatch);
                    }
                    let mut relays = definition
                        .links
                        .values()
                        .flat_map(|l| l.values())
                        .filter_map(|l| l.relay.as_ref());
                    let relay = relays.next().ok_or(replication::Error::Unsupported)?;
                    if relays.next().is_some() {
                        return Err(replication::Error::DuplicateObject);
                    }
                    // Check the complete authored success path before the
                    // account worker can commit anything. This dry run remains
                    // private; the actual callback still waits for persistence.
                    let mut candidate = self.execution.clone();
                    let success = candidate
                        .awarded(node, context.seconds)
                        .map_err(|_| replication::Error::Shape)?;
                    let mut selected = false;
                    let mut completed = false;
                    for command in success.commands {
                        match command {
                            Effect::Select { node } if !selected => {
                                let source = link(node, 3_339_738_264)?;
                                let target = link(node, 2_356_970_361)?;
                                let primary = context
                                    .vehicles
                                    .layout
                                    .slot(0)
                                    .ok_or(replication::Error::Shape)?;
                                if source.definition != Some(item)
                                    || target.target
                                        != Some(Channel::Scene {
                                            content_key: context.vehicles.layout.item_scene_key,
                                            serializer: primary.item_component_index,
                                        })
                                {
                                    return Err(replication::Error::Shape);
                                }
                                selected = true;
                            }
                            Effect::ParticipantEvent { node, event } if !completed => {
                                if link(node, 3_253_352_650)?.relay.as_ref() != Some(relay)
                                    || event != relay.selected
                                {
                                    return Err(replication::Error::Shape);
                                }
                                completed = true;
                            }
                            _ => return Err(replication::Error::Unsupported),
                        }
                    }
                    if !selected || !completed {
                        return Err(replication::Error::Shape);
                    }
                    self.choice = Some((node, item));
                    effects.jobs.push(Request {
                        participant: context.owner.participant,
                        player_index: u32::from(
                            context
                                .players
                                .runtime_index(
                                    context.owner.connection,
                                    context.owner.persona,
                                    context.owner.participant,
                                )
                                .ok_or(replication::Error::UnknownObject)?,
                        ),
                        progression: context.progression.clone(),
                        action: Action::Choose {
                            sequence: context
                                .instances
                                .ghost(context.owner.participant)
                                .ok_or(replication::Error::UnknownObject)?,
                            node,
                            objective: relay.objective,
                            definition: item,
                        },
                    });
                }
                Effect::Select { node } => {
                    let receipt = self.receipt.ok_or(replication::Error::Shape)?;
                    let source = link(node, 3_339_738_264)?;
                    let target = link(node, 2_356_970_361)?;
                    let slot = context
                        .vehicles
                        .layout
                        .slot(0)
                        .ok_or(replication::Error::Shape)?;
                    if source.class != "ItemEntityData"
                        || source.definition != Some(receipt.definition)
                        || target.target
                            != Some(Channel::Scene {
                                content_key: context.vehicles.layout.item_scene_key,
                                serializer: slot.item_component_index,
                            })
                        || context.inventory.and_then(|i| i.slots.values()[0])
                            != Some(receipt.item.get())
                    {
                        return Err(replication::Error::Shape);
                    }
                }
                Effect::ParticipantEvent { node, event } => {
                    let relay = link(node, 3_253_352_650)?
                        .relay
                        .as_ref()
                        .ok_or(replication::Error::Unsupported)?;
                    if event == relay.watched {
                        self.watched = true;
                    } else if event == relay.selected {
                        let slot = context
                            .vehicles
                            .layout
                            .slot(0)
                            .ok_or(replication::Error::Shape)?;
                        if self.receipt.is_none()
                            || relay.primary_slot
                                != (Channel::Scene {
                                    content_key: context.vehicles.layout.scene_key,
                                    serializer: slot.component_index,
                                })
                        {
                            return Err(replication::Error::Shape);
                        }
                    } else {
                        return Err(replication::Error::Unsupported);
                    }
                }
                Effect::Unload { node } => {
                    if self.receipt.is_none() {
                        return Err(replication::Error::Shape);
                    }
                    let slot = link(node, 926_267_033)?;
                    let Some(Channel::Scene {
                        content_key,
                        serializer,
                    }) = slot.target
                    else {
                        return Err(replication::Error::Shape);
                    };
                    let previews = context
                        .content
                        .previews
                        .as_ref()
                        .ok_or(replication::Error::Unsupported)?;
                    if slot.class != "VehicleSlotEntityData"
                        || content_key != previews.garage.scene_key
                        || !previews
                            .garage
                            .slots
                            .iter()
                            .any(|s| s.component == serializer)
                        || !unload.insert(serializer)
                    {
                        return Err(replication::Error::Shape);
                    }
                }
                Effect::Stop => {
                    if self.receipt.is_none() || !self.unloaded {
                        return Err(replication::Error::Shape);
                    }
                    if let Some(stop) = context.instances.stop(
                        context
                            .content
                            .selection
                            .as_ref()
                            .ok_or(replication::Error::Unsupported)?,
                        context.players,
                        context.owner,
                    )? {
                        effects.calls.push(HostRpc::SequenceStop(stop));
                    }
                }
            }
        }
        if !unload.is_empty() {
            if unload.len() != 3 {
                return Err(replication::Error::Shape);
            }
            if !self.unloaded {
                effects.deleted = context.population.remove_previews(
                    context.players,
                    crate::vehicle_content::population::Owner {
                        connection: context.owner.connection,
                        persona: context.owner.persona,
                        participant: context.owner.participant,
                    },
                )?;
                self.unloaded = true;
            }
        }
        for message in out.remote {
            let nfs_world::logic::Message::Fire {
                events,
                target,
                player,
            } = message
            else {
                return Err(replication::Error::Shape);
            };
            effects.calls.extend(events.into_iter().map(|event| {
                HostRpc::Event(nfs_world::logic::Fire {
                    event,
                    target,
                    player,
                })
            }));
        }
        Ok(effects)
    }
}
