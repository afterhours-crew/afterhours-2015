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
    Bool(Option<bool>),
    Int(Option<i32>),
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
            Self::Variant { .. } | Self::ReputationLevelStat => Kind::RpcVariant,
            Self::Bool(_) => Kind::BoolProperty,
            Self::Int(_) | Self::Reputation(_) => Kind::I32Property,
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Batch {
    pub scene_updates: Vec<SceneUpdate>,
    pub entities: Vec<Definition>,
    /// Boolean property changes on live entities: (capture, field, value).
    pub entity_updates: Vec<(u16, usize, bool)>,
    pub delete: Vec<u16>,
    pub events: Vec<Event>,
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
}

#[derive(Clone, Debug)]
pub struct GarageLogic {
    content: Content,
    reputation: Option<reputation::Thresholds>,
    spawn: Batch,
    vehicles: Batch,
    entry: Batch,
    exit: Batch,
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

impl GarageLogic {
    pub fn load(path: &Path) -> Result<Self, Failure> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| Failure::Output)?
            .take(262_145)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Output)?;
        if bytes.len() > 262_144 {
            return Err(Failure::BodyLimit);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| Failure::ProfileConfig)?)
    }
    pub fn from_json(v: &Json) -> Result<Self, Failure> {
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
            .transpose()?;
        if entry
            .entities
            .iter()
            .chain(&exit.entities)
            .flat_map(|d| &d.roles)
            .any(|r| matches!(r, Role::Reputation(_) | Role::ReputationLevelStat))
        {
            return Err(Failure::ProfileConfig);
        }
        for definition in spawn.entities.iter().chain(&entry.entities) {
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
            vehicles,
            entry,
            exit,
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
        }
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
        self.produce_with_reputation(which, players, participant, ghosts, None)
    }
    pub fn produce_with_reputation(
        &self,
        which: Which,
        players: &mut Players,
        participant: u16,
        ghosts: &mut BTreeMap<u16, u16>,
        scores: Option<ReputationScores>,
    ) -> Result<Output, replication::Error> {
        if participant == 0 {
            return Err(replication::Error::Shape);
        }
        let batch = self.batch(which);
        let values = match (scores, &self.reputation) {
            (Some(scores), Some(thresholds)) => Some(thresholds.evaluate(scores)),
            _ => None,
        };
        if values.is_none()
            && batch
                .entities
                .iter()
                .flat_map(|d| &d.roles)
                .any(|r| matches!(r, Role::Reputation(_) | Role::ReputationLevelStat))
        {
            return Err(replication::Error::Shape);
        }
        let mut output = Output::default();
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
        if !batch.delete.is_empty() {
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
        }
        for event in &batch.events {
            let ghost = match event.target {
                EventTarget::Capture(id) => {
                    *ghosts.get(&id).ok_or(replication::Error::UnknownObject)?
                }
                EventTarget::Scene(key) => players
                    .objects()
                    .scene(key)
                    .ok_or(replication::Error::UnknownObject)?,
            };
            let fire = Fire {
                event: event.event,
                player: None,
                target: EntityRef {
                    ghost,
                    entity: event.entity,
                },
            };
            fire.encode().map_err(|_| replication::Error::Shape)?;
            output.events.push(fire);
        }
        Ok(output)
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
                Role::Bool(value) => (Initial::BoolProperty(value), Update::BoolProperty(None)),
                Role::Int(value) => (Initial::I32Property(value), Update::I32Property(None)),
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
}
