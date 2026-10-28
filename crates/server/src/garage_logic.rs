// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod reputation;
use crate::Failure;
use nfs_protocol::world::rpc::Serial;
use nfs_world::logic::{EntityRef, Fire};
use nfs_world::replication::{
    self, Record, Update as RecordUpdate,
    entity::{
        Body, Initial, Kind, Rpc, Update,
        creation::{Asset, Content, Creation, MAX_PROFILES, Prefix},
    },
    players::Players,
    sublevel,
};
pub use reputation::{Field as ReputationField, Scores as ReputationScores};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::{fs::File, io::Read, path::Path};

pub const FORMAT: &str = "nfs-garage-logic";
pub const VERSION: u64 = 1;
pub const SUB_ID: u32 = 1;
const MAX_ENTITIES: usize = 64;
const MAX_UPDATES: usize = 16;
const MAX_FIELDS: usize = 512;
const MAX_EVENTS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Blueprint {
    Scene(u32),
    Entity(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Root {
        bus_count: u16,
    },
    Channel {
        scene_content_key: u32,
        scene_serializer: usize,
    },
    EntityChannel {
        capture_target: u16,
        target_selector: u16,
    },
    Rpc,
    Variant {
        tag: u8,
        value: Option<u32>,
    },
    EmptyVariant,
    Bool(Option<bool>),
    Int(Option<i32>),
    Float(Option<u32>),
    Reputation(ReputationField),
    ReputationLevelStat,
    Flags([bool; 2]),
}
impl Role {
    fn kind(self) -> Kind {
        match self {
            Self::Root { .. } => Kind::Root,
            Self::Channel { .. } | Self::EntityChannel { .. } => Kind::RpcReference,
            Self::Rpc => Kind::Rpc,
            Self::Variant { .. } | Self::EmptyVariant | Self::ReputationLevelStat => {
                Kind::RpcVariant
            }
            Self::Bool(_) => Kind::BoolProperty,
            Self::Int(_) | Self::Reputation(_) => Kind::I32Property,
            Self::Float(_) => Kind::FloatProperty,
            Self::Flags(_) => Kind::RpcFlags,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Definition {
    pub capture_id: u16,
    pub asset: Asset,
    pub blueprint: Blueprint,
    pub roles: Vec<Role>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneUpdate {
    pub scene_content_key: u32,
    pub fields: Vec<(usize, i32)>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventTarget {
    Capture(u16),
    Scene(u32),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event {
    pub target: EventTarget,
    pub entity: u32,
    pub event: u32,
}

/// FadeControlPresentationBlueprint event that activates ViewWorld (E824).
pub const VIEW_WORLD: u32 = 0;

fn presentation_fire(
    players: &Players,
    ghost: u16,
    entity: u32,
    event: u32,
) -> Result<Fire, replication::Error> {
    if players.objects().get(ghost).is_none() {
        return Err(replication::Error::UnknownObject);
    }
    let fire = Fire {
        event,
        player: None,
        target: EntityRef { ghost, entity },
    };
    fire.encode().map_err(|_| replication::Error::Shape)?;
    Ok(fire)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Batch {
    pub scene_updates: Vec<SceneUpdate>,
    pub entities: Vec<Definition>,
    /// Boolean property changes on live entities: (capture, field, value).
    pub entity_updates: Vec<(u16, usize, bool)>,
    pub delete: Vec<u16>,
    pub events: Vec<Event>,
}

/// The part of a batch to produce: the official host spreads a garage
/// return's batch over time (E921: the free-roam presentation is deleted one
/// frame after the confirming request, the returning entities and events
/// follow about 1.4 s later).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Part {
    All,
    /// Only the batch's deletions.
    Deletions,
    /// Everything but the deletions: scene and entity updates, entities, events.
    Creations,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Which {
    Spawn,
    Entry,
    Vehicles,
    /// Garage exit: the official host deletes the garage-only participant
    /// logic and recreates the loading presentation for the world load
    /// (E742 exit frame, decoded for E757).
    Exit,
    /// Arrive sequence start: the official host deletes the lock-vehicle-input
    /// and loading-screen presentation logic in the frame that creates the
    /// sequence (E101 109,092 ms: ghosts 421 and 422, 3404/65 and 3404/35).
    Arrive,
    /// Arrive sequence completion: the remaining exit presentation logic goes
    /// with the sequence (E101 115,292 ms: ghost 423, 3404/18).
    Arrived,
    /// First readiness report from the participant's newly created world car.
    WorldReady,
    /// Spawn assignment released after the world car is ready.
    WorldSpawn,
    /// First spawn-zone report has settled; queued with the vehicle replies.
    WorldSettled,
    /// Completion presentation and world membership have been queued.
    FreeRoam,
    /// Hide world presentation and hold the black screen during relocation.
    InWorldTeleport,
    /// Release the black screen as the in-world arrival sequence starts.
    InWorldArrival,
    /// Prepare the return cinematic and retire driving-only presentation.
    Returning,
    ReturnRequested,
    /// Replace return presentation with the ordinary garage loading context.
    Returned,
    Crashed,
    Recovered,
    /// Hide relocation until the client acknowledges the destination.
    Resetting,
    ResetArrived,
}

#[derive(Clone, Debug)]
pub struct GarageLogic {
    content: Content,
    reputation: Option<reputation::Thresholds>,
    spawn: Batch,
    /// Extra spawn logic of a prologue participant (installed predicates and
    /// official career start E1026 34.631–34.919 s): created with the spawn
    /// events' captures; their own events follow the spawn events.
    intro_spawn: Batch,
    /// Logic a prologue participant gains when the intro hands over to
    /// normal play (official E1026 1247.825 s: the challenge root, with the
    /// event canceller's shutdown).
    intro_handoff: Batch,
    vehicles: Batch,
    entry: Batch,
    exit: Batch,
    arrive: Batch,
    arrived: Batch,
    world_ready: Batch,
    world_spawn: Batch,
    world_settled: Batch,
    free_roam: Batch,
    in_world_teleport: Batch,
    in_world_arrival: Batch,
    returning: Batch,
    return_requested: Batch,
    /// Installed client EventSync that confirms the requested return.
    return_confirmation: Option<Event>,
    returned: Batch,
    crashed: Batch,
    recovered: Batch,
    resetting: Batch,
    reset_arrived: Batch,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Output {
    pub records: Vec<Record>,
    pub deleted: Vec<u16>,
    pub events: Vec<Fire>,
}

fn keys(v: &Json, expected: &[&str]) -> Result<(), Failure> {
    let map = v.as_object().ok_or(Failure::ProfileConfig)?;
    if map.len() != expected.len() || !expected.iter().all(|k| map.contains_key(*k)) {
        return Err(Failure::ProfileConfig);
    }
    Ok(())
}
fn uint<T: TryFrom<u64>>(v: &Json) -> Result<T, Failure> {
    v.as_u64()
        .and_then(|n| T::try_from(n).ok())
        .ok_or(Failure::ProfileConfig)
}
fn ghost_id(v: &Json) -> Result<u16, Failure> {
    let id: u16 = uint(v)?;
    if id == 0 || id > 8191 {
        return Err(Failure::ProfileConfig);
    }
    Ok(id)
}
fn role(v: &Json) -> Result<Role, Failure> {
    Ok(match v["kind"].as_str().ok_or(Failure::ProfileConfig)? {
        "root" => {
            keys(v, &["kind", "bus_count"])?;
            Role::Root {
                bus_count: uint(&v["bus_count"])?,
            }
        }
        "channel" => {
            keys(v, &["kind", "scene_content_key", "scene_serializer"])?;
            let index = uint::<usize>(&v["scene_serializer"])?;
            if !(1..MAX_FIELDS).contains(&index) {
                return Err(Failure::ProfileConfig);
            }
            Role::Channel {
                scene_content_key: uint::<u32>(&v["scene_content_key"])?.max(1),
                scene_serializer: index,
            }
        }
        "entity_channel" => {
            keys(v, &["kind", "capture_target", "target_selector"])?;
            Role::EntityChannel {
                capture_target: ghost_id(&v["capture_target"])?,
                target_selector: uint(&v["target_selector"])?,
            }
        }
        "rpc" => {
            keys(v, &["kind"])?;
            Role::Rpc
        }
        "variant" => {
            keys(v, &["kind", "tag", "value"])?;
            if v["tag"].is_null() && v["value"].is_null() {
                return Ok(Role::EmptyVariant);
            }
            let tag: u8 = uint(&v["tag"])?;
            let value = if v["value"].is_null() {
                None
            } else {
                Some(uint(&v["value"])?)
            };
            if (tag <= 2) != value.is_some() {
                return Err(Failure::ProfileConfig);
            }
            Role::Variant { tag, value }
        }
        "bool" => {
            keys(v, &["kind", "value"])?;
            Role::Bool(match &v["value"] {
                Json::Null => None,
                Json::Bool(b) => Some(*b),
                _ => return Err(Failure::ProfileConfig),
            })
        }
        "int" => {
            keys(v, &["kind", "value"])?;
            Role::Int(match &v["value"] {
                Json::Null => None,
                other => Some(
                    other
                        .as_i64()
                        .and_then(|n| i32::try_from(n).ok())
                        .ok_or(Failure::ProfileConfig)?,
                ),
            })
        }
        "float" => {
            keys(v, &["kind", "bits"])?;
            Role::Float(if v["bits"].is_null() {
                None
            } else {
                Some(uint(&v["bits"])?)
            })
        }
        "reputation" => {
            keys(v, &["kind", "field"])?;
            Role::Reputation(match v["field"].as_str() {
                Some("level") => ReputationField::Level,
                Some("build") => ReputationField::Build,
                Some("crew") => ReputationField::Crew,
                Some("outlaw") => ReputationField::Outlaw,
                Some("speed") => ReputationField::Speed,
                Some("style") => ReputationField::Style,
                Some("total") => ReputationField::Total,
                Some("next_threshold") => ReputationField::NextThreshold,
                Some("current_threshold") => ReputationField::CurrentThreshold,
                _ => return Err(Failure::ProfileConfig),
            })
        }
        "reputation_level_stat" => {
            keys(v, &["kind"])?;
            Role::ReputationLevelStat
        }
        "flags" => {
            keys(v, &["kind", "flags"])?;
            let flags = v["flags"]
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or(Failure::ProfileConfig)?;
            Role::Flags([
                flags[0].as_bool().ok_or(Failure::ProfileConfig)?,
                flags[1].as_bool().ok_or(Failure::ProfileConfig)?,
            ])
        }
        _ => return Err(Failure::ProfileConfig),
    })
}
fn asset(v: &Json) -> Result<Asset, Failure> {
    keys(v, &["bundle", "type_id", "local_index"])?;
    Ok(Asset {
        bundle: uint(&v["bundle"])?,
        type_id: uint(&v["type_id"])?,
        local_index: uint(&v["local_index"])?,
    })
}
fn batch(v: &Json, content: &Content, earlier: &BTreeSet<u16>) -> Result<Batch, Failure> {
    let o = v.as_object().ok_or(Failure::ProfileConfig)?;
    if !o.contains_key("scene_updates") || !o.contains_key("entities") {
        return Err(Failure::ProfileConfig);
    }
    let scene_updates = v["scene_updates"]
        .as_array()
        .filter(|a| a.len() <= MAX_UPDATES)
        .ok_or(Failure::ProfileConfig)?
        .iter()
        .map(|u| {
            keys(u, &["scene_content_key", "fields"])?;
            let fields = u["fields"]
                .as_array()
                .filter(|a| !a.is_empty() && a.len() <= MAX_UPDATES)
                .ok_or(Failure::ProfileConfig)?
                .iter()
                .map(|f| {
                    keys(f, &["index", "value"])?;
                    let index = uint::<usize>(&f["index"])?;
                    if index >= MAX_FIELDS {
                        return Err(Failure::ProfileConfig);
                    }
                    let value = f["value"]
                        .as_i64()
                        .and_then(|n| i32::try_from(n).ok())
                        .ok_or(Failure::ProfileConfig)?;
                    Ok((index, value))
                })
                .collect::<Result<Vec<_>, Failure>>()?;
            Ok(SceneUpdate {
                scene_content_key: uint(&u["scene_content_key"])?,
                fields,
            })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    let mut seen = earlier.clone();
    let mut entities = Vec::new();
    for e in v["entities"]
        .as_array()
        .filter(|a| a.len() <= MAX_ENTITIES)
        .ok_or(Failure::ProfileConfig)?
    {
        let o = e.as_object().ok_or(Failure::ProfileConfig)?;
        for k in ["capture_id", "asset", "blueprint", "roles"] {
            if !o.contains_key(k) {
                return Err(Failure::ProfileConfig);
            }
        }
        let capture_id = ghost_id(&e["capture_id"])?;
        let asset = asset(&e["asset"])?;
        let profile = content.profile(asset).map_err(|_| Failure::ProfileConfig)?;
        let blueprint = if let Some(key) = e["blueprint"].get("scene_content_key") {
            Blueprint::Scene(uint::<u32>(key)?.max(1))
        } else if let Some(target) = e["blueprint"].get("capture_entity") {
            let target = ghost_id(target)?;
            if !seen.contains(&target) {
                return Err(Failure::ProfileConfig);
            }
            Blueprint::Entity(target)
        } else {
            return Err(Failure::ProfileConfig);
        };
        let roles = e["roles"]
            .as_array()
            .filter(|a| a.len() == profile.kinds().len() && !a.is_empty())
            .ok_or(Failure::ProfileConfig)?
            .iter()
            .map(role)
            .collect::<Result<Vec<_>, _>>()?;
        if roles[0].kind() != Kind::Root
            || roles
                .iter()
                .filter(|r| matches!(r, Role::Root { .. }))
                .count()
                != 1
        {
            return Err(Failure::ProfileConfig);
        }
        for (role, kind) in roles.iter().zip(profile.kinds()) {
            if role.kind() != *kind {
                return Err(Failure::ProfileConfig);
            }
            if let Role::EntityChannel { capture_target, .. } = role
                && !seen.contains(capture_target)
            {
                return Err(Failure::ProfileConfig);
            }
        }
        if !seen.insert(capture_id) {
            return Err(Failure::ProfileConfig);
        }
        entities.push(Definition {
            capture_id,
            asset,
            blueprint,
            roles,
        });
    }
    let entity_updates = match v.get("entity_updates") {
        None => Vec::new(),
        Some(list) => list
            .as_array()
            .filter(|a| a.len() <= MAX_UPDATES)
            .ok_or(Failure::ProfileConfig)?
            .iter()
            .map(|u| {
                keys(u, &["capture_id", "field", "bool"])?;
                let capture = ghost_id(&u["capture_id"])?;
                // Only entities that exist before this batch: their kinds are
                // checked against the content when the batch is produced.
                if !earlier.contains(&capture) {
                    return Err(Failure::ProfileConfig);
                }
                let field = uint::<usize>(&u["field"])?;
                if field >= MAX_FIELDS {
                    return Err(Failure::ProfileConfig);
                }
                let value = u["bool"].as_bool().ok_or(Failure::ProfileConfig)?;
                Ok((capture, field, value))
            })
            .collect::<Result<Vec<_>, Failure>>()?,
    };
    let delete = match v.get("delete_capture_ids") {
        None => Vec::new(),
        Some(list) => list
            .as_array()
            .filter(|a| a.len() <= MAX_ENTITIES)
            .ok_or(Failure::ProfileConfig)?
            .iter()
            .map(ghost_id)
            .collect::<Result<Vec<_>, _>>()?,
    };
    let mut unique = BTreeSet::new();
    for id in &delete {
        if !earlier.contains(id) || !unique.insert(*id) {
            return Err(Failure::ProfileConfig);
        }
    }
    let events = match v.get("events") {
        None => Vec::new(),
        Some(list) => list
            .as_array()
            .filter(|a| a.len() <= MAX_EVENTS)
            .ok_or(Failure::ProfileConfig)?
            .iter()
            .map(|e| {
                keys(e, &["target", "entity", "event"])?;
                let target = if let Some(id) = e["target"].get("capture_id") {
                    let id = ghost_id(id)?;
                    if !seen.contains(&id) || unique.contains(&id) {
                        return Err(Failure::ProfileConfig);
                    }
                    EventTarget::Capture(id)
                } else if let Some(key) = e["target"].get("scene_content_key") {
                    EventTarget::Scene(uint::<u32>(key)?.max(1))
                } else {
                    return Err(Failure::ProfileConfig);
                };
                Ok(Event {
                    target,
                    entity: uint(&e["entity"])?,
                    event: uint(&e["event"])?,
                })
            })
            .collect::<Result<Vec<_>, Failure>>()?,
    };
    Ok(Batch {
        scene_updates,
        entities,
        entity_updates,
        delete,
        events,
    })
}

/// Captures the spawn batch's own events address.
fn spawn_targets(spawn: &Batch) -> BTreeSet<u16> {
    spawn
        .events
        .iter()
        .filter_map(|e| match e.target {
            EventTarget::Capture(id) => Some(id),
            EventTarget::Scene(_) => None,
        })
        .collect()
}

impl GarageLogic {
    /// Transfer whole participant assets to a state-owned service. Reject any
    /// remaining dependency on their legacy IDs instead of dropping events or
    /// silently inventing a replacement reference.
    pub fn without_assets(&self, assets: &BTreeSet<Asset>) -> Result<Self, replication::Error> {
        let mut next = self.clone();
        let batches = [
            &mut next.spawn,
            &mut next.vehicles,
            &mut next.entry,
            &mut next.exit,
            &mut next.arrive,
            &mut next.arrived,
            &mut next.world_ready,
            &mut next.world_spawn,
            &mut next.world_settled,
            &mut next.free_roam,
            &mut next.in_world_teleport,
            &mut next.in_world_arrival,
            &mut next.returning,
            &mut next.return_requested,
            &mut next.returned,
            &mut next.crashed,
            &mut next.recovered,
            &mut next.resetting,
            &mut next.reset_arrived,
        ];
        let removed: BTreeSet<_> = batches
            .iter()
            .flat_map(|batch| &batch.entities)
            .filter(|definition| assets.contains(&definition.asset))
            .map(|definition| definition.capture_id)
            .collect();
        for batch in &batches {
            if batch.entity_updates.iter().any(|(id, _, _)| removed.contains(id))
                || batch.events.iter().any(|event| matches!(event.target, EventTarget::Capture(id) if removed.contains(&id))) {
                return Err(replication::Error::Unsupported);
            }
            for definition in batch.entities.iter().filter(|d| !assets.contains(&d.asset)) {
                if removed.contains(&definition.capture_id)
                    || matches!(definition.blueprint, Blueprint::Entity(id) if removed.contains(&id))
                    || definition.roles.iter().any(|role| matches!(role, Role::EntityChannel { capture_target, .. } if removed.contains(capture_target))) {
                    return Err(replication::Error::Unsupported);
                }
            }
        }
        for batch in batches {
            batch
                .entities
                .retain(|definition| !assets.contains(&definition.asset));
            batch.delete.retain(|id| !removed.contains(id));
        }
        Ok(next)
    }
    pub fn load(path: &Path) -> Result<Self, Failure> {
        Self::load_with_reputation(path, None)
    }
    /// Supply installation-derived thresholds when the document omits them.
    /// Embedded thresholds remain explicit operator policy and take precedence.
    pub fn load_with_reputation(
        path: &Path,
        default: Option<reputation::Thresholds>,
    ) -> Result<Self, Failure> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| Failure::Output)?
            .take(262_145)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Output)?;
        if bytes.len() > 262_144 {
            return Err(Failure::BodyLimit);
        }
        Self::from_json_with_reputation(
            &serde_json::from_slice(&bytes).map_err(|_| Failure::ProfileConfig)?,
            default,
        )
    }
    pub fn from_json(v: &Json) -> Result<Self, Failure> {
        Self::from_json_with_reputation(v, None)
    }
    pub fn from_json_with_reputation(
        v: &Json,
        default: Option<reputation::Thresholds>,
    ) -> Result<Self, Failure> {
        let o = v.as_object().ok_or(Failure::ProfileConfig)?;
        for k in [
            "format",
            "version",
            "build_sha256",
            "content",
            "spawn",
            "entry",
        ] {
            if !o.contains_key(k) {
                return Err(Failure::ProfileConfig);
            }
        }
        if v["format"] != FORMAT
            || v["version"] != VERSION
            || v["build_sha256"] != crate::content::BUILD
        {
            return Err(Failure::ProfileConfig);
        }
        let content = crate::progression::entities::content(&v["content"])?;
        let spawn = batch(&v["spawn"], &content, &BTreeSet::new())?;
        if !spawn.delete.is_empty() {
            return Err(Failure::ProfileConfig);
        }
        let spawned: BTreeSet<u16> = spawn.entities.iter().map(|e| e.capture_id).collect();
        let intro_spawn = match v.get("intro_spawn") {
            None => Batch::default(),
            Some(b) => {
                let b = batch(b, &content, &BTreeSet::new())?;
                let targets = spawn_targets(&spawn);
                if !b.scene_updates.is_empty()
                    || !b.entity_updates.is_empty()
                    || !b.delete.is_empty()
                    || b.entities.iter().any(|d| targets.contains(&d.capture_id))
                    || b.events
                        .iter()
                        .any(|e| matches!(e.target, EventTarget::Scene(_)))
                {
                    return Err(Failure::ProfileConfig);
                }
                b
            }
        };
        let intro_handoff = match v.get("intro_handoff") {
            None => Batch::default(),
            Some(b) => {
                let b = batch(b, &content, &BTreeSet::new())?;
                let early: BTreeSet<u16> = spawn_targets(&spawn)
                    .into_iter()
                    .chain(intro_spawn.entities.iter().map(|d| d.capture_id))
                    .collect();
                if b != (Batch {
                    entities: b.entities.clone(),
                    ..Batch::default()
                }) || b.entities.iter().any(|d| {
                    early.contains(&d.capture_id)
                        || d.roles
                            .iter()
                            .any(|r| matches!(r, Role::Reputation(_) | Role::ReputationLevelStat))
                }) {
                    return Err(Failure::ProfileConfig);
                }
                b
            }
        };
        let vehicles = match v.get("vehicles") {
            None => Batch::default(),
            Some(b) => {
                let b = batch(b, &content, &spawned)?;
                if !b.entities.is_empty() || !b.scene_updates.is_empty() || !b.delete.is_empty() {
                    return Err(Failure::ProfileConfig);
                }
                b
            }
        };
        let entry = batch(&v["entry"], &content, &spawned)?;
        let entered: BTreeSet<u16> = spawned
            .iter()
            .copied()
            .chain(entry.entities.iter().map(|e| e.capture_id))
            .filter(|id| !entry.delete.contains(id))
            .collect();
        let exit = match v.get("exit") {
            None => Batch::default(),
            Some(b) => batch(b, &content, &entered)?,
        };
        // Arrive batches run after exit; they only see what exit left live.
        let after = |live: &BTreeSet<u16>, b: &Batch| -> BTreeSet<u16> {
            live.iter()
                .copied()
                .chain(b.entities.iter().map(|e| e.capture_id))
                .filter(|id| !b.delete.contains(id))
                .collect()
        };
        let exited = after(&entered, &exit);
        let arrive = match v.get("arrive") {
            None => Batch::default(),
            Some(b) if exit != Batch::default() => batch(b, &content, &exited)?,
            Some(_) => return Err(Failure::ProfileConfig),
        };
        let arrived = match v.get("arrived") {
            None => Batch::default(),
            Some(b) if exit != Batch::default() => batch(b, &content, &after(&exited, &arrive))?,
            Some(_) => return Err(Failure::ProfileConfig),
        };
        // These callback phases only notify existing objects. They cannot
        // create/delete objects or mutate properties while reserving queue space.
        let event_phase = |name: &str, live: &BTreeSet<u16>| {
            let Some(value) = v.get(name) else {
                return Ok(Batch::default());
            };
            let b = batch(value, &content, live)?;
            if !b.entities.is_empty()
                || !b.scene_updates.is_empty()
                || !b.entity_updates.is_empty()
                || !b.delete.is_empty()
            {
                return Err(Failure::ProfileConfig);
            }
            Ok(b)
        };
        let world_ready = event_phase("world_ready", &exited)?;
        let world_spawn = event_phase("world_spawn", &exited)?;
        let world_settled = event_phase("world_settled", &exited)?;
        let free_roam = event_phase("free_roam", &after(&after(&exited, &arrive), &arrived))?;
        let driving = after(&after(&exited, &arrive), &arrived);
        // In-world relocation replaces live presentation roles, including a
        // new lifetime for free-roam statistics. Validate deletions against the
        // old roles and creations/events against the surviving roles.
        let transition = |name: &str, live: &BTreeSet<u16>| -> Result<Batch, Failure> {
            let Some(value) = v.get(name) else {
                return Ok(Batch::default());
            };
            let mut deletion = value.clone();
            deletion["entities"] = serde_json::json!([]);
            deletion["events"] = serde_json::json!([]);
            let deleted = batch(&deletion, &content, live)?.delete;
            let remaining = live
                .difference(&deleted.iter().copied().collect())
                .copied()
                .collect();
            let mut creation = value.clone();
            creation["delete_capture_ids"] = serde_json::json!([]);
            let mut result = batch(&creation, &content, &remaining)?;
            if !result.scene_updates.is_empty() || !result.entity_updates.is_empty() {
                return Err(Failure::ProfileConfig);
            }
            result.delete = deleted;
            Ok(result)
        };
        let replaced = |live: &BTreeSet<u16>, b: &Batch| -> BTreeSet<u16> {
            live.iter()
                .copied()
                .filter(|id| !b.delete.contains(id))
                .chain(b.entities.iter().map(|e| e.capture_id))
                .collect()
        };
        let in_world_teleport = transition("in_world_teleport", &driving)?;
        let loading = replaced(&driving, &in_world_teleport);
        let in_world_arrival = transition("in_world_arrival", &loading)?;
        if !in_world_teleport.entities.is_empty() || !in_world_teleport.delete.is_empty() {
            let arrival = replaced(&loading, &in_world_arrival);
            let restore = batch(&v["arrived"], &content, &arrival)?;
            if after(&arrival, &restore) != driving {
                return Err(Failure::ProfileConfig);
            }
        }
        let crashed = v
            .get("crashed")
            .map(|b| batch(b, &content, &driving))
            .transpose()?
            .unwrap_or_default();
        let recovered = v
            .get("recovered")
            .map(|b| batch(b, &content, &after(&driving, &crashed)))
            .transpose()?
            .unwrap_or_default();
        if v.get("crashed").is_some() != v.get("recovered").is_some()
            || after(&after(&driving, &crashed), &recovered) != driving
        {
            return Err(Failure::ProfileConfig);
        }
        let crashing = after(&driving, &crashed);
        let resetting = v
            .get("resetting")
            .map(|b| batch(b, &content, &crashing))
            .transpose()?
            .unwrap_or_default();
        let reset_arrived = v
            .get("reset_arrived")
            .map(|b| batch(b, &content, &after(&crashing, &resetting)))
            .transpose()?
            .unwrap_or_default();
        if v.get("resetting").is_some() != v.get("reset_arrived").is_some()
            || after(&after(&crashing, &resetting), &reset_arrived) != crashing
        {
            return Err(Failure::ProfileConfig);
        }
        let return_requested = event_phase("return_requested", &driving)?;
        let return_confirmation = v
            .get("return_confirmation")
            .map(|value| {
                keys(value, &["capture_id", "entity", "event"])?;
                let capture = ghost_id(&value["capture_id"])?;
                let entity = uint(&value["entity"])?;
                let event = uint(&value["event"])?;
                if entity == 0
                    || event > 0x1ff_ffff
                    || !driving.contains(&capture)
                    || !return_requested
                        .events
                        .iter()
                        .any(|e| e.target == EventTarget::Capture(capture))
                {
                    return Err(Failure::ProfileConfig);
                }
                Ok(Event {
                    target: EventTarget::Capture(capture),
                    entity,
                    event,
                })
            })
            .transpose()?;
        let returning = v
            .get("returning")
            .map(|b| batch(b, &content, &driving))
            .transpose()?
            .unwrap_or_default();
        let returned = v
            .get("returned")
            .map(|b| batch(b, &content, &after(&driving, &returning)))
            .transpose()?
            .unwrap_or_default();
        if v.get("returned").is_some() {
            // Every cycle must restore the same live roles as the first load;
            // the usual entry/exit phases must never refer to stale lifetimes.
            let loading = after(&after(&driving, &returning), &returned);
            if after(&loading, &entry) != entered {
                return Err(Failure::ProfileConfig);
            }
        }
        let reputation = v
            .get("reputation_thresholds")
            .map(|value| {
                let values = value
                    .as_array()
                    .filter(|v| v.len() <= 256)
                    .ok_or(Failure::ProfileConfig)?;
                let values = values
                    .iter()
                    .map(|v| {
                        v.as_i64()
                            .and_then(|n| i32::try_from(n).ok())
                            .ok_or(Failure::ProfileConfig)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                reputation::Thresholds::new(values).map_err(|_| Failure::ProfileConfig)
            })
            .transpose()?
            .or(default);
        if entry
            .entities
            .iter()
            .chain(&exit.entities)
            .chain(&arrive.entities)
            .chain(&arrived.entities)
            .chain(&returning.entities)
            .chain(&returned.entities)
            .chain(&in_world_teleport.entities)
            .chain(&in_world_arrival.entities)
            .chain(&crashed.entities)
            .chain(&recovered.entities)
            .chain(&resetting.entities)
            .chain(&reset_arrived.entities)
            .flat_map(|d| &d.roles)
            .any(|r| matches!(r, Role::Reputation(_) | Role::ReputationLevelStat))
        {
            return Err(Failure::ProfileConfig);
        }
        for definition in spawn
            .entities
            .iter()
            .chain(&entry.entities)
            .chain(&intro_spawn.entities)
        {
            let owned = definition
                .roles
                .iter()
                .any(|r| matches!(r, Role::Reputation(_) | Role::ReputationLevelStat));
            if owned && reputation.is_none() {
                return Err(Failure::ProfileConfig);
            }
            if definition.asset
                == (Asset {
                    bundle: 1,
                    type_id: 3404,
                    local_index: 55,
                })
            {
                let expected = [
                    (2, ReputationField::Level),
                    (4, ReputationField::Build),
                    (5, ReputationField::Crew),
                    (6, ReputationField::Outlaw),
                    (7, ReputationField::Speed),
                    (8, ReputationField::Style),
                    (9, ReputationField::Total),
                    (11, ReputationField::NextThreshold),
                    (12, ReputationField::CurrentThreshold),
                ];
                if definition.roles.len() != 15
                    || !expected
                        .iter()
                        .all(|(i, f)| definition.roles[*i] == Role::Reputation(*f))
                    || definition.roles[14] != Role::ReputationLevelStat
                {
                    return Err(Failure::ProfileConfig);
                }
            }
        }
        Ok(Self {
            content,
            reputation,
            spawn,
            intro_spawn,
            intro_handoff,
            vehicles,
            entry,
            exit,
            arrive,
            arrived,
            world_ready,
            world_spawn,
            world_settled,
            free_roam,
            in_world_teleport,
            in_world_arrival,
            returning,
            return_requested,
            return_confirmation,
            returned,
            crashed,
            recovered,
            resetting,
            reset_arrived,
        })
    }
    pub fn content(&self) -> &Content {
        &self.content
    }
    pub fn requires_reputation(&self) -> bool {
        self.reputation.is_some()
    }

    pub fn reputation_thresholds(&self) -> Option<&nfs_services::reputation::Thresholds> {
        self.reputation.as_ref()
    }
    pub fn batch(&self, which: Which) -> &Batch {
        match which {
            Which::Spawn => &self.spawn,
            Which::Vehicles => &self.vehicles,
            Which::Entry => &self.entry,
            Which::Exit => &self.exit,
            Which::Arrive => &self.arrive,
            Which::Arrived => &self.arrived,
            Which::WorldReady => &self.world_ready,
            Which::WorldSpawn => &self.world_spawn,
            Which::WorldSettled => &self.world_settled,
            Which::FreeRoam => &self.free_roam,
            Which::InWorldTeleport => &self.in_world_teleport,
            Which::InWorldArrival => &self.in_world_arrival,
            Which::Returning => &self.returning,
            Which::ReturnRequested => &self.return_requested,
            Which::Returned => &self.returned,
            Which::Crashed => &self.crashed,
            Which::Recovered => &self.recovered,
            Which::Resetting => &self.resetting,
            Which::ResetArrived => &self.reset_arrived,
        }
    }
    /// Every integer serializer that presents a reputation value, as
    /// (capture id, field index, reputation field), across all batches.
    pub fn reputation_fields(&self) -> Vec<(u16, usize, ReputationField)> {
        let mut fields = Vec::new();
        for which in [
            Which::Spawn,
            Which::Vehicles,
            Which::Entry,
            Which::Exit,
            Which::Arrive,
            Which::Arrived,
            Which::WorldReady,
            Which::WorldSpawn,
            Which::WorldSettled,
            Which::FreeRoam,
            Which::InWorldTeleport,
            Which::InWorldArrival,
            Which::Returning,
            Which::ReturnRequested,
            Which::Returned,
            Which::Crashed,
            Which::Recovered,
            Which::Resetting,
            Which::ResetArrived,
        ] {
            for definition in &self.batch(which).entities {
                for (index, role) in definition.roles.iter().enumerate() {
                    if let Role::Reputation(field) = role {
                        fields.push((definition.capture_id, index, *field));
                    }
                }
            }
        }
        fields
    }
    /// Whether deployment content describes a batch for this point.
    pub fn has(&self, which: Which) -> bool {
        *self.batch(which) != Batch::default()
    }
    /// Both halves of the crash presentation must be installed together.
    pub fn has_crash_recovery(&self) -> bool {
        !self.crashed.entities.is_empty() && !self.recovered.entities.is_empty()
    }
    /// Whether deployment content describes the garage exit batch.
    pub fn has_exit(&self) -> bool {
        self.exit != Batch::default()
    }
    pub fn produce(
        &self,
        which: Which,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
    ) -> Result<Output, replication::Error> {
        self.produce_with_reputation_part(which, players, participant, ghosts, None, Part::All)
    }
    /// One part of a batch (see [`Part`]).
    pub fn produce_part(
        &self,
        which: Which,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
        part: Part,
    ) -> Result<Output, replication::Error> {
        self.produce_with_reputation_part(which, players, participant, ghosts, None, part)
    }
    pub fn produce_with_reputation(
        &self,
        which: Which,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
        scores: Option<ReputationScores>,
    ) -> Result<Output, replication::Error> {
        self.produce_with_reputation_part(which, players, participant, ghosts, scores, Part::All)
    }
    pub fn produce_with_reputation_part(
        &self,
        which: Which,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
        scores: Option<ReputationScores>,
        part: Part,
    ) -> Result<Output, replication::Error> {
        if participant == 0 {
            return Err(replication::Error::Shape);
        }
        let batch = self.batch(which);
        let values = match (scores, &self.reputation) {
            (Some(scores), Some(thresholds)) => Some(thresholds.evaluate(scores)),
            _ => None,
        };
        if part != Part::Deletions
            && values.is_none()
            && batch
                .entities
                .iter()
                .flat_map(|d| &d.roles)
                .any(|r| matches!(r, Role::Reputation(_) | Role::ReputationLevelStat))
        {
            return Err(replication::Error::Shape);
        }
        let mut output = Output::default();
        if part == Part::Deletions {
            return Self::delete_batch(batch, players, ghosts, output);
        }
        let replaces = matches!(which, Which::InWorldTeleport | Which::InWorldArrival);
        if replaces && part == Part::All {
            output = Self::delete_batch(batch, players, ghosts, output)?;
        }
        for update in &batch.scene_updates {
            let scene = players
                .objects()
                .scene(update.scene_content_key)
                .ok_or(replication::Error::UnknownObject)?;
            let RecordUpdate::SubLevel { profile, .. } = &players
                .objects()
                .get(scene)
                .ok_or(replication::Error::UnknownObject)?
                .current
            else {
                return Err(replication::Error::TypeMismatch);
            };
            let profile = profile.clone();
            let mut fields = vec![None; profile.kinds().len()];
            for &(index, value) in &update.fields {
                let slot = fields.get_mut(index).ok_or(replication::Error::Bound)?;
                *slot = Some(sublevel::Update::I32Property(Some(value)));
            }
            if let Some(record) =
                players.change(scene, RecordUpdate::SubLevel { profile, fields })?
            {
                output.records.push(record);
            }
        }
        for &(capture, field, value) in &batch.entity_updates {
            let ghost = *ghosts
                .get(&capture)
                .ok_or(replication::Error::UnknownObject)?;
            let RecordUpdate::Entity { binding, .. } = &players
                .objects()
                .get(ghost)
                .ok_or(replication::Error::UnknownObject)?
                .current
            else {
                return Err(replication::Error::TypeMismatch);
            };
            let binding = binding.clone();
            let kinds = binding.profile()?.kinds().to_vec();
            if kinds.get(field) != Some(&Kind::BoolProperty) {
                return Err(replication::Error::TypeMismatch);
            }
            let mut fields = vec![None; kinds.len()];
            fields[field] = Some(Update::BoolProperty(Some(value)));
            if let Some(record) = players.change(ghost, RecordUpdate::Entity { binding, fields })? {
                output.records.push(record);
            }
        }
        for definition in &batch.entities {
            let creation = self.creation(definition, players, participant, ghosts, values)?;
            let record = players
                .spawn_entities(vec![creation], &self.content)?
                .remove(0);
            ghosts.insert(definition.capture_id, record.id);
            output.records.push(record);
        }
        if part != Part::Creations && !replaces {
            output = Self::delete_batch(batch, players, ghosts, output)?;
        }
        output.events = self.events(which, players, ghosts)?;
        Ok(output)
    }
    fn delete_batch(
        batch: &Batch,
        players: &mut Players,
        ghosts: &mut BTreeMap<u16, u16>,
        mut output: Output,
    ) -> Result<Output, replication::Error> {
        if batch.delete.is_empty() {
            return Ok(output);
        }
        let ids = batch
            .delete
            .iter()
            .map(|id| {
                ghosts
                    .get(id)
                    .copied()
                    .ok_or(replication::Error::UnknownObject)
            })
            .collect::<Result<Vec<_>, _>>()?;
        players.remove_entities(&ids)?;
        for id in &batch.delete {
            ghosts.remove(id);
        }
        output.deleted = ids;
        Ok(output)
    }
    /// A batch applied to a prologue participant. The intro reuses the
    /// ordinary per-participant presentation lifecycle (official career start,
    /// E1183) but skips the garage entry and exit between its phases, so only
    /// absent captures are created, present ones deleted, and events fired on
    /// existing targets. Scene and entity value updates are not applied.
    pub fn produce_present(
        &self,
        which: Which,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
        part: Part,
    ) -> Result<Output, replication::Error> {
        if participant == 0 {
            return Err(replication::Error::Shape);
        }
        let batch = self.batch(which);
        let mut output = Output::default();
        if part != Part::Deletions {
            // The intro keeps its spawn logic, so an asset it already holds
            // (the return cinematic of capture 15 for capture 20) is reused.
            let held = |asset: &Asset| {
                ghosts.values().any(|&ghost| {
                    matches!(
                        players.objects().get(ghost).map(|o| &o.initial),
                        Some(replication::Initial::Entity { prefix, .. }) if prefix.asset == *asset
                    )
                })
            };
            let missing: Vec<&Definition> = batch
                .entities
                .iter()
                .filter(|d| !ghosts.contains_key(&d.capture_id) && !held(&d.asset))
                .collect();
            for definition in missing {
                let creation = self.creation(definition, players, participant, ghosts, None)?;
                let record = players
                    .spawn_entities(vec![creation], &self.content)?
                    .remove(0);
                ghosts.insert(definition.capture_id, record.id);
                output.records.push(record);
            }
        }
        if part != Part::Creations {
            let present: Vec<u16> = batch
                .delete
                .iter()
                .filter(|id| ghosts.contains_key(id))
                .copied()
                .collect();
            let ids: Vec<u16> = present.iter().map(|id| ghosts[id]).collect();
            if !ids.is_empty() {
                players.remove_entities(&ids)?;
            }
            for id in &present {
                ghosts.remove(id);
            }
            output.deleted = ids;
        }
        if part != Part::Deletions {
            output.events = self.present_events(which, players, ghosts)?;
        }
        Ok(output)
    }
    /// The intro's final garage becomes an ordinary garage at customization:
    /// the official host creates the customization presentation with the
    /// customization Enter (career start 1247625 ms, ghost 7124), so the later
    /// garage exit finds the same logic as after a normal entry (E912 58041 ms
    /// deletes garage stats and that presentation). Captures the exit batch
    /// addresses that the intro created through its own paths are adopted by
    /// asset from the participant's live objects; the entry batch's remaining
    /// entities are created without its fade events, which the composite
    /// Leave presentation already sent. Returns the new records.
    pub fn intro_customization(
        &self,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
    ) -> Result<Vec<Record>, replication::Error> {
        if participant == 0 {
            return Err(replication::Error::Shape);
        }
        let asset_of = |capture: u16| {
            [
                &self.spawn,
                &self.entry,
                &self.returned,
                &self.return_requested,
                &self.returning,
            ]
            .into_iter()
            .flat_map(|b| &b.entities)
            .find(|d| d.capture_id == capture)
            .map(|d| d.asset)
        };
        let owned_by = |players: &Players, asset: &Asset, ghosts: &BTreeMap<u16, u16>| {
            let mut found = players.objects().iter().filter_map(|(id, o)| {
                matches!(&o.initial, replication::Initial::Entity { prefix, fields }
                    if prefix.asset == *asset
                    && matches!(fields.first(), Some(Initial::Root { reference, .. })
                        if *reference == participant))
                .then_some(id)
            });
            match (found.next(), found.next()) {
                (Some(_), Some(_)) => Err(replication::Error::DuplicateObject),
                // Already held under another capture: not adopted twice.
                (Some(id), None) if ghosts.values().any(|&g| g == id) => Ok(None),
                (first, _) => Ok(first),
            }
        };
        let exit = self.batch(Which::Exit);
        let needed: Vec<u16> = exit
            .delete
            .iter()
            .copied()
            .chain(exit.entity_updates.iter().map(|&(capture, _, _)| capture))
            .collect();
        for capture in needed {
            if ghosts.contains_key(&capture) {
                continue;
            }
            let Some(asset) = asset_of(capture) else {
                continue;
            };
            if let Some(id) = owned_by(players, &asset, ghosts)? {
                ghosts.insert(capture, id);
            }
        }
        let mut records = Vec::new();
        for definition in &self.entry.entities {
            if ghosts.contains_key(&definition.capture_id) {
                continue;
            }
            if let Some(id) = owned_by(players, &definition.asset, ghosts)? {
                ghosts.insert(definition.capture_id, id);
                continue;
            }
            let creation = self.creation(definition, players, participant, ghosts, None)?;
            let record = players
                .spawn_entities(vec![creation], &self.content)?
                .remove(0);
            ghosts.insert(definition.capture_id, record.id);
            records.push(record);
        }
        Ok(records)
    }
    /// The spawn logic of a prologue participant: only the captures the spawn
    /// batch's own events address (the fade control and one more in installed
    /// content; official career start 480 and 482), then the startup events.
    /// BlackScreen is held for the intro teleport membership publication. The
    /// rest of the batch is garage presentation; given to an intro player it
    /// held the client in garage loading behind the whole intro (E1184).
    pub fn produce_intro_spawn(
        &self,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
    ) -> Result<Output, replication::Error> {
        self.produce_intro_spawn_with_reputation(players, participant, ghosts, None)
    }
    /// As `produce_intro_spawn`, with the account's reputation for the intro
    /// spawn's reputation logic (required when it has any).
    pub fn produce_intro_spawn_with_reputation(
        &self,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
        scores: Option<ReputationScores>,
    ) -> Result<Output, replication::Error> {
        if participant == 0 {
            return Err(replication::Error::Shape);
        }
        let values = match (scores, &self.reputation) {
            (Some(scores), Some(thresholds)) => Some(thresholds.evaluate(scores)),
            _ => None,
        };
        if values.is_none() && self.intro_requires_reputation() {
            return Err(replication::Error::Shape);
        }
        let targets = spawn_targets(&self.spawn);
        let mut output = Output::default();
        for definition in self
            .spawn
            .entities
            .iter()
            .filter(|d| targets.contains(&d.capture_id))
            .chain(&self.intro_spawn.entities)
        {
            if ghosts.contains_key(&definition.capture_id) {
                return Err(replication::Error::DuplicateObject);
            }
            let creation = self.creation(definition, players, participant, ghosts, values)?;
            let record = players
                .spawn_entities(vec![creation], &self.content)?
                .remove(0);
            ghosts.insert(definition.capture_id, record.id);
            output.records.push(record);
        }
        output.events = self.present_events(Which::Spawn, players, ghosts)?;
        // Intro membership, rather than allocating its controller, causes
        // BlackScreen. The listener publishes that effect after EnterTeleport.
        if let Some(fade) = self.fade_capture().and_then(|id| ghosts.get(&id)) {
            output
                .events
                .retain(|fire| fire.target.ghost != *fade || fire.event != 2);
        }
        for event in &self.intro_spawn.events {
            let EventTarget::Capture(capture) = event.target else {
                return Err(replication::Error::Shape);
            };
            let ghost = *ghosts
                .get(&capture)
                .ok_or(replication::Error::UnknownObject)?;
            output.events.push(presentation_fire(
                players,
                ghost,
                event.entity,
                event.event,
            )?);
        }
        Ok(output)
    }
    /// The drive presentation the arrive sequence completes with (installed
    /// content: AliveState, EventResultRep, DriverDetails); a prologue drive
    /// is live while all of them are.
    pub fn arrived_captures(&self) -> Vec<u16> {
        self.arrived.entities.iter().map(|d| d.capture_id).collect()
    }
    /// The intro's handoff logic, created once when the intro completes;
    /// captures already live are left alone.
    pub fn produce_intro_handoff(
        &self,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
    ) -> Result<Vec<Record>, replication::Error> {
        if participant == 0 {
            return Err(replication::Error::Shape);
        }
        let mut records = Vec::new();
        for definition in &self.intro_handoff.entities {
            if ghosts.contains_key(&definition.capture_id) {
                continue;
            }
            let creation = self.creation(definition, players, participant, ghosts, None)?;
            let record = players
                .spawn_entities(vec![creation], &self.content)?
                .remove(0);
            ghosts.insert(definition.capture_id, record.id);
            records.push(record);
        }
        Ok(records)
    }
    /// Whether the prologue spawn needs the account's reputation.
    pub fn intro_requires_reputation(&self) -> bool {
        self.intro_spawn
            .entities
            .iter()
            .flat_map(|d| &d.roles)
            .any(|r| matches!(r, Role::Reputation(_) | Role::ReputationLevelStat))
    }
    /// A batch's participant events whose capture exists, in authored order.
    /// Scene-wide events belong to the ordinary garage and are left out.
    pub fn present_events(
        &self,
        which: Which,
        players: &Players,
        ghosts: &BTreeMap<u16, u16>,
    ) -> Result<Vec<Fire>, replication::Error> {
        self.batch(which)
            .events
            .iter()
            .filter_map(|e| match e.target {
                EventTarget::Capture(id) => ghosts.get(&id).map(|&ghost| (ghost, e)),
                EventTarget::Scene(_) => None,
            })
            .map(|(ghost, e)| presentation_fire(players, ghost, e.entity, e.event))
            .collect()
    }
    /// Resolve the return callback against this participant's current instance.
    pub fn return_confirmation(
        &self,
        players: &Players,
        ghosts: &BTreeMap<u16, u16>,
    ) -> Result<Option<Fire>, replication::Error> {
        self.return_confirmation
            .map(|event| {
                let EventTarget::Capture(capture) = event.target else {
                    return Err(replication::Error::Shape);
                };
                let ghost = *ghosts
                    .get(&capture)
                    .ok_or(replication::Error::UnknownObject)?;
                presentation_fire(players, ghost, event.entity, event.event)
            })
            .transpose()
    }
    /// The fade presentation logic: the target of the free-roam ViewWorld
    /// event (FadeControlPresentationBlueprint; capture 1 in installed content).
    pub fn fade_capture(&self) -> Option<u16> {
        self.free_roam.events.iter().find_map(|e| match e.target {
            EventTarget::Capture(id) if e.event == VIEW_WORLD && e.entity == 1 => Some(id),
            _ => None,
        })
    }
    /// Events on the participant's fade presentation logic: 0 activates
    /// ViewWorld, 1 deactivates it, 2 activates BlackScreen and 3 deactivates
    /// it (E824). Empty without that logic.
    pub fn fade(
        &self,
        players: &Players,
        ghosts: &BTreeMap<u16, u16>,
        events: &[u32],
    ) -> Result<Vec<Fire>, replication::Error> {
        let Some(&ghost) = self.fade_capture().and_then(|c| ghosts.get(&c)) else {
            return Ok(Vec::new());
        };
        events
            .iter()
            .map(|&event| presentation_fire(players, ghost, 1, event))
            .collect()
    }
    /// Resolve authored event targets from current owned objects without
    /// allocating identities or changing the lifecycle.
    pub fn events(
        &self,
        which: Which,
        players: &Players,
        ghosts: &BTreeMap<u16, u16>,
    ) -> Result<Vec<Fire>, replication::Error> {
        let mut events = Vec::new();
        for event in &self.batch(which).events {
            let ghost = match event.target {
                EventTarget::Capture(id) => {
                    *ghosts.get(&id).ok_or(replication::Error::UnknownObject)?
                }
                EventTarget::Scene(key) => players
                    .objects()
                    .scene(key)
                    .ok_or(replication::Error::UnknownObject)?,
            };
            events.push(presentation_fire(
                players,
                ghost,
                event.entity,
                event.event,
            )?);
        }
        Ok(events)
    }
    fn creation(
        &self,
        definition: &Definition,
        players: &Players,
        participant: u16,
        ghosts: &BTreeMap<u16, u16>,
        reputation: Option<reputation::Values>,
    ) -> Result<Creation, replication::Error> {
        let world = players.objects();
        let blueprint = match definition.blueprint {
            Blueprint::Scene(key) => world.scene(key).ok_or(replication::Error::UnknownObject)?,
            Blueprint::Entity(capture) => *ghosts
                .get(&capture)
                .ok_or(replication::Error::UnknownObject)?,
        };
        let serial = Serial::new(0)
            .ok_or(replication::Error::Shape)?
            .next_initialization();
        let mut selector = 0u16;
        let mut initial = Vec::with_capacity(definition.roles.len());
        let mut updates = Vec::with_capacity(definition.roles.len());
        for role in &definition.roles {
            let mut rpc = || {
                let current = Rpc { selector, serial };
                selector = selector.checked_add(1).ok_or(replication::Error::Bound)?;
                Ok::<_, replication::Error>(current)
            };
            let (value, update) = match *role {
                Role::Root { bus_count } => (
                    Initial::Root {
                        value: bus_count,
                        rpc: rpc()?,
                        reference: participant,
                    },
                    Update::Noop,
                ),
                Role::Channel {
                    scene_content_key,
                    scene_serializer,
                } => {
                    let (ghost, target_selector) = world
                        .scene_endpoint(scene_content_key, scene_serializer)
                        .ok_or(replication::Error::UnknownObject)?;
                    (
                        Initial::RpcReference {
                            rpc: rpc()?,
                            reference: ghost,
                            target_selector,
                        },
                        Update::Noop,
                    )
                }
                Role::EntityChannel {
                    capture_target,
                    target_selector,
                } => (
                    Initial::RpcReference {
                        rpc: rpc()?,
                        reference: *ghosts
                            .get(&capture_target)
                            .ok_or(replication::Error::UnknownObject)?,
                        target_selector,
                    },
                    Update::Noop,
                ),
                Role::Rpc => (Initial::Rpc(rpc()?), Update::Noop),
                Role::Variant { tag, value } => (
                    Initial::RpcVariant {
                        rpc: rpc()?,
                        value: Some((tag, value)),
                    },
                    Update::Noop,
                ),
                Role::EmptyVariant => (
                    Initial::RpcVariant {
                        rpc: rpc()?,
                        value: None,
                    },
                    Update::Noop,
                ),
                Role::Bool(value) => (Initial::BoolProperty(value), Update::BoolProperty(None)),
                Role::Int(value) => (Initial::I32Property(value), Update::I32Property(None)),
                Role::Float(value) => (Initial::FloatProperty(value), Update::FloatProperty(None)),
                Role::Reputation(field) => (
                    Initial::I32Property(Some(
                        reputation.ok_or(replication::Error::Shape)?.get(field),
                    )),
                    Update::I32Property(None),
                ),
                Role::ReputationLevelStat => (
                    Initial::RpcVariant {
                        rpc: rpc()?,
                        value: Some((
                            0,
                            Some(
                                reputation
                                    .ok_or(replication::Error::Shape)?
                                    .get(ReputationField::Level)
                                    as u32,
                            ),
                        )),
                    },
                    Update::Noop,
                ),
                Role::Flags(flags) => (Initial::RpcFlags { rpc: rpc()?, flags }, Update::Noop),
            };
            initial.push(value);
            updates.push(Some(update));
        }
        let creation = Creation {
            prefix: Prefix {
                parent: None,
                blueprint,
                sub_id: SUB_ID,
                owner: None,
                asset: definition.asset,
            },
            body: Body {
                initial: Some(initial),
                updates,
            },
        };
        creation.encode(&self.content)?;
        if ghosts.len() >= MAX_PROFILES {
            return Err(replication::Error::Bound);
        }
        Ok(creation)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nfs_protocol::world::rpc::Serial;
    use nfs_world::replication::{Initial as RecordInitial, players::Request, sublevel::ordinary};
    use serde_json::json;
    #[test]
    fn transferring_an_asset_removes_legacy_creation_and_retirement_but_rejects_dependencies() {
        let original = GarageLogic::from_json(&content_json()).unwrap();
        let asset = |local_index| Asset {
            bundle: 4,
            type_id: 10,
            local_index,
        };
        let transferred = original
            .without_assets(&BTreeSet::from([asset(3)]))
            .unwrap();
        assert!(
            transferred
                .spawn
                .entities
                .iter()
                .all(|definition| definition.asset != asset(3))
        );
        assert!(!transferred.entry.delete.contains(&295));
        assert!(original.entry.delete.contains(&295));
        assert_eq!(transferred.entry.events, original.entry.events);
        // Another entity uses this asset's legacy ID as its parent/channel.
        assert!(matches!(
            original.without_assets(&BTreeSet::from([asset(1)])),
            Err(replication::Error::Unsupported)
        ));
        // A legacy event still targets this asset: moving its lifetime alone
        // cannot silently drop that behavior.
        assert!(matches!(
            original.without_assets(&BTreeSet::from([asset(0)])),
            Err(replication::Error::Unsupported)
        ));
    }
    pub(crate) fn content_json() -> Json {
        json!({
            "format": FORMAT, "version": VERSION, "build_sha256": crate::content::BUILD,
            "content": {"catalogs": [{"bundle": 4, "entries": [[10, 4]]}],
                "profiles": [
                    {"asset": {"bundle": 4, "type_id": 10, "local_index": 0}, "serializers": ["root", "rpc_reference", "i32_property", "rpc_variant"]},
                    {"asset": {"bundle": 4, "type_id": 10, "local_index": 1}, "serializers": ["root", "rpc", "rpc_reference", "bool_property"]},
                    {"asset": {"bundle": 4, "type_id": 10, "local_index": 2}, "serializers": ["root", "rpc_reference"]},
                    {"asset": {"bundle": 4, "type_id": 10, "local_index": 3}, "serializers": ["root", "rpc_flags"]}]},
            "spawn": {
                "scene_updates": [{"scene_content_key": 101, "fields": [{"index": 81, "value": 4}]}],
                "entities": [
                    {"capture_id": 277, "asset": {"bundle": 4, "type_id": 10, "local_index": 0}, "blueprint": {"scene_content_key": 101},
                     "roles": [{"kind": "root", "bus_count": 1}, {"kind": "channel", "scene_content_key": 101, "scene_serializer": 64},
                               {"kind": "int", "value": 43}, {"kind": "variant", "tag": 0, "value": 43}]},
                    {"capture_id": 291, "asset": {"bundle": 4, "type_id": 10, "local_index": 1}, "blueprint": {"scene_content_key": 101},
                     "roles": [{"kind": "root", "bus_count": 1}, {"kind": "rpc"}, {"kind": "channel", "scene_content_key": 101, "scene_serializer": 112}, {"kind": "bool", "value": true}]},
                    {"capture_id": 288, "asset": {"bundle": 4, "type_id": 10, "local_index": 2}, "blueprint": {"capture_entity": 291},
                     "roles": [{"kind": "root", "bus_count": 1}, {"kind": "entity_channel", "capture_target": 291, "target_selector": 1}]},
                    {"capture_id": 295, "asset": {"bundle": 4, "type_id": 10, "local_index": 3}, "blueprint": {"scene_content_key": 101},
                     "roles": [{"kind": "root", "bus_count": 2}, {"kind": "flags", "flags": [false, true]}]}]},
            "entry": {
                "scene_updates": [{"scene_content_key": 101, "fields": [{"index": 80, "value": 3}]}],
                "entities": [
                    {"capture_id": 319, "asset": {"bundle": 4, "type_id": 10, "local_index": 2}, "blueprint": {"scene_content_key": 101},
                     "roles": [{"kind": "root", "bus_count": 1}, {"kind": "channel", "scene_content_key": 101, "scene_serializer": 45}]}],
                "delete_capture_ids": [295],
                "events": [{"target": {"capture_id": 277}, "entity": 1, "event": 3}, {"target": {"scene_content_key": 101}, "entity": 1, "event": 144}]},
            "vehicles": {"scene_updates": [], "entities": [],
                "events": [{"target": {"capture_id": 277}, "entity": 1, "event": 2}],
                "player_events": [11, 9, 7], "player_entity": 2}
        })
    }
    pub(crate) fn presentation_json() -> Json {
        let mut v = content_json();
        let phase = |events: &[u32]| {
            json!({"scene_updates":[],"entities":[],
            "events":events.iter().map(|event| json!({"target":{"capture_id":277},
                "entity":1,"event":event})).collect::<Vec<_>>()})
        };
        v["exit"] = phase(&[10]);
        v["world_ready"] = phase(&[11, 12, 13]);
        v["world_spawn"] = phase(&[14]);
        v["world_settled"] = phase(&[21, 20]);
        v["arrive"] = phase(&[15]);
        v["arrived"] = phase(&[16]);
        v["free_roam"] = phase(&[17]);
        v["in_world_teleport"] = phase(&[22, 23]);
        v["in_world_arrival"] = phase(&[24]);
        v
    }

    #[test]
    fn return_confirmation_binds_the_current_instance_and_rejects_unrelated_roles() {
        let mut v = presentation_json();
        v["return_requested"] = json!({"scene_updates":[], "entities":[],
            "events":[{"target":{"capture_id":277},"entity":1,"event":18}]});
        v["return_confirmation"] = json!({"capture_id":277,"entity":1,"event":33});
        let logic = GarageLogic::from_json(&v).unwrap();
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        assert!(logic.return_confirmation(&players, &ghosts).is_err());
        logic
            .produce(Which::Spawn, &mut players, participant, &mut ghosts)
            .unwrap();
        let expected = logic
            .return_confirmation(&players, &ghosts)
            .unwrap()
            .unwrap();
        assert_eq!(expected.target.ghost, ghosts[&277]);
        assert_eq!(expected.target.entity, 1);
        assert_eq!(expected.event, 33);
        for (key, value) in [("capture_id", 291), ("entity", 0), ("event", 0x200_0000)] {
            let mut invalid = v.clone();
            invalid["return_confirmation"][key] = json!(value);
            assert!(GarageLogic::from_json(&invalid).is_err(), "{key}");
        }
        players.remove_entities(&[expected.target.ghost]).unwrap();
        assert!(logic.return_confirmation(&players, &ghosts).is_err());
    }

    #[test]
    fn repeated_return_restores_live_roles_and_allocates_new_presentation() {
        let mut v = presentation_json();
        v["exit"]["delete_capture_ids"] = json!([319]);
        v["return_requested"] = json!({"scene_updates":[],"entities":[],"events":[{"target":{"capture_id":277},"entity":1,"event":18}]});
        v["returning"] = json!({"scene_updates":[],"entities":[v["spawn"]["entities"][3].clone()]});
        v["returned"] = json!({"scene_updates":[],"entities":[],"events":[{"target":{"capture_id":277},"entity":1,"event":19}]});
        let logic = GarageLogic::from_json(&v).unwrap();
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        for phase in [Which::Spawn, Which::Entry] {
            logic
                .produce(phase, &mut players, participant, &mut ghosts)
                .unwrap();
        }
        let count = players.objects().snapshot().len();
        let roles: Vec<_> = ghosts.keys().copied().collect();
        for _ in 0..3 {
            let old = ghosts[&319];
            for phase in [
                Which::Exit,
                Which::Arrive,
                Which::Arrived,
                Which::ReturnRequested,
                Which::Returning,
                Which::Returned,
                Which::Entry,
            ] {
                logic
                    .produce(phase, &mut players, participant, &mut ghosts)
                    .unwrap();
            }
            assert_ne!(ghosts[&319], old);
            assert!(players.objects().get(old).is_none());
            assert_eq!(players.objects().snapshot().len(), count);
            assert_eq!(ghosts.keys().copied().collect::<Vec<_>>(), roles);
        }
        let mut bad = v.clone();
        bad["returned"]["delete_capture_ids"] = json!([277]);
        assert!(GarageLogic::from_json(&bad).is_err());
        bad = v;
        bad["return_requested"]["delete_capture_ids"] = json!([277]);
        assert!(GarageLogic::from_json(&bad).is_err());
    }

    #[test]
    fn return_batch_parts_split_deletions_from_creations() {
        let mut v = presentation_json();
        v["exit"]["delete_capture_ids"] = json!([319]);
        v["return_requested"] = json!({"scene_updates":[],"entities":[],"events":[{"target":{"capture_id":277},"entity":1,"event":18}]});
        v["returning"] = json!({"scene_updates":[],"entities":[v["spawn"]["entities"][3].clone()],
            "delete_capture_ids":[288],"events":[{"target":{"capture_id":277},"entity":1,"event":19}]});
        let logic = GarageLogic::from_json(&v).unwrap();
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        for phase in [Which::Spawn, Which::Entry, Which::Exit] {
            logic
                .produce(phase, &mut players, participant, &mut ghosts)
                .unwrap();
        }
        let doomed = ghosts[&288];
        let count = players.objects().snapshot().len();
        // Deletions alone: the object goes, nothing is created or fired.
        let out = logic
            .produce_part(
                Which::Returning,
                &mut players,
                participant,
                &mut ghosts,
                Part::Deletions,
            )
            .unwrap();
        assert_eq!(out.deleted, vec![doomed]);
        assert!(out.records.is_empty() && out.events.is_empty());
        assert!(players.objects().get(doomed).is_none());
        assert!(!ghosts.contains_key(&288));
        assert_eq!(players.objects().snapshot().len(), count - 1);
        // Creations alone: the entity and the event, no deletion.
        let out = logic
            .produce_part(
                Which::Returning,
                &mut players,
                participant,
                &mut ghosts,
                Part::Creations,
            )
            .unwrap();
        assert!(out.deleted.is_empty());
        assert_eq!(out.records.len(), 1);
        assert_eq!(
            out.events.iter().map(|e| e.event).collect::<Vec<_>>(),
            vec![19]
        );
        assert!(ghosts.contains_key(&295));
        assert_eq!(players.objects().snapshot().len(), count);
    }

    #[test]
    fn presentation_phases_resolve_current_objects_and_reject_mutations_or_dead_targets() {
        let v = presentation_json();
        let logic = GarageLogic::from_json(&v).unwrap();
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        for phase in [Which::Spawn, Which::Entry, Which::Exit] {
            logic
                .produce(phase, &mut players, participant, &mut ghosts)
                .unwrap();
        }
        let before = players.objects().snapshot();
        let target = ghosts[&277];
        for (phase, expected) in [
            (Which::WorldReady, vec![11, 12, 13]),
            (Which::WorldSpawn, vec![14]),
            (Which::WorldSettled, vec![21, 20]),
            (Which::FreeRoam, vec![17]),
            (Which::InWorldTeleport, vec![22, 23]),
            (Which::InWorldArrival, vec![24]),
        ] {
            let events = logic.events(phase, &players, &ghosts).unwrap();
            assert_eq!(events.iter().map(|e| e.event).collect::<Vec<_>>(), expected);
            assert!(events.iter().all(|e| e.target.ghost == target));
        }
        assert_eq!(players.objects().snapshot(), before);
        players.remove_entities(&[target]).unwrap();
        assert!(logic.events(Which::WorldReady, &players, &ghosts).is_err());
        for phase in [
            "world_ready",
            "world_spawn",
            "world_settled",
            "free_roam",
            "in_world_teleport",
            "in_world_arrival",
        ] {
            let mut bad = v.clone();
            bad[phase]["entity_updates"] = json!([{"capture_id":291,"field":3,"bool":false}]);
            assert!(GarageLogic::from_json(&bad).is_err());
            bad = v.clone();
            bad[phase]["events"][0]["target"] = json!({"capture_id":295});
            assert!(GarageLogic::from_json(&bad).is_err());
        }
    }
    pub(crate) fn world() -> (Players, u16) {
        use nfs_world::replication::sublevel::{Content, gameplay};
        let content = Content::new(vec![(101, gameplay::profile().unwrap())]).unwrap();
        let creation = ordinary::creation(
            1,
            101,
            None,
            Serial::new(0).unwrap(),
            &ordinary::unpopulated(content.profile(101).unwrap()).unwrap(),
            &content,
        )
        .unwrap();
        let mut players = Players::default();
        players
            .create_scenes(&[1], vec![creation], &content)
            .unwrap();
        let player = players
            .create(
                1,
                100,
                Request {
                    name: b"Local".to_vec(),
                    flag: false,
                    slot: 0,
                },
            )
            .unwrap()
            .unwrap();
        let participant = players.join(1, 100, player.id).unwrap().unwrap().id;
        (players, participant)
    }
    pub(crate) fn reputation_json() -> Json {
        let mut v = content_json();
        v["reputation_thresholds"] = json!([0, 10, 25]);
        v["spawn"]["entities"][0]["roles"][2] = json!({"kind":"reputation","field":"level"});
        v["spawn"]["entities"][0]["roles"][3] = json!({"kind":"reputation_level_stat"});
        v
    }

    #[test]
    fn reputation_requires_owned_inputs_and_rejects_captured_roles() {
        let v = reputation_json();
        let logic = GarageLogic::from_json(&v).unwrap();
        let (mut players, participant) = world();
        let before = players.objects().snapshot();
        let mut ghosts = BTreeMap::new();
        assert!(
            logic
                .produce(Which::Spawn, &mut players, participant, &mut ghosts)
                .is_err()
        );
        assert_eq!(players.objects().snapshot(), before);
        assert!(ghosts.is_empty());
        for total in [9, 25] {
            let scores = ReputationScores {
                total,
                build: 1,
                crew: 2,
                outlaw: 3,
                speed: 4,
                style: 5,
            };
            let out = logic
                .produce_with_reputation(
                    Which::Spawn,
                    &mut players.clone(),
                    participant,
                    &mut BTreeMap::new(),
                    Some(scores),
                )
                .unwrap();
            let Some(RecordInitial::Entity { fields, .. }) = &out.records[1].initial else {
                panic!()
            };
            let level = if total == 9 { 1 } else { 3 };
            assert_eq!(fields[2], Initial::I32Property(Some(level)));
            assert!(
                matches!(fields[3],Initial::RpcVariant{value:Some((0,Some(n))),..} if n==level as u32)
            );
        }
        let mut bad = v.clone();
        bad.as_object_mut().unwrap().remove("reputation_thresholds");
        assert!(GarageLogic::from_json(&bad).is_err());
        let mut bad = v.clone();
        bad["spawn"]["entities"][0]["roles"][2]["field"] = json!("unknown");
        assert!(GarageLogic::from_json(&bad).is_err());
        let mut bad = v;
        bad["reputation_thresholds"] = json!([0, 1, 1]);
        assert!(GarageLogic::from_json(&bad).is_err());
    }

    #[test]
    fn installed_thresholds_fill_missing_policy_without_masking_invalid_policy() {
        let mut v = reputation_json();
        let fallback = || Some(reputation::Thresholds::new(vec![0, 100]).unwrap());
        let total = ReputationScores {
            total: 25,
            build: 0,
            crew: 0,
            outlaw: 0,
            speed: 0,
            style: 0,
        };
        let level = |v: &Json| {
            GarageLogic::from_json_with_reputation(v, fallback())
                .unwrap()
                .reputation_thresholds()
                .unwrap()
                .evaluate(total)
                .get(ReputationField::Level)
        };
        assert_eq!(level(&v), 3); // Embedded operator policy wins.
        v.as_object_mut().unwrap().remove("reputation_thresholds");
        assert!(GarageLogic::from_json(&v).is_err());
        assert_eq!(level(&v), 1); // Installed thresholds supply the missing policy.
        v["reputation_thresholds"] = json!([0, 1, 1]);
        assert!(GarageLogic::from_json_with_reputation(&v, fallback()).is_err());
    }

    #[test]
    fn content_validates_structure_roles_and_batch_references() {
        let logic = GarageLogic::from_json(&content_json()).unwrap();
        assert_eq!(logic.batch(Which::Spawn).entities.len(), 4);
        assert_eq!(logic.batch(Which::Entry).delete, vec![295]);
        let mut bad = Vec::new();
        let mut v = content_json();
        v["spawn"]["entities"][2]["blueprint"] = json!({"capture_entity": 300});
        bad.push(v);
        let mut v = content_json();
        v["spawn"]["entities"][0]["roles"][2] = json!({"kind": "bool", "value": null});
        bad.push(v);
        let mut v = content_json();
        v["entry"]["delete_capture_ids"] = json!([319]);
        bad.push(v);
        let mut v = content_json();
        v["spawn"]["entities"][1]["capture_id"] = json!(277);
        bad.push(v);
        let mut v = content_json();
        v["spawn"]["entities"][0]["roles"][3] = json!({"kind": "variant", "tag": 0, "value": null});
        bad.push(v);
        let mut v = content_json();
        v["version"] = json!(2);
        bad.push(v);
        let mut v = content_json();
        v["spawn"]["scene_updates"][0]["fields"] = json!([]);
        bad.push(v);
        let mut v = content_json();
        v["entry"]["events"][0]["target"] = json!({"capture_id": 295});
        bad.push(v);
        let mut v = content_json();
        v["spawn"]["events"] = json!([{"target": {"capture_id": 319}, "entity": 1, "event": 1}]);
        bad.push(v);
        let mut v = content_json();
        v["vehicles"]["player_entity"] = json!(null);
        assert!(GarageLogic::from_json(&v).is_ok());
        for (case, v) in bad.iter().enumerate() {
            assert!(GarageLogic::from_json(v).is_err(), "case{case}");
        }
    }

    #[test]
    fn batches_produce_owned_records_in_order_with_current_identities_and_deletions() {
        let logic = GarageLogic::from_json(&content_json()).unwrap();
        let (mut players, participant) = world();
        let gameplay = players.objects().scene(101).unwrap();
        let mut ghosts = BTreeMap::new();
        let before = players.clone();
        let mut missing = ghosts.clone();
        assert!(
            logic
                .produce(
                    Which::Entry,
                    &mut players.clone(),
                    participant,
                    &mut missing
                )
                .is_err()
        );
        let out = logic
            .produce(Which::Spawn, &mut players, participant, &mut ghosts)
            .unwrap();
        assert_eq!(out.records.len(), 5);
        assert!(out.deleted.is_empty());
        assert_eq!(out.records[0].id, gameplay);
        assert!(out.records[0].initial.is_none());
        let RecordUpdate::SubLevel { fields, .. } = &out.records[0].update else {
            panic!()
        };
        assert_eq!(fields[81], Some(sublevel::Update::I32Property(Some(4))));
        assert_eq!(fields.iter().filter(|f| f.is_some()).count(), 1);
        let ids = out.records[1..].iter().map(|r| r.id).collect::<Vec<_>>();
        let mut allocated = ghosts.values().copied().collect::<Vec<_>>();
        allocated.sort();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(allocated, sorted);
        let Some(RecordInitial::Entity { prefix, fields }) = &out.records[3].initial else {
            panic!()
        };
        assert_eq!(prefix.blueprint, ghosts[&291]);
        assert!(
            matches!(fields[1], Initial::RpcReference { reference, target_selector: 1, .. } if reference == ghosts[&291])
        );
        let Some(RecordInitial::Entity { fields, .. }) = &out.records[1].initial else {
            panic!()
        };
        assert!(
            matches!(fields[0], Initial::Root { value: 1, reference, .. } if reference == participant)
        );
        assert_eq!(fields[2], Initial::I32Property(Some(43)));
        assert!(matches!(
            fields[3],
            Initial::RpcVariant {
                value: Some((0, Some(43))),
                ..
            }
        ));
        let count = players.objects().len();
        let vehicles = logic
            .produce(Which::Vehicles, &mut players, participant, &mut ghosts)
            .unwrap();
        assert!(vehicles.records.is_empty() && vehicles.deleted.is_empty());
        assert_eq!(vehicles.events.len(), 1);
        assert_eq!(vehicles.events[0].event, 2);
        let entry = logic
            .produce(Which::Entry, &mut players, participant, &mut ghosts)
            .unwrap();
        assert_eq!(entry.records.len(), 2);
        assert_eq!(entry.deleted, vec![ids[3]]);
        assert_eq!(
            entry.events,
            vec![
                Fire {
                    event: 3,
                    player: None,
                    target: EntityRef {
                        ghost: ghosts[&277],
                        entity: 1
                    }
                },
                Fire {
                    event: 144,
                    player: None,
                    target: EntityRef {
                        ghost: gameplay,
                        entity: 1
                    }
                },
            ]
        );
        assert!(!ghosts.contains_key(&295) && ghosts.contains_key(&319));
        assert_eq!(players.objects().len(), count);
        assert!(players.objects().get(ids[3]).is_none());
        let mut again = ghosts.clone();
        assert!(
            logic
                .produce(Which::Entry, &mut players.clone(), participant, &mut again)
                .is_err()
        );
        assert_ne!(players.objects().snapshot(), before.objects().snapshot());
    }

    fn with_exit(exit: Json) -> Json {
        let mut v = content_json();
        v["exit"] = exit;
        v
    }

    #[test]
    fn exit_batch_is_optional_and_references_live_captures_only() {
        let logic = GarageLogic::from_json(&content_json()).unwrap();
        assert!(!logic.has_exit());
        let exit = json!({"scene_updates": [], "entities": [
            {"capture_id": 400, "asset": {"bundle": 4, "type_id": 10, "local_index": 2}, "blueprint": {"scene_content_key": 101},
             "roles": [{"kind": "root", "bus_count": 1}, {"kind": "channel", "scene_content_key": 101, "scene_serializer": 45}]}],
            "delete_capture_ids": [277, 319]});
        assert!(
            GarageLogic::from_json(&with_exit(exit.clone()))
                .unwrap()
                .has_exit()
        );
        // Entry already deleted 295; an exit may not delete it again.
        let mut bad = exit.clone();
        bad["delete_capture_ids"] = json!([295]);
        assert!(GarageLogic::from_json(&with_exit(bad)).is_err());
        // Capture ids are unique across phases.
        let mut bad = exit.clone();
        bad["entities"][0]["capture_id"] = json!(319);
        assert!(GarageLogic::from_json(&with_exit(bad)).is_err());
        // Exit entities may not carry owned reputation roles.
        let mut bad = exit;
        bad["entities"][0]["asset"]["local_index"] = json!(0);
        bad["entities"][0]["roles"] = json!([{"kind": "root", "bus_count": 1},
            {"kind": "channel", "scene_content_key": 101, "scene_serializer": 64},
            {"kind": "reputation", "field": "level"}, {"kind": "variant", "tag": 0, "value": 43}]);
        assert!(GarageLogic::from_json(&with_exit(bad)).is_err());
    }

    #[test]
    fn entity_updates_change_bool_properties_of_live_entities() {
        let exit = json!({"scene_updates": [], "entities": [],
            "entity_updates": [{"capture_id": 291, "field": 3, "bool": false}]});
        let logic = GarageLogic::from_json(&with_exit(exit)).unwrap();
        assert!(logic.has_exit());
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        logic
            .produce(Which::Spawn, &mut players, participant, &mut ghosts)
            .unwrap();
        logic
            .produce(Which::Entry, &mut players, participant, &mut ghosts)
            .unwrap();
        let out = logic
            .produce(Which::Exit, &mut players, participant, &mut ghosts)
            .unwrap();
        assert_eq!(out.records.len(), 1);
        assert_eq!(out.records[0].id, ghosts[&291]);
        assert!(out.records[0].initial.is_none());
        let RecordUpdate::Entity { fields, .. } = &out.records[0].update else {
            panic!()
        };
        assert_eq!(fields[3], Some(Update::BoolProperty(Some(false))));
        assert!(fields[..3].iter().all(Option::is_none));
        // The same value again changes nothing.
        let again = logic
            .produce(Which::Exit, &mut players, participant, &mut ghosts)
            .unwrap();
        assert!(again.records.is_empty());
        // A field that is not a bool property, or an entity that is not alive.
        let field = with_exit(json!({"scene_updates": [], "entities": [],
            "entity_updates": [{"capture_id": 291, "field": 2, "bool": false}]}));
        let logic = GarageLogic::from_json(&field).unwrap();
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        logic
            .produce(Which::Spawn, &mut players, participant, &mut ghosts)
            .unwrap();
        logic
            .produce(Which::Entry, &mut players, participant, &mut ghosts)
            .unwrap();
        assert_eq!(
            logic
                .produce(Which::Exit, &mut players, participant, &mut ghosts)
                .err(),
            Some(replication::Error::TypeMismatch)
        );
        let dead = with_exit(json!({"scene_updates": [], "entities": [],
            "entity_updates": [{"capture_id": 295, "field": 1, "bool": false}]}));
        assert!(GarageLogic::from_json(&dead).is_err());
    }

    #[test]
    fn exit_batch_deletes_garage_logic_and_recreates_presentation() {
        let exit = json!({"scene_updates": [], "entities": [
            {"capture_id": 400, "asset": {"bundle": 4, "type_id": 10, "local_index": 2}, "blueprint": {"scene_content_key": 101},
             "roles": [{"kind": "root", "bus_count": 1}, {"kind": "channel", "scene_content_key": 101, "scene_serializer": 45}]}],
            "delete_capture_ids": [319]});
        let logic = GarageLogic::from_json(&with_exit(exit)).unwrap();
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        logic
            .produce(Which::Spawn, &mut players, participant, &mut ghosts)
            .unwrap();
        // Exit before entry: the customization presentation does not exist yet.
        assert!(
            logic
                .produce(
                    Which::Exit,
                    &mut players.clone(),
                    participant,
                    &mut ghosts.clone()
                )
                .is_err()
        );
        logic
            .produce(Which::Entry, &mut players, participant, &mut ghosts)
            .unwrap();
        let presentation = ghosts[&319];
        let out = logic
            .produce(Which::Exit, &mut players, participant, &mut ghosts)
            .unwrap();
        assert_eq!(out.deleted, vec![presentation]);
        assert_eq!(out.records.len(), 1);
        let Some(RecordInitial::Entity { fields, .. }) = &out.records[0].initial else {
            panic!()
        };
        assert!(
            matches!(fields[0], Initial::Root { value: 1, reference, .. } if reference == participant)
        );
        assert!(players.objects().get(presentation).is_none());
        assert_eq!(ghosts.get(&400), Some(&out.records[0].id));
        assert!(!ghosts.contains_key(&319));
        // A second exit has nothing left to delete.
        assert!(
            logic
                .produce(Which::Exit, &mut players, participant, &mut ghosts)
                .is_err()
        );
    }

    #[test]
    fn intro_customization_makes_the_final_garage_exit_like_a_normal_one() {
        let exit = json!({"scene_updates": [], "entities": [
            {"capture_id": 400, "asset": {"bundle": 4, "type_id": 10, "local_index": 2}, "blueprint": {"scene_content_key": 101},
             "roles": [{"kind": "root", "bus_count": 1}, {"kind": "channel", "scene_content_key": 101, "scene_serializer": 45}]}],
            "delete_capture_ids": [319]});
        let logic = GarageLogic::from_json(&with_exit(exit)).unwrap();
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        // The intro holds only its spawn-event targets, never the entry batch.
        logic
            .produce_intro_spawn(&mut players, participant, &mut ghosts)
            .unwrap();
        assert!(!ghosts.contains_key(&319));
        assert!(
            logic
                .produce(
                    Which::Exit,
                    &mut players.clone(),
                    participant,
                    &mut ghosts.clone()
                )
                .is_err()
        );
        let created = logic
            .intro_customization(&mut players, participant, &mut ghosts)
            .unwrap();
        let presentation = ghosts[&319];
        assert!(created.iter().any(|r| r.id == presentation));
        // Repeats adopt what exists instead of creating it again.
        let mut again = ghosts.clone();
        assert!(
            logic
                .intro_customization(&mut players, participant, &mut again)
                .unwrap()
                .is_empty()
        );
        assert_eq!(again, ghosts);
        let out = logic
            .produce(Which::Exit, &mut players, participant, &mut ghosts)
            .unwrap();
        assert_eq!(out.deleted, vec![presentation]);
        assert!(players.objects().get(presentation).is_none());
        // Participant zero is never a garage owner.
        assert_eq!(
            logic.intro_customization(&mut players, 0, &mut BTreeMap::new()),
            Err(replication::Error::Shape)
        );
    }

    fn presentation_exit() -> Json {
        json!({"scene_updates": [], "entities": [
            {"capture_id": 400, "asset": {"bundle": 4, "type_id": 10, "local_index": 2}, "blueprint": {"scene_content_key": 101},
             "roles": [{"kind": "root", "bus_count": 1}, {"kind": "channel", "scene_content_key": 101, "scene_serializer": 45}]},
            {"capture_id": 401, "asset": {"bundle": 4, "type_id": 10, "local_index": 2}, "blueprint": {"scene_content_key": 101},
             "roles": [{"kind": "root", "bus_count": 1}, {"kind": "channel", "scene_content_key": 101, "scene_serializer": 45}]}],
            "delete_capture_ids": [319]})
    }
    fn deletes(ids: &[u16]) -> Json {
        json!({"scene_updates": [], "entities": [], "delete_capture_ids": ids})
    }

    #[test]
    fn arrive_batches_delete_the_exit_presentation_in_two_steps() {
        let mut v = with_exit(presentation_exit());
        v["arrive"] = deletes(&[400]);
        v["arrived"] = deletes(&[401]);
        let logic = GarageLogic::from_json(&v).unwrap();
        assert!(logic.has(Which::Arrive) && logic.has(Which::Arrived));
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        for which in [Which::Spawn, Which::Entry, Which::Exit] {
            logic
                .produce(which, &mut players, participant, &mut ghosts)
                .unwrap();
        }
        let (lock, voice) = (ghosts[&400], ghosts[&401]);
        let out = logic
            .produce(Which::Arrive, &mut players, participant, &mut ghosts)
            .unwrap();
        assert_eq!((out.deleted, out.records.len()), (vec![lock], 0));
        assert!(players.objects().get(lock).is_none() && players.objects().get(voice).is_some());
        let out = logic
            .produce(Which::Arrived, &mut players, participant, &mut ghosts)
            .unwrap();
        assert_eq!(out.deleted, vec![voice]);
        assert!(players.objects().get(voice).is_none());
        // Repeats have nothing left to delete.
        assert!(
            logic
                .produce(Which::Arrived, &mut players, participant, &mut ghosts)
                .is_err()
        );
    }

    #[test]
    fn arrive_batches_see_only_what_exit_left_live() {
        let base = with_exit(presentation_exit());
        assert!(!GarageLogic::from_json(&base).unwrap().has(Which::Arrive));
        for (arrive, arrived) in [
            // Exit already deleted the customization presentation.
            (deletes(&[319]), None),
            // Never created.
            (deletes(&[999]), None),
            // Arrive deleted it first.
            (deletes(&[400]), Some(deletes(&[400]))),
            // Duplicates within one batch.
            (deletes(&[400, 400]), None),
        ] {
            let mut v = base.clone();
            v["arrive"] = arrive;
            if let Some(arrived) = arrived {
                v["arrived"] = arrived;
            }
            assert!(GarageLogic::from_json(&v).is_err(), "{v}");
        }
        // Arrive batches need an exit batch to follow.
        let mut v = content_json();
        v["arrived"] = deletes(&[277]);
        assert!(GarageLogic::from_json(&v).is_err());
        // Entities spawned by entry and kept by exit may go at arrive.
        let mut v = base;
        v["arrive"] = deletes(&[277]);
        assert!(GarageLogic::from_json(&v).is_ok());
    }

    /// The intro's shape of the lifecycle: spawn's event addresses the fade
    /// control (277), the first drive adds a HUD (330) and return-request
    /// logic (331), and the return adds the cinematic (332, spawn 295's asset).
    pub(crate) fn intro_json() -> Json {
        let mut v = presentation_json();
        v["content"]["catalogs"][0]["entries"] = json!([[10, 5]]);
        v["content"]["profiles"]
            .as_array_mut()
            .unwrap()
            .push(json!({"asset": {"bundle": 4, "type_id": 10, "local_index": 4}, "serializers": ["root", "rpc_reference"]}));
        let def = |capture: u16, serializer: u16| {
            json!({"capture_id": capture, "asset": {"bundle": 4, "type_id": 10, "local_index": 4},
                "blueprint": {"scene_content_key": 101},
                "roles": [{"kind": "root", "bus_count": 1},
                          {"kind": "channel", "scene_content_key": 101, "scene_serializer": serializer}]})
        };
        let ev = |capture: u16, event: u32| json!({"target": {"capture_id": capture}, "entity": 1, "event": event});
        let mut cinematic = v["spawn"]["entities"][3].clone();
        cinematic["capture_id"] = json!(332);
        v["spawn"]["events"] = json!([ev(277, 2)]);
        v["arrived"] =
            json!({"scene_updates": [], "entities": [def(330, 46), def(331, 47)], "events": []});
        v["free_roam"] = json!({"scene_updates": [], "entities": [],
            "events": [ev(330, 19), ev(330, 17), ev(330, 12), ev(277, 0)]});
        v["return_requested"] =
            json!({"scene_updates": [], "entities": [], "events": [ev(331, 3)]});
        v["returning"] = json!({"scene_updates": [], "entities": [cinematic],
            "delete_capture_ids": [330, 331], "events": [ev(277, 1)]});
        v["returned"] =
            json!({"scene_updates": [], "entities": [], "delete_capture_ids": [332], "events": []});
        // Transfer presentation as installed: four logics per relocation, each
        // its own asset, the last (336) carrying the two bursts; two go at
        // unblack, one at the arrival, the last with the next drive's retirement.
        v["content"]["catalogs"][0]["entries"] = json!([[10, 9]]);
        for local_index in 5..9 {
            v["content"]["profiles"]
                .as_array_mut()
                .unwrap()
                .push(json!({"asset": {"bundle": 4, "type_id": 10, "local_index": local_index}, "serializers": ["root", "rpc_reference"]}));
        }
        let transfer = |capture: u16, serializer: u16, local_index: u16| {
            json!({"capture_id": capture, "asset": {"bundle": 4, "type_id": 10, "local_index": local_index},
                "blueprint": {"scene_content_key": 101},
                "roles": [{"kind": "root", "bus_count": 2},
                          {"kind": "channel", "scene_content_key": 101, "scene_serializer": serializer}]})
        };
        let burst = |entity: u32, event: u32| json!({"target": {"capture_id": 336}, "entity": entity, "event": event});
        let transfers = || {
            json!([
                transfer(333, 48, 5),
                transfer(334, 49, 6),
                transfer(335, 50, 7),
                transfer(336, 51, 8)
            ])
        };
        // The ordinary exit/arrive cycle owns the same four roles, so every
        // relocation, crash and return cycle restores the live set.
        v["exit"] = json!({"scene_updates": [], "entities": transfers(), "events": [ev(277, 10)]});
        v["arrive"] = json!({"scene_updates": [], "entities": [],
            "delete_capture_ids": [333, 334], "events": [ev(277, 15)]});
        v["arrived"]["delete_capture_ids"] = json!([335]);
        v["returning"]["delete_capture_ids"] = json!([330, 331, 336]);
        v["in_world_teleport"] = json!({"scene_updates": [], "entities": transfers(),
            "delete_capture_ids": [336, 331, 330], "events": [ev(277, 1), ev(277, 2)]});
        v["in_world_arrival"] = json!({"scene_updates": [], "entities": [],
            "delete_capture_ids": [333, 334], "events": [ev(277, 3)]});
        v["crashed"] = json!({"scene_updates": [], "entities": [],
            "delete_capture_ids": [336, 331, 330], "events": []});
        v["recovered"] = json!({"scene_updates": [],
            "entities": [transfer(336, 51, 8), def(331, 47), def(330, 46)],
            "events": [burst(1, 17), burst(1, 15), burst(1, 13), burst(1, 11),
                       burst(2, 9), burst(2, 7), burst(2, 5), burst(2, 4)]});
        v
    }

    #[test]
    fn intro_spawn_adds_installed_career_start_logic_and_its_events() {
        let mut v = intro_json();
        let mut stats = v["recovered"]["entities"][0].clone();
        stats["capture_id"] = json!(340);
        v["intro_spawn"] = json!({"scene_updates": [], "entities": [stats],
            "events": [{"target": {"capture_id": 340}, "entity": 2, "event": 9},
                       {"target": {"capture_id": 340}, "entity": 1, "event": 17}]});
        let logic = GarageLogic::from_json(&v).unwrap();
        assert!(!logic.intro_requires_reputation());
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        let spawn = logic
            .produce_intro_spawn(&mut players, participant, &mut ghosts)
            .unwrap();
        // Keep the fade controller dormant; intro startup owns its Black.
        // Other authored intro events still fire in their original order.
        assert_eq!(ghosts.keys().copied().collect::<Vec<_>>(), [277, 340]);
        assert_eq!(
            spawn.records.iter().map(|r| r.id).collect::<Vec<_>>(),
            [ghosts[&277], ghosts[&340]]
        );
        assert_eq!(
            spawn
                .events
                .iter()
                .map(|f| (f.target.ghost, f.target.entity, f.event))
                .collect::<Vec<_>>(),
            [(ghosts[&340], 2, 9), (ghosts[&340], 1, 17)]
        );
        // A repeated spawn refuses instead of duplicating.
        assert!(
            logic
                .produce_intro_spawn(&mut players, participant, &mut ghosts)
                .is_err()
        );
        // The intro batch may not redefine a spawn event target, update or
        // delete anything, or fire scene-wide events.
        let mut bad = v.clone();
        bad["intro_spawn"]["entities"][0]["capture_id"] = json!(277);
        bad["intro_spawn"]["events"] = json!([]);
        assert!(GarageLogic::from_json(&bad).is_err());
        let mut bad = v.clone();
        bad["intro_spawn"]["delete_capture_ids"] = json!([277]);
        assert!(GarageLogic::from_json(&bad).is_err());
        let mut bad = v.clone();
        bad["intro_spawn"]["events"] =
            json!([{"target": {"scene_content_key": 101}, "entity": 1, "event": 0}]);
        assert!(GarageLogic::from_json(&bad).is_err());
        let mut bad = v;
        bad["intro_spawn"]["events"] =
            json!([{"target": {"capture_id": 277}, "entity": 1, "event": 0}]);
        assert!(GarageLogic::from_json(&bad).is_err());
    }

    #[test]
    fn intro_handoff_creates_its_logic_once_and_only_as_plain_creations() {
        let mut v = intro_json();
        let mut root = v["recovered"]["entities"][0].clone();
        root["capture_id"] = json!(341);
        v["intro_handoff"] = json!({"scene_updates": [], "entities": [root]});
        let logic = GarageLogic::from_json(&v).unwrap();
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        logic
            .produce_intro_spawn(&mut players, participant, &mut ghosts)
            .unwrap();
        let created = logic
            .produce_intro_handoff(&mut players, participant, &mut ghosts)
            .unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(ghosts.get(&341), Some(&created[0].id));
        // A repeated handoff leaves the live logic alone.
        assert!(
            logic
                .produce_intro_handoff(&mut players, participant, &mut ghosts)
                .unwrap()
                .is_empty()
        );
        for (key, value) in [
            ("delete_capture_ids", json!([341])),
            (
                "events",
                json!([{"target": {"capture_id": 341}, "entity": 1, "event": 0}]),
            ),
        ] {
            let mut bad = v.clone();
            bad["intro_handoff"][key] = value;
            assert!(GarageLogic::from_json(&bad).is_err(), "{key}");
        }
        let mut bad = v;
        bad["intro_handoff"]["entities"][0]["capture_id"] = json!(277);
        assert!(GarageLogic::from_json(&bad).is_err());
    }

    #[test]
    fn intro_batches_create_only_addressed_logic_and_skip_absent_targets() {
        let logic = GarageLogic::from_json(&intro_json()).unwrap();
        let (mut players, participant) = world();
        let mut ghosts = BTreeMap::new();
        let objects = players.objects().snapshot().len();
        let spawn = logic
            .produce_intro_spawn(&mut players, participant, &mut ghosts)
            .unwrap();
        // Only the fade control its event addresses: no garage logic.
        assert_eq!(ghosts.keys().copied().collect::<Vec<_>>(), [277]);
        assert_eq!(spawn.records.len(), 1);
        assert_eq!(players.objects().snapshot().len(), objects + 1);
        assert!(spawn.events.is_empty());
        // Ordinary spawn retains its Black; only intro membership defers it.
        assert_eq!(
            logic
                .present_events(Which::Spawn, &players, &ghosts)
                .unwrap()
                .iter()
                .map(|f| (f.target.ghost, f.event))
                .collect::<Vec<_>>(),
            [(ghosts[&277], 2)]
        );
        assert_eq!(
            logic.produce_intro_spawn(&mut players, participant, &mut ghosts),
            Err(replication::Error::DuplicateObject)
        );
        assert_eq!(logic.fade_capture(), Some(277));
        let fade = ghosts[&277];
        let arrived = logic
            .produce_present(
                Which::Arrived,
                &mut players,
                participant,
                &mut ghosts,
                Part::Creations,
            )
            .unwrap();
        assert_eq!(arrived.records.len(), 2);
        assert!(arrived.deleted.is_empty() && arrived.events.is_empty());
        let (hud, request) = (ghosts[&330], ghosts[&331]);
        let roam = logic
            .present_events(Which::FreeRoam, &players, &ghosts)
            .unwrap();
        assert_eq!(
            roam.iter()
                .map(|f| (f.target.ghost, f.event))
                .collect::<Vec<_>>(),
            [(hud, 19), (hud, 17), (hud, 12), (fade, 0)]
        );
        let requested = logic
            .produce_present(
                Which::ReturnRequested,
                &mut players,
                participant,
                &mut ghosts,
                Part::All,
            )
            .unwrap();
        assert_eq!(
            requested
                .events
                .iter()
                .map(|f| (f.target.ghost, f.event))
                .collect::<Vec<_>>(),
            [(request, 3)]
        );
        let before = players.objects().snapshot().len();
        let returning = logic
            .produce_present(
                Which::Returning,
                &mut players,
                participant,
                &mut ghosts,
                Part::All,
            )
            .unwrap();
        let mut deleted = returning.deleted.clone();
        deleted.sort();
        assert_eq!(deleted, [hud, request]);
        // The cinematic is created with the return.
        assert_eq!(returning.records.len(), 1);
        let cinematic = ghosts[&332];
        assert_eq!(players.objects().snapshot().len(), before - 1);
        assert_eq!(
            returning
                .events
                .iter()
                .map(|f| (f.target.ghost, f.event))
                .collect::<Vec<_>>(),
            [(fade, 1)]
        );
        let returned = logic
            .produce_present(
                Which::Returned,
                &mut players,
                participant,
                &mut ghosts,
                Part::Deletions,
            )
            .unwrap();
        assert_eq!(returned.deleted, [cinematic]);
        assert!(returned.records.is_empty() && !ghosts.contains_key(&332));
        assert!(players.objects().get(cinematic).is_none());
        // Without the logic nothing is fired and nothing fails.
        let empty = BTreeMap::new();
        assert!(
            logic
                .present_events(Which::FreeRoam, &players, &empty)
                .unwrap()
                .is_empty()
        );
        assert!(logic.fade(&players, &empty, &[2]).unwrap().is_empty());
        assert_eq!(
            logic
                .fade(&players, &ghosts, &[2, 3])
                .unwrap()
                .iter()
                .map(|f| (f.target, f.event))
                .collect::<Vec<_>>(),
            [
                (
                    EntityRef {
                        ghost: fade,
                        entity: 1
                    },
                    2
                ),
                (
                    EntityRef {
                        ghost: fade,
                        entity: 1
                    },
                    3
                )
            ]
        );
        // Content without the free-roam ViewWorld has no fade logic.
        let plain = GarageLogic::from_json(&presentation_json()).unwrap();
        assert_eq!(plain.fade_capture(), None);
    }
}

#[cfg(test)]
mod value_tests;
