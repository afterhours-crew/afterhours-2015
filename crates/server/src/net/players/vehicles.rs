// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::vehicle_content::population::Owner;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use nfs_world::{
    content::Message,
    garage::vehicle::Loaded,
    participants::{HostRpc, Lifecycle},
};

#[derive(Default)]
pub(super) struct Output {
    pub messages: Vec<Message>,
    pub sections: Vec<Section>,
    pub replies: Vec<HostRpc>,
}
impl PlayerListener {
    pub(super) fn garage_presence(
        roles: Option<&crate::scene_roles::SceneRoles>,
        players: &Players,
        replies: &[HostRpc],
        presence: &mut nfs_world::garage::presence::Presence,
        persona: u64,
    ) -> Result<Vec<HostRpc>, replication::Error> {
        let mut out = Vec::new();
        for reply in replies {
            let HostRpc::Vehicle(vehicle) = reply else {
                continue;
            };
            let endpoint = Self::presence_endpoint(roles, players)?;
            if let Some(notification) = presence.set(endpoint, vehicle.participant, true, |id| {
                players.owns_participant(HOST_SELECTOR as u8, persona, id)
            })? {
                out.push(HostRpc::GaragePresence(notification));
            }
        }
        Ok(out)
    }
    fn presence_endpoint(
        roles: Option<&crate::scene_roles::SceneRoles>,
        players: &Players,
    ) -> Result<nfs_world::participants::Endpoint, replication::Error> {
        use nfs_world::{
            garage::presence::COMPONENT,
            participants::Endpoint,
            replication::{Initial, sublevel},
        };
        let scene = players
            .objects()
            .scene(roles.ok_or(replication::Error::Unsupported)?.garage)
            .ok_or(replication::Error::UnknownObject)?;
        let Some(Initial::SubLevel { fields, .. }) =
            players.objects().get(scene).map(|o| &o.initial)
        else {
            return Err(replication::Error::TypeMismatch);
        };
        let Some(sublevel::Initial::RpcBool { rpc, value: false }) = fields.get(COMPONENT) else {
            return Err(replication::Error::TypeMismatch);
        };
        Ok(Endpoint {
            scene,
            selector: rpc.selector,
            serial: rpc.serial,
        })
    }
    /// A participant leaving the garage: presence off when it was the last one.
    pub(super) fn garage_presence_leave(
        roles: Option<&crate::scene_roles::SceneRoles>,
        players: &Players,
        presence: &mut nfs_world::garage::presence::Presence,
        participant: u16,
        persona: u64,
    ) -> Result<Option<HostRpc>, replication::Error> {
        let endpoint = Self::presence_endpoint(roles, players)?;
        Ok(presence
            .set(endpoint, participant, false, |id| {
                players.owns_participant(HOST_SELECTOR as u8, persona, id)
            })?
            .map(HostRpc::GaragePresence))
    }
    /// Remember the world car of a single exiting participant. The car leads
    /// the exit section; garage logic records may follow it (E758).
    pub(super) fn register_world_car(
        world_cars: &mut BTreeMap<u16, (u16, bool)>,
        exited: &[u16],
        section: &Section,
    ) -> Result<(), replication::Error> {
        let ([participant], Some(record)) = (exited, section.records.first()) else {
            return Ok(());
        };
        if !world_cars.contains_key(&record.id) && world_cars.len() >= MAX_WORLD_CARS {
            return Err(replication::Error::Bound);
        }
        world_cars.insert(record.id, (*participant, false));
        Ok(())
    }
    /// The client's glass condition reports on a world car we created (it
    /// sends three on every new player car; E748/E750/E751). The first one
    /// assigns the participant a spawn point; `Some(None)` is a recognized
    /// report that needs no assignment (repeat, or no SpawnPoints scene bound).
    /// The glass reply itself is the caller's.
    pub(super) fn world_car_ready(
        message: &nfs_world::logic::Message,
        world_cars: &mut BTreeMap<u16, (u16, bool)>,
        spawn_points: &mut nfs_world::spawn_points::SpawnPoints,
    ) -> Result<Option<Option<nfs_world::participants::Notification>>, replication::Error> {
        let nfs_world::logic::Message::Reached { target, .. } = message else {
            return Ok(None);
        };
        if !crate::glass::Glass::recognizes(message) {
            return Ok(None);
        }
        let Some((participant, assigned)) = world_cars.get_mut(&target.ghost) else {
            return Ok(None);
        };
        if *assigned || spawn_points.bindings().is_none() {
            return Ok(Some(None));
        }
        let notification =
            spawn_points.assign(*participant, nfs_world::spawn_points::GARAGE_EXIT)?;
        *assigned = true;
        Ok(Some(Some(notification)))
    }
    /// Garage exit: swap the participant's garage car for its world car in one
    /// section (deletion and creation, absolute origin as for garage cars).
    /// `None` when no world spawn or garage content is configured.
    pub(super) fn exit_world_car(
        players: &mut Players,
        population: Option<&mut Population>,
        inventory: Option<&Inventory>,
        content: Option<&GarageContent>,
        participant: u16,
        persona: u64,
    ) -> Result<Option<(Section, Vec<u16>)>, replication::Error> {
        let (Some(population), Some(inventory), Some(content)) = (population, inventory, content)
        else {
            return Ok(None);
        };
        if content.layout.world_spawn().is_none() {
            return Ok(None);
        }
        let spawned = population.spawn_world(
            players,
            Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant,
            },
            inventory,
            &content.vehicles,
            &content.layout,
        )?;
        if !spawned.messages.is_empty() {
            // The car bundle was registered at garage entry; a new registration
            // here would need content delivery before the creation.
            return Err(replication::Error::Unsupported);
        }
        // The garage car stays on the wire until the client reports the world
        // car; the official host deletes it in a later frame (E742, E759).
        Ok(Some((
            Section {
                float_bits: None,
                flag: false,
                deleted: Vec::new(),
                setup: Some(Setup::RawEscape([0; 3])),
                records: spawned.records,
            },
            spawned.deleted,
        )))
    }
    /// The deferred garage car deletion for a world car's first report.
    pub(super) fn garage_car_deletion(
        message: &nfs_world::logic::Message,
        deferred: &mut BTreeMap<u16, Vec<u16>>,
    ) -> Option<Section> {
        let nfs_world::logic::Message::Reached { target, .. } = message else {
            return None;
        };
        let deleted = deferred.remove(&target.ghost)?;
        Some(Section {
            float_bits: None,
            flag: false,
            deleted,
            setup: None,
            records: Vec::new(),
        })
    }
    pub(super) fn populate(
        players: &mut Players,
        participants: &Lifecycle,
        population: Option<&mut Population>,
        inventory: Option<&Inventory>,
        content: Option<&GarageContent>,
        persona: u64,
    ) -> Result<Output, replication::Error> {
        let (Some(population), Some(inventory), Some(content)) = (population, inventory, content)
        else {
            return Ok(Output::default());
        };
        let mut output = Output::default();
        for participant in participants.waiting_garage() {
            let spawned = population.spawn(
                players,
                Owner {
                    connection: HOST_SELECTOR as u8,
                    persona,
                    participant,
                },
                inventory.clone(),
                &content.vehicles,
                &content.layout,
                [0.; 3],
            )?;
            let mut sections = if spawned.records.is_empty() {
                Vec::new()
            } else {
                split_scenes(
                    spawned.records,
                    nfs_world::application::OUTBOUND_FRAME_BITS - 100,
                )?
            };
            for section in &mut sections {
                section.setup = Some(Setup::RawEscape([0; 3]));
            }
            output.sections.extend(sections);
            output.messages.extend(spawned.messages);
            output
                .replies
                .extend(spawned.bindings.into_iter().map(HostRpc::Vehicle));
        }
        Ok(output)
    }
    pub(super) fn vehicle_loaded(
        population: &mut Population,
        players: &Players,
        persona: u64,
        body: BitSpan<'_>,
    ) -> Result<bool, replication::Error> {
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
        let Some(&scene) = envelope.references().first() else {
            return Ok(false);
        };
        if !population.has_endpoint(scene, route.selector()) || route.method_index() != 1 {
            return Ok(false);
        }
        let loaded = Loaded::decode(body)?;
        population.acknowledge(
            players,
            Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant: loaded.participant,
            },
            loaded,
        )?;
        Ok(true)
    }
}
