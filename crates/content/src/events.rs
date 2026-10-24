// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed composition of one event: its start area in a scene, the start area's
//! game-mode launcher, the launched game mode's serializers and channel bindings, the
//! participant logics spawned by the start area or the game mode, and the game mode's
//! sequences. Every channel is bound through the spawner that creates the entity, as
//! the client binds it; a channel asset matching elsewhere is never used.
//! Lifecycle, values, timing and live object identities belong to the caller.
use crate::{
    Error,
    assets::AssetIndex,
    graph::{
        scenes::{SceneLookup, embedded_bus, scene_roots},
        *,
    },
    participant_logic::{Rules, shape},
    scan::Assets,
    scoring::participant_properties::{Property, mirrored},
    world::StartupKind,
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::replication::entity::{Kind, Profile, creation::Asset};
use std::collections::{BTreeMap, BTreeSet};

pub mod ai;
pub mod awards;
pub mod discovery;
mod invitations;
pub mod lifecycle;
mod pacing;
mod recovery;
mod request;
pub mod restart;
mod rewards;
mod route;
mod sequence;
mod spatial;
mod standings;
mod values;
use values::{Evaluator, Number, name};

pub const MAX_CHECKPOINTS: usize = 255;
const LAUNCHER: &str = "SimpleGameModeLauncherEntityData";
const DETAILS: &str = "EventDetailsEntityData";
const DETAILS_CHANNEL: &str = "EventDetailsChannelEntityData";
const WAYPOINTS_CHANNEL: &str = "WaypointListChannelEntityData";
const CHECKPOINT: &str = "VolumeWaypointEntityData";
const START_VOLUME: &str = "VolumeParticipantSetFilterEntityData";
const NOTIFICATION: &str = "SetClientNotificationEntityData";
const BOOL_NOTIFICATION: &str = "BoolPropertyClientNotificationEntityData";
const DNF_TRANSITION: &str = "DNFGameModeStateTransitionEntityData";
const GOTO_COMMAND: &str = "GotoWaypointCommandEntityData";
const GOTO_PRECISION: &str = "GotoWaypointCommandPrecisionEntityData";
const LOCATOR: &str = "LocatorEntityData";
const SPEED_LIMIT: &str = "SpeedLimitCommandEntityData";
/// Link field of a start-area prefab instance naming its external participant set
/// (E996: start areas of one event's variants link the same set; name unrecovered).
const EXTERNAL_SET: u32 = 2_017_017_944;
const MANAGER_CLASSES: [&str; 3] = [
    "GameTimeEntityData",
    "GameModeManagerEntityData",
    "ParticipantManagerEntityData",
];

/// Where a channel reference points: a scene serializer or a serializer of the
/// event's game mode.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Binding {
    Scene { content_key: u32, serializer: usize },
    GameMode { serializer: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub kind: Kind,
    pub class: String,
    /// The composition asset that declares the serializer, and its object index there.
    pub asset: String,
    pub object: usize,
    pub binding: Option<Binding>,
    /// The integer participant property a synchronized integer mirrors (its
    /// input is an accessor's `OutputValue`; #138).
    pub mirrors: Option<Property>,
}

/// A blueprint instance with its creation asset and serializer layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    pub blueprint: String,
    pub asset: Asset,
    pub catalog: Vec<(u32, u32)>,
    pub bus_count: u16,
    /// One-based embedded bus of the spawner in its owner (the creation sub-ID).
    pub sub_id: u32,
    /// Which spawner of this blueprint, in its owner's composition order.
    pub occurrence: usize,
    pub fields: Vec<Field>,
}
impl Layout {
    pub fn profile(&self) -> Result<Profile, Error> {
        Profile::new(&self.fields.iter().map(|f| f.kind).collect::<Vec<_>>())
            .map_err(|_| bad("event serializer profile"))
    }
    /// The serializer index of the first field of `class`, or of the only one.
    pub fn unique(&self, class: &str) -> Result<usize, Error> {
        let mut found = self
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.class == class);
        let (index, _) = found
            .next()
            .ok_or_else(|| bad(&format!("event layout has no {class}")))?;
        if found.next().is_some() {
            return Err(bad(&format!("event layout has several {class}")));
        }
        Ok(index)
    }
}

/// The game mode: its layout (owned by the Gameplay game-mode manager), the root's
/// three Gameplay endpoints (game time, game-mode manager, participant manager), and
/// the start area's checkpoint route.
#[derive(Clone, Debug, PartialEq)]
pub struct GameMode {
    pub lifecycle: lifecycle::Lifecycle,
    pub recovery: Option<nfs_world::crash::route::Config>,
    pub layout: Layout,
    pub root_bindings: [(u32, usize); 3],
    pub checkpoints: Vec<(u32, usize)>,
    /// AI destination locators in checkpoint order, including an explicitly
    /// linked continuation past the finish. These differ from volume centers.
    pub points: Vec<[f32; 3]>,
    /// Ordinary waypoint locators used to measure progress, without continuation.
    pub checkpoint_points: Vec<[f32; 3]>,
    /// Authored waypoint-channel driving command; None when this mode has none.
    pub drift: Option<bool>,
    pub pacing: Option<nfs_world::race_pacing::Config>,
    /// Authored repeating cache-load delay for the ongoing race position.
    /// Score- and time-based modes without that collector have no such timer.
    pub standings_interval: Option<f32>,
    pub laps: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    pub layout: Layout,
    /// The sequence manager endpoint in the manager scene.
    pub manager: (u32, usize),
    /// The game-mode state that spawns it, when it has channels to bind.
    pub spawner: Option<usize>,
    /// The AI goal of a go-to-waypoint sequence (a race intro).
    pub goto: Option<Goto>,
    /// Authored post-race AI commands and the sequence's server timeout.
    pub wander: Option<Wander>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wander {
    pub urgent: bool,
    pub prefer_straight: bool,
    pub speed_limit: f32,
    pub timeout: f32,
}

/// A go-to-waypoint sequence's AI goal (E997): the location and the speed-limit
/// command linked from the scene channel bound to the sequence, and the reach
/// radius of the sequence's own precision setting (official intro goal: the
/// `Sprint_Race_007` locator, radius 150 and limit 22.35).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Goto {
    pub point: [f32; 3],
    pub radius: f32,
    pub speed_limit: f32,
}

/// The first-place rewards the start area presents (its `CashReward` and
/// `RepReward` inputs), evaluated from installed defaults at creation. Server
/// time-limited bonuses are separate outputs and are not included.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rewards {
    pub cash: i32,
    pub rep: i32,
}

/// One event's composition. Logic and sequence names are the caller's selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Snapshot-independent threshold and weight values from this event's
    /// bound award channels, resolved through its own spawner.
    pub award_values: BTreeMap<Property, nfs_world::scoring::value::Value>,
    pub ai: ai::Roster,
    pub invitations: Invitations,
    pub restart: Option<Vec<restart::Destination>>,
    pub scene: String,
    pub scene_content_key: u32,
    pub game_mode_id: String,
    pub launcher: usize,
    pub start_volume: usize,
    pub rewards: Rewards,
    /// First, second, third and other payouts. The last finisher also selects
    /// "other", using the race's maximum participant count.
    pub placement_rewards: Option<[Rewards; 4]>,
    pub game_mode: GameMode,
    pub start_logics: Vec<Layout>,
    pub game_mode_logics: Vec<Layout>,
    pub sequences: Vec<Sequence>,
    /// Logics spawned in other startup scenes, in request order.
    pub scene_logics: Vec<SceneLogic>,
    /// The linked logic's layouts in the other start areas that share this area's
    /// external participant set, in scene order.
    pub linked_logics: Vec<Layout>,
    /// Event-scene serializers of the requested start-area notifications.
    pub area_notifications: Vec<usize>,
    /// Event-scene serializers of the requested scene notifications.
    pub scene_notifications: Vec<usize>,
    /// (scene content key, serializer) of the requested in-event flags.
    pub game_mode_notifications: Vec<(u32, usize)>,
    /// The game mode's DNF transition time-out, in seconds.
    pub dnf_timeout: f32,
}

/// The Events scene's invitation-availability output. Its threshold is authored
/// against the human participant set after the Prestige-mode exclusion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Invitations {
    pub scene_content_key: u32,
    pub serializer: usize,
    /// Availability is true only when the eligible count exceeds this value.
    pub count_above: usize,
}

/// A logic spawned by a spawner in another startup scene, which owns it and whose
/// bundle registers its blueprint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneLogic {
    pub scene_content_key: u32,
    pub layout: Layout,
}

/// What to resolve. `bundle` is the startup sublevel whose registry holds the
/// event blueprints; `scene` is the sublevel that places the start area.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub bundle: String,
    pub scene: String,
    pub game_mode_id: String,
    /// The game-mode blueprint the start area's launcher must name.
    pub game_mode: String,
    pub manager_scene: String,
    pub start_logics: Vec<String>,
    pub game_mode_logics: Vec<String>,
    pub sequences: Vec<String>,
    /// Sequences from another loaded scene bundle, as (bundle, blueprint).
    pub external_sequences: Vec<(String, String)>,
    /// Logics spawned in other startup scenes, as (scene, blueprint).
    pub scene_logics: Vec<(String, String)>,
    /// A start-area logic also resolved in the start areas linked to this one.
    pub linked_logic: Option<String>,
    /// (asset, object) of client notifications inside the start area.
    pub area_notifications: Vec<(String, usize)>,
    /// (asset, object) of client notifications unique in the event scene.
    pub scene_notifications: Vec<(String, usize)>,
    /// (scene, asset, object) of Boolean client notifications in other startup
    /// scenes that the host sets while the participant is in the game mode.
    pub game_mode_notifications: Vec<(String, String, usize)>,
}

/// The djb2 hash with XOR mixing of the lower-case asset name: the race-event ID
/// of the account's statistics records (E995: `gameplay/gamemodes/sprint_race_007`
/// gives 1761118161).
pub fn record_id(game_mode_id: &str) -> u32 {
    game_mode_id
        .to_lowercase()
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b))
}

fn within(site: &Site, instance: &Key) -> bool {
    site.parents.iter().any(|f| f.reference == *instance)
}
/// The unique node of `class` declared directly in the same prefab as `member`.
fn sibling<'a>(nodes: &'a [Node], class: &str, member: &Site) -> Result<&'a Node, Error> {
    let mut found = nodes.iter().filter(|n| {
        n.object.class == class && n.site.bus == member.bus && n.site.parents == member.parents
    });
    let node = found
        .next()
        .ok_or_else(|| bad(&format!("start area has no {class}")))?;
    if found.next().is_some() {
        return Err(bad(&format!("start area has several {class}")));
    }
    Ok(node)
}
fn unique_in<'a>(nodes: &'a [Node], class: &str, instance: &Key) -> Result<&'a Node, Error> {
    let mut found = nodes
        .iter()
        .filter(|n| n.object.class == class && within(&n.site, instance));
    let node = found
        .next()
        .ok_or_else(|| bad(&format!("start area has no {class}")))?;
    if found.next().is_some() {
        return Err(bad(&format!("start area has several {class}")));
    }
    Ok(node)
}
fn one_port<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<u32, Error> {
    match graph.link_ports(site)?.as_slice() {
        [port] => Ok(*port),
        _ => Err(bad("expected one outgoing link port")),
    }
}
fn finite(value: Option<&Value>, what: &str) -> Result<f32, Error> {
    match value {
        Some(Value::Float32(v)) if v.is_finite() => Ok(*v),
        _ => Err(bad(what)),
    }
}
/// A locator's position: its transform's translation.
fn location(object: &Object) -> Result<[f32; 3], Error> {
    let trans = object
        .fields
        .get("Transform")
        .and_then(Value::as_fields)
        .and_then(|t| t.get("trans"))
        .and_then(Value::as_fields)
        .ok_or_else(|| bad("locator transform"))?;
    Ok([
        finite(trans.get("x"), "locator position")?,
        finite(trans.get("y"), "locator position")?,
        finite(trans.get("z"), "locator position")?,
    ])
}
fn class_kind(rules: &Rules, class: &str) -> Result<Option<Kind>, Error> {
    rules
        .classes
        .get(class)
        .copied()
        .ok_or_else(|| bad(&format!("unsupported event class {class}")))
}
/// The reference class a channel class binds, or the channel class a reference names.
fn channel_class(reference: &str) -> Option<&'static str> {
    Some(match reference {
        "LinkedSchematicChannelReferenceEntityData" => "LinkedSchematicChannelEntityData",
        "WaypointListChannelReferenceEntityData" => WAYPOINTS_CHANNEL,
        "StateSequenceChannelReferenceEntityData" => "StateSequenceChannelEntityData",
        "EventDetailsChannelReferenceEntityData" => DETAILS_CHANNEL,
        "NotificationCollectorChannelReferenceEntityData" => {
            "NotificationCollectorChannelEntityData"
        }
        _ => return None,
    })
}

/// A serializing node's composition site, class and serializer kind.
type FieldSite = (Site, String, Kind);
/// A blueprint registered outside the event bundle: bundle, catalog and reference.
type Registry<'a> = (u16, &'a [(u32, u32)], crate::assets::AssetReference);

/// Fields of a blueprint composition under the class rules, with the composition
/// sites of serializing nodes and the bus count.
fn fields(nodes: &[Node], rules: &Rules, root_class: &str) -> Result<(u16, Vec<FieldSite>), Error> {
    let mut buses = 0u16;
    let mut out = Vec::new();
    for node in nodes {
        if node.object.class.ends_with("Blueprint") {
            let flags = match node.object.fields.get("Flags") {
                Some(Value::UInt16(v)) => u32::from(*v),
                Some(v) => bits32(v)?,
                None => return Err(bad("event bus flags")),
            };
            if flags & 1 != 0 {
                buses = buses.checked_add(1).ok_or_else(|| bad("event bus count"))?;
            }
            continue;
        }
        if let Some(kind) = class_kind(rules, &node.object.class)? {
            out.push((node.site.clone(), node.object.class.clone(), kind));
        }
    }
    if buses == 0
        || out.first().is_none_or(|(_, class, _)| class != root_class)
        || out.len() > nfs_world::replication::entity::MAX_SERIALIZERS
    {
        return Err(bad("event root or bus count"));
    }
    Ok((buses, out))
}

/// Bind the channel references linked from `root` to the channels linked from
/// `spawner`, in declared order: a linked-schematic reference takes the next
/// unused target with its channel asset; a typed reference takes the next unused
/// target of its channel class.
fn bind<S: Source>(
    graph: &mut Graph<S>,
    references: &[(Site, String)],
    root: &Site,
    spawner: &Site,
    rules: &Rules,
) -> Result<Vec<Site>, Error> {
    let linked: BTreeSet<Site> = graph
        .links(root, rules.root_channels)?
        .into_iter()
        .filter(|e| e.port == 0)
        .map(|e| e.site)
        .collect();
    let targets = graph.links(spawner, rules.spawner_channels)?;
    let mut used = BTreeSet::new();
    let mut out = Vec::new();
    for (site, class) in references {
        if !linked.contains(site) {
            return Err(bad("channel reference is not linked from the root"));
        }
        let wanted = channel_class(class).ok_or_else(|| bad("unsupported channel reference"))?;
        let asset = if class == "LinkedSchematicChannelReferenceEntityData" {
            Some(graph.target_field(site, "ChannelAsset")?)
        } else {
            None
        };
        let mut chosen = None;
        for (index, target) in targets.iter().enumerate() {
            if used.contains(&index) || target.port != 0 {
                continue;
            }
            if graph.object(&target.site)?.class != wanted {
                continue;
            }
            if let Some(asset) = &asset
                && graph.target_field(&target.site, "ChannelAsset")? != *asset
            {
                continue;
            }
            chosen = Some(index);
            break;
        }
        let index = chosen.ok_or_else(|| bad(&format!("unbound channel reference {class}")))?;
        used.insert(index);
        out.push(targets[index].site.clone());
    }
    Ok(out)
}

impl AssetIndex {
    /// Recover serializer occurrence identities using the same composition
    /// traversal and rules that produced this layout. Repeated prefab assets
    /// retain separate identities; matching only asset/object would be ambiguous.
    pub fn event_serializer_identities(
        &mut self,
        layout: &Layout,
        root_class: &str,
    ) -> Result<Vec<crate::scoring::events::Identity>, Error> {
        let rules = Rules::from_json(&self.identity.participant_logic.to_json())?;
        let nodes = Graph::new(&mut self.assets).read(Site::root(&layout.blueprint))?;
        let (buses, actual) = fields(&nodes, &rules, root_class)?;
        if buses != layout.bus_count || actual.len() != layout.fields.len() {
            return Err(bad("event serializer identity layout"));
        }
        actual
            .into_iter()
            .zip(&layout.fields)
            .map(|((site, class, kind), field)| {
                if class != field.class
                    || kind != field.kind
                    || site.key != (field.asset.clone(), field.object)
                {
                    return Err(bad("event serializer identity mismatch"));
                }
                Ok(crate::scoring::events::Identity::from(&site))
            })
            .collect()
    }

    /// Resolve one event's composition. Ambiguous or missing spawners, channels,
    /// start areas or checkpoints are errors.
    pub fn event(&mut self, request: &Request) -> Result<Event, Error> {
        let rival_links = self.identity.rival_links;
        let ai_links = self.identity.ai_links;
        let rules = Rules::from_json(&self.identity.participant_logic.to_json())?;
        let self_sequences_rules = self.identity.sequences.to_json();
        let scene_rules = self.identity.scenes.clone();
        let startup = self.startup_entries(&scene_rules.level)?;
        let bundle_scene = request.bundle.to_lowercase();
        let scene = request.scene.to_lowercase();
        let game_mode_id = request.game_mode_id.to_lowercase();
        let bundle = startup
            .iter()
            .position(|e| {
                e.kind == StartupKind::SubLevel && e.name.eq_ignore_ascii_case(&bundle_scene)
            })
            .and_then(|i| u16::try_from(i + 1).ok())
            .ok_or_else(|| bad("event bundle is not loaded"))?;
        let catalog = self.catalog(&bundle_scene)?;
        // Registry references before the graph borrows the assets.
        let mut references = BTreeMap::new();
        for name in std::iter::once(&request.game_mode)
            .chain(&request.start_logics)
            .chain(&request.linked_logic)
            .chain(&request.game_mode_logics)
            .chain(&request.sequences)
        {
            let name = name.to_lowercase();
            let r = self.reference(&bundle_scene, &name)?;
            references.insert(name, r);
        }
        let mut scene_registries = Vec::new();
        let mut sequence_registries = BTreeMap::new();
        for (scene, blueprint) in &request.external_sequences {
            let scene = scene.to_lowercase();
            let name = blueprint.to_lowercase();
            let bundle = startup
                .iter()
                .position(|e| {
                    e.kind == StartupKind::SubLevel && e.name.eq_ignore_ascii_case(&scene)
                })
                .and_then(|i| u16::try_from(i + 1).ok())
                .ok_or_else(|| bad("external sequence bundle is not loaded"))?;
            if sequence_registries
                .insert(
                    name.clone(),
                    (
                        bundle,
                        self.catalog(&scene)?,
                        self.reference(&scene, &name)?,
                    ),
                )
                .is_some()
            {
                return Err(bad("duplicate external sequence"));
            }
        }
        for (scene_name, blueprint) in &request.scene_logics {
            let scene_name = scene_name.to_lowercase();
            let blueprint = blueprint.to_lowercase();
            let scene_bundle = startup
                .iter()
                .position(|e| {
                    e.kind == StartupKind::SubLevel && e.name.eq_ignore_ascii_case(&scene_name)
                })
                .and_then(|i| u16::try_from(i + 1).ok())
                .ok_or_else(|| bad("scene-logic bundle is not loaded"))?;
            let scene_catalog = self.catalog(&scene_name)?;
            let r = self.reference(&scene_name, &blueprint)?;
            scene_registries.push((scene_name, blueprint, scene_bundle, scene_catalog, r));
        }
        let mut graph = Graph::new(&mut self.assets);
        let mut scenes = SceneLookup {
            roots: scene_roots(&mut graph, &scene_rules.level, &startup)?,
            rules: &scene_rules,
            endpoints: BTreeMap::new(),
            loaded: BTreeSet::new(),
        };
        let scene_root = scenes
            .roots
            .get(&scene)
            .ok_or_else(|| bad("event scene is not a startup scene"))?
            .clone();
        let scene_nodes = graph.read(scene_root)?;
        let (launcher, instance) = request::start_area(&mut graph, &scene_nodes, &game_mode_id)?;
        let scene_content_key = crate::assets::scene_key(&scene);
        let rewards = {
            let frame = launcher
                .parents
                .last()
                .ok_or_else(|| bad("start area frame"))?
                .clone();
            let mut area = launcher.clone();
            area.parents.pop();
            area.key = frame.reference;
            area.bus = frame.bus;
            let mut values = Evaluator::new(&mut graph);
            let mut reward = |field: &str| -> Result<i32, Error> {
                match values.input(&area, name(field))? {
                    Some(Number::Integer(v)) if v >= 0 => Ok(v),
                    _ => Err(bad(&format!(
                        "start area {field} is not a non-negative integer"
                    ))),
                }
            };
            Rewards {
                cash: reward("CashReward")?,
                rep: reward("RepReward")?,
            }
        };
        let endpoint = |graph: &mut Graph<Assets>, scenes: &mut SceneLookup<'_>, site: &Site| {
            scenes.endpoint(graph, site)
        };
        let (key, launcher_serializer) = endpoint(&mut graph, &mut scenes, &launcher)?;
        let volume = unique_in(&scene_nodes, START_VOLUME, &instance)?
            .site
            .clone();
        let (volume_key, start_volume) = endpoint(&mut graph, &mut scenes, &volume)?;
        if key != scene_content_key || volume_key != key {
            return Err(bad("start area outside the event scene"));
        }
        let instance_site = {
            let mut found = scene_nodes.iter().filter(|n| {
                n.site.key == instance && n.object.class.ends_with("ReferenceObjectData")
            });
            let node = found.next().ok_or_else(|| bad("start area instance"))?;
            if found.next().is_some() {
                return Err(bad("repeated start area instance"));
            }
            node.site.clone()
        };
        // Client notifications by their declaring asset and object.
        let notification = |graph: &mut Graph<Assets>,
                            scenes: &mut SceneLookup<'_>,
                            (asset, object): &(String, usize),
                            inside: bool|
         -> Result<usize, Error> {
            let mut found = scene_nodes.iter().filter(|n| {
                n.site.key.0.eq_ignore_ascii_case(asset)
                    && n.site.key.1 == *object
                    && (!inside || within(&n.site, &instance))
            });
            let node = found
                .next()
                .ok_or_else(|| bad("missing client notification"))?;
            if found.next().is_some() {
                return Err(bad("ambiguous client notification"));
            }
            if node.object.class != NOTIFICATION {
                return Err(bad("client notification class"));
            }
            let (key, serializer) = scenes.endpoint(graph, &node.site)?;
            if key != scene_content_key {
                return Err(bad("client notification outside the event scene"));
            }
            Ok(serializer)
        };
        let mut area_notifications = Vec::new();
        for n in &request.area_notifications {
            area_notifications.push(notification(&mut graph, &mut scenes, n, true)?);
        }
        let mut scene_notifications = Vec::new();
        for n in &request.scene_notifications {
            scene_notifications.push(notification(&mut graph, &mut scenes, n, false)?);
        }
        let mut game_mode_notifications = Vec::new();
        for (scene_name, asset, object) in &request.game_mode_notifications {
            let scene_name = scene_name.to_lowercase();
            let root = scenes
                .roots
                .get(&scene_name)
                .ok_or_else(|| bad("in-event flag scene is not a startup scene"))?
                .clone();
            let nodes = graph.read(root)?;
            let mut found = nodes
                .iter()
                .filter(|n| n.site.key.0.eq_ignore_ascii_case(asset) && n.site.key.1 == *object);
            let node = found.next().ok_or_else(|| bad("missing in-event flag"))?;
            if found.next().is_some() {
                return Err(bad("ambiguous in-event flag"));
            }
            if node.object.class != BOOL_NOTIFICATION {
                return Err(bad("in-event flag class"));
            }
            let site = node.site.clone();
            let (key, serializer) = scenes.endpoint(&mut graph, &site)?;
            if key != crate::assets::scene_key(&scene_name) {
                return Err(bad("in-event flag outside its scene"));
            }
            game_mode_notifications.push((key, serializer));
        }
        // Checkpoint route: the waypoint-list channel's targets in link order.
        let waypoints = sibling(&scene_nodes, WAYPOINTS_CHANNEL, &launcher)?;
        let laps = route::laps(&mut graph, &waypoints.site)?;
        let waypoint_site = waypoints.site.clone();
        let port = one_port(&mut graph, &waypoint_site)?;
        let mut checkpoints = Vec::new();
        let mut points = Vec::new();
        let mut checkpoint_points = Vec::new();
        for target in graph.links(&waypoint_site, port)? {
            if target.port != 0 || graph.object(&target.site)?.class != CHECKPOINT {
                return Err(bad("checkpoint route endpoint"));
            }
            checkpoints.push(endpoint(&mut graph, &mut scenes, &target.site)?);
            points.push(route::point(&mut graph, &target.site)?);
            checkpoint_points.push(route::checkpoint(&mut graph, &target.site)?);
        }
        if checkpoints.is_empty()
            || checkpoints.len() > MAX_CHECKPOINTS
            || checkpoints.iter().collect::<BTreeSet<_>>().len() != checkpoints.len()
        {
            return Err(bad("checkpoint route bound"));
        }
        // The game mode.
        let gm_name = request.game_mode.to_lowercase();
        if graph.target_field(&launcher, "GameMode")? != (gm_name.clone(), 0) {
            return Err(bad("start area launches another game mode"));
        }
        let gm_nodes = graph.read(Site::root(&gm_name))?;
        let (gm_buses, gm_fields) = fields(&gm_nodes, &rules, "GameModeEntityData")?;
        let gm_root = gm_fields[0].0.clone();
        let dnf_timeout = {
            let mut found = gm_fields
                .iter()
                .filter(|(_, class, _)| class == DNF_TRANSITION);
            let (site, _, _) = found
                .next()
                .ok_or_else(|| bad("game mode has no DNF transition"))?;
            if found.next().is_some() {
                return Err(bad("game mode has several DNF transitions"));
            }
            let site = site.clone();
            match Evaluator::new(&mut graph).field_or_input(
                &site,
                name("TimeOutDuration"),
                "TimeOutDuration",
            )? {
                Number::Float(v) if v.is_finite() && v > 0. && v <= 3600. => v,
                _ => return Err(bad("DNF time-out")),
            }
        };
        let channels: Vec<(Site, String)> = gm_fields
            .iter()
            .filter(|(_, class, kind)| {
                *kind == Kind::RpcReference && channel_class(class).is_some()
            })
            .map(|(site, class, _)| (site.clone(), class.clone()))
            .collect();
        let bound = bind(&mut graph, &channels, &gm_root, &launcher, &rules)?;
        let mut gm_binding = BTreeMap::new();
        for ((reference, _), target) in channels.iter().zip(bound) {
            let (content_key, serializer) = endpoint(&mut graph, &mut scenes, &target)?;
            gm_binding.insert(
                reference.clone(),
                (
                    target,
                    Binding::Scene {
                        content_key,
                        serializer,
                    },
                ),
            );
        }
        let placement_rewards = (gm_name == "gameplay/gamemodes/racegamemode")
            .then(|| rewards::extract(&mut graph, &gm_binding, rewards))
            .transpose()?;
        let manager_root = scenes
            .roots
            .get(&request.manager_scene.to_lowercase())
            .ok_or_else(|| bad("event manager scene missing"))?
            .clone();
        let manager_nodes = graph.read(manager_root)?;
        let mut root_bindings = [(0, 0); 3];
        let mut game_mode_bus = None;
        for (slot, class) in MANAGER_CLASSES.iter().enumerate() {
            let mut found = manager_nodes.iter().filter(|n| n.object.class == *class);
            let node = found
                .next()
                .ok_or_else(|| bad(&format!("manager scene has no {class}")))?;
            if found.next().is_some() {
                return Err(bad(&format!("manager scene has several {class}")));
            }
            root_bindings[slot] = endpoint(&mut graph, &mut scenes, &node.site)?;
            if slot == 1 {
                game_mode_bus = Some(embedded_bus(&manager_nodes, &node.site)?);
            }
        }
        let gm_fields_out = gm_fields
            .iter()
            .map(|(site, class, kind)| {
                Ok(Field {
                    kind: *kind,
                    class: class.clone(),
                    asset: site.key.0.clone(),
                    object: site.key.1,
                    binding: gm_binding.get(site).map(|(_, b)| *b),
                    mirrors: mirrored(&mut graph, site)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let gm_index: BTreeMap<Site, usize> = gm_fields
            .iter()
            .enumerate()
            .map(|(i, (site, _, _))| (site.clone(), i))
            .collect();
        let gm_reference = references
            .get(&gm_name)
            .ok_or_else(|| bad("game mode not registered"))?;
        let (gm_class, gm_local) = (gm_reference.class_id, gm_reference.local_index);
        let mut lifecycle = lifecycle::extract(&mut graph, &gm_fields)?;
        lifecycle.bindings.channels = lifecycle::channels::read(&mut graph, &gm_binding)?;
        lifecycle::resolve_configurations(&mut lifecycle, &mut graph, &gm_fields, &gm_binding)?;
        lifecycle::resolve_imports(
            &mut lifecycle,
            &mut graph,
            &gm_fields,
            &gm_binding,
            |graph, site| endpoint(graph, &mut scenes, site),
        )?;
        let game_mode = GameMode {
            lifecycle,
            recovery: recovery::extract(&gm_nodes)?,
            layout: Layout {
                blueprint: gm_name.clone(),
                asset: Asset {
                    bundle,
                    type_id: u16::try_from(gm_class).map_err(|_| bad("game mode class ID"))?,
                    local_index: u16::try_from(gm_local).map_err(|_| bad("game mode ordinal"))?,
                },
                catalog: catalog.clone(),
                bus_count: gm_buses,
                sub_id: game_mode_bus.ok_or_else(|| bad("game mode manager bus"))?,
                occurrence: 0,
                fields: gm_fields_out,
            },
            root_bindings,
            checkpoints,
            points,
            checkpoint_points,
            drift: route::drift(&gm_nodes)?,
            pacing: pacing::extract(&mut graph, &gm_nodes, &gm_binding)?,
            standings_interval: standings::interval(&mut graph, &gm_nodes)?,
            laps,
        };
        game_mode.layout.profile()?;
        // Resolve a spawned participant logic's channels against its spawner.
        let spawned = |graph: &mut Graph<Assets>,
                       scenes: &mut SceneLookup<'_>,
                       name: &str,
                       spawner: &Site,
                       occurrence: usize,
                       owner_nodes: &[Node],
                       owner_bus_scene: bool,
                       registry: Option<Registry<'_>>|
         -> Result<Layout, Error> {
            let nodes = graph.read(Site::root(name))?;
            let (buses, field_sites) = shape(&nodes, &rules)?;
            let refs: Vec<(Site, String)> = field_sites
                .iter()
                .filter(|(_, k)| *k == Kind::RpcReference)
                .map(|(s, _)| Ok((s.clone(), graph.object(s)?.class)))
                .collect::<Result<_, Error>>()?;
            let targets = bind(graph, &refs, &field_sites[0].0, spawner, &rules)?;
            let mut bindings = BTreeMap::new();
            for ((reference, _), target) in refs.iter().zip(targets) {
                let binding = if let Some(index) = gm_index.get(&target) {
                    Binding::GameMode { serializer: *index }
                } else {
                    let (content_key, serializer) = scenes.endpoint(graph, &target)?;
                    Binding::Scene {
                        content_key,
                        serializer,
                    }
                };
                bindings.insert(reference.clone(), binding);
            }
            let sub_id = if owner_bus_scene {
                scenes.embedded_bus(graph, spawner)?
            } else {
                embedded_bus(owner_nodes, spawner)?
            };
            let (bundle, catalog, r) = match registry {
                Some(registry) => registry,
                None => (
                    bundle,
                    catalog.as_slice(),
                    *references
                        .get(&name.to_lowercase())
                        .ok_or_else(|| bad("spawned blueprint not registered"))?,
                ),
            };
            let layout = Layout {
                blueprint: name.to_lowercase(),
                asset: Asset {
                    bundle,
                    type_id: u16::try_from(r.class_id).map_err(|_| bad("spawned class ID"))?,
                    local_index: u16::try_from(r.local_index)
                        .map_err(|_| bad("spawned ordinal"))?,
                },
                catalog: catalog.to_vec(),
                bus_count: buses,
                sub_id,
                occurrence,
                fields: field_sites
                    .iter()
                    .map(|(site, kind)| {
                        Ok(Field {
                            kind: *kind,
                            class: graph.object(site)?.class,
                            asset: site.key.0.clone(),
                            object: site.key.1,
                            binding: bindings.get(site).copied(),
                            mirrors: mirrored(graph, site)?,
                        })
                    })
                    .collect::<Result<_, Error>>()?,
            };
            layout.profile()?;
            Ok(layout)
        };
        let spawner_for = |graph: &mut Graph<Assets>,
                           nodes: &[Node],
                           name: &str,
                           filter: &dyn Fn(&Site) -> bool|
         -> Result<Vec<Site>, Error> {
            let mut found = Vec::new();
            for node in nodes {
                if rules.spawners.contains(&node.object.class)
                    && filter(&node.site)
                    && graph.target_field(&node.site, "ParticipantLogicBlueprint")?
                        == (name.to_lowercase(), 0)
                {
                    found.push(node.site.clone());
                }
            }
            if found.is_empty() {
                return Err(bad(&format!("no spawner for {name}")));
            }
            Ok(found)
        };
        // Sequences: the state-sequence channel the game mode is bound to lists each
        // sequence ID with its blueprint; the game-mode state with that ID spawns it.
        let seq_rules = crate::sequences::Rules::from_json(&self_sequences_rules)?;
        let state_channel = gm_binding
            .iter()
            .find(|(site, _)| {
                gm_fields.iter().any(|(s, class, _)| {
                    s == *site && class == "StateSequenceChannelReferenceEntityData"
                })
            })
            .map(|(_, (target, _))| target.clone())
            .ok_or_else(|| bad("game mode has no bound state-sequence channel"))?;
        let mut sequence_ids = BTreeMap::new();
        let table = graph.object(&state_channel)?;
        for entry in table
            .fields
            .get("Sequences")
            .and_then(Value::as_array)
            .filter(|a| a.len() <= 64)
            .ok_or_else(|| bad("state-sequence channel table"))?
        {
            let fields = entry
                .as_fields()
                .ok_or_else(|| bad("state-sequence entry"))?;
            let mut target = |field: &str| -> Result<Key, Error> {
                let pointer = fields
                    .get(field)
                    .and_then(Value::as_pointer)
                    .ok_or_else(|| bad("state-sequence entry pointer"))?;
                graph
                    .source
                    .target(&state_channel.key.0, pointer)?
                    .ok_or_else(|| bad("null state-sequence entry"))
            };
            let (id, sequence) = (target("Id")?, target("Sequence")?);
            if sequence_ids.insert(sequence.0, id).is_some() {
                return Err(bad("repeated state-sequence blueprint"));
            }
        }
        let manager_node = {
            let mut found = manager_nodes
                .iter()
                .filter(|n| n.object.class == "StateSequenceManagerEntityData");
            let node = found.next().ok_or_else(|| bad("no sequence manager"))?;
            if found.next().is_some() {
                return Err(bad("several sequence managers"));
            }
            node.site.clone()
        };
        let manager = endpoint(&mut graph, &mut scenes, &manager_node)?;
        let manager_bus = embedded_bus(&manager_nodes, &manager_node)?;
        let mut sequences = Vec::new();
        for name in request.sequences.iter().chain(sequence_registries.keys()) {
            let name = name.to_lowercase();
            let nodes = graph.read(Site::root(&name))?;
            let (buses, field_sites) = crate::sequences::authored::shape(&nodes, &seq_rules)?;
            let refs: Vec<Site> = field_sites
                .iter()
                .filter(|(_, _, k)| *k == Kind::RpcReference)
                .map(|(s, _, _)| s.clone())
                .collect();
            let mut spawner = None;
            let mut bindings = BTreeMap::new();
            let mut channels = Vec::new();
            if !refs.is_empty() {
                let id = sequence_ids
                    .get(&name)
                    .ok_or_else(|| bad("sequence not listed by the state-sequence channel"))?
                    .clone();
                let mut states = Vec::new();
                for (index, (site, class, _)) in gm_fields.iter().enumerate() {
                    if class == "ParticipantSequenceSpawnerGameModeStateEntityData"
                        && graph.target_field(site, "SequenceId")? == id
                    {
                        states.push((index, site.clone()));
                    }
                }
                let [(state_index, state)] = <[_; 1]>::try_from(states)
                    .map_err(|_| bad("missing or ambiguous sequence spawner state"))?;
                spawner = Some(state_index);
                // Spawner targets; a channel reference with a port continues
                // through the scene channel it is bound to.
                let mut targets = Vec::new();
                for target in graph.links(&state, rules.spawner_channels)? {
                    if target.port == 0 {
                        targets.push(target.site);
                        continue;
                    }
                    let (bound, _) = gm_binding
                        .get(&target.site)
                        .ok_or_else(|| bad("sequence spawner reaches an unbound reference"))?
                        .clone();
                    for next in graph.links(&bound, target.port)? {
                        if next.port != 0 {
                            return Err(bad("sequence channel chain depth"));
                        }
                        targets.push(next.site);
                    }
                }
                let root = field_sites[0].0.clone();
                let linked: BTreeSet<Site> = graph
                    .links(&root, rules.root_channels)?
                    .into_iter()
                    .filter(|e| e.port == 0)
                    .map(|e| e.site)
                    .collect();
                let mut used = BTreeSet::new();
                for site in &refs {
                    if !linked.contains(site) {
                        return Err(bad("sequence channel is not linked from its root"));
                    }
                    let asset = graph.target_field(site, "ChannelAsset")?;
                    let mut chosen = None;
                    for (i, t) in targets.iter().enumerate() {
                        if used.contains(&i)
                            || graph.object(t)?.class != "LinkedSchematicChannelEntityData"
                            || graph.target_field(t, "ChannelAsset")? != asset
                        {
                            continue;
                        }
                        chosen = Some(i);
                        break;
                    }
                    let i = chosen.ok_or_else(|| bad("unbound sequence channel"))?;
                    used.insert(i);
                    channels.push(targets[i].clone());
                    let binding = if let Some(index) = gm_index.get(&targets[i]) {
                        Binding::GameMode { serializer: *index }
                    } else {
                        let (content_key, serializer) =
                            endpoint(&mut graph, &mut scenes, &targets[i])?;
                        Binding::Scene {
                            content_key,
                            serializer,
                        }
                    };
                    bindings.insert(site.clone(), binding);
                }
            }
            let goto = if nodes.iter().any(|n| n.object.class == GOTO_COMMAND) {
                let mut precision = nodes.iter().filter(|n| n.object.class == GOTO_PRECISION);
                let node = precision
                    .next()
                    .ok_or_else(|| bad("go-to sequence has no precision setting"))?;
                if precision.next().is_some() {
                    return Err(bad("go-to sequence has several precision settings"));
                }
                let radius = finite(
                    node.object.fields.get("LocationReachedRadius"),
                    "go-to reach radius",
                )?;
                let (mut points, mut limits) = (Vec::new(), Vec::new());
                for channel in &channels {
                    for port in graph.link_ports(channel)? {
                        for target in graph.links(channel, port)? {
                            if target.port != 0 {
                                continue;
                            }
                            let object = graph.object(&target.site)?;
                            match object.class.as_str() {
                                LOCATOR => points.push(location(&object)?),
                                SPEED_LIMIT => limits.push(finite(
                                    object.fields.get("SpeedLimit"),
                                    "go-to speed limit",
                                )?),
                                _ => {}
                            }
                        }
                    }
                }
                let ([point], [speed_limit]) = (points.as_slice(), limits.as_slice()) else {
                    return Err(bad("missing or ambiguous go-to waypoint or speed limit"));
                };
                // Both are finite: `finite` refused anything else.
                if radius <= 0. || *speed_limit <= 0. {
                    return Err(bad("go-to radius or speed limit"));
                }
                Some(Goto {
                    point: *point,
                    radius,
                    speed_limit: *speed_limit,
                })
            } else {
                None
            };
            let (sequence_bundle, sequence_catalog, r) = match sequence_registries.get(&name) {
                Some((b, c, r)) => (*b, c, r),
                None => (
                    bundle,
                    &catalog,
                    references
                        .get(&name)
                        .ok_or_else(|| bad("sequence not registered"))?,
                ),
            };
            let layout = Layout {
                blueprint: name.clone(),
                asset: Asset {
                    bundle: sequence_bundle,
                    type_id: u16::try_from(r.class_id).map_err(|_| bad("sequence class ID"))?,
                    local_index: u16::try_from(r.local_index)
                        .map_err(|_| bad("sequence ordinal"))?,
                },
                catalog: sequence_catalog.clone(),
                bus_count: buses,
                sub_id: manager_bus,
                occurrence: 0,
                fields: field_sites
                    .iter()
                    .map(|(site, class, kind)| {
                        Ok(Field {
                            kind: *kind,
                            class: class.clone(),
                            asset: site.key.0.clone(),
                            object: site.key.1,
                            binding: bindings.get(site).copied(),
                            mirrors: mirrored(&mut graph, site)?,
                        })
                    })
                    .collect::<Result<_, Error>>()?,
            };
            layout.profile()?;
            sequences.push(Sequence {
                layout,
                manager,
                spawner,
                goto,
                wander: sequence::wander(&mut graph, &nodes)?,
            });
        }
        let mut start_logics = Vec::new();
        for name in &request.start_logics {
            let spawners = spawner_for(&mut graph, &scene_nodes, name, &|s| within(s, &instance))?;
            for (occurrence, spawner) in spawners.iter().enumerate() {
                start_logics.push(spawned(
                    &mut graph,
                    &mut scenes,
                    name,
                    spawner,
                    occurrence,
                    &scene_nodes,
                    true,
                    None,
                )?);
            }
        }
        let mut game_mode_logics = Vec::new();
        let mut award_values = BTreeMap::new();
        for name in &request.game_mode_logics {
            let spawners = spawner_for(&mut graph, &gm_nodes, name, &|_| true)?;
            for (occurrence, spawner) in spawners.iter().enumerate() {
                if name.eq_ignore_ascii_case(awards::BLUEPRINT) {
                    if occurrence != 0 {
                        return Err(bad("ambiguous award spawner"));
                    }
                    let nodes = graph.read(Site::root(name))?;
                    let (_, fields) = shape(&nodes, &rules)?;
                    let refs = fields
                        .iter()
                        .filter(|(_, kind)| *kind == Kind::RpcReference)
                        .map(|(site, _)| Ok((site.clone(), graph.object(site)?.class)))
                        .collect::<Result<Vec<_>, Error>>()?;
                    let targets = bind(&mut graph, &refs, &fields[0].0, spawner, &rules)?;
                    award_values = awards::bound_values(&mut graph, &refs, targets, &gm_binding)?;
                }
                game_mode_logics.push(spawned(
                    &mut graph,
                    &mut scenes,
                    name,
                    spawner,
                    occurrence,
                    &gm_nodes,
                    false,
                    None,
                )?);
            }
        }
        // The linked logic in the other start areas joined to this area's external
        // participant set (E996: the variants of one event).
        let mut linked_logics = Vec::new();
        if let Some(name) = &request.linked_logic {
            let own: BTreeSet<Endpoint> = graph
                .links(&instance_site, EXTERNAL_SET)?
                .into_iter()
                .collect();
            if own.is_empty() {
                return Err(bad("start area has no external participant set"));
            }
            let mut linked = Vec::new();
            // Only instances placed beside this one: an object key inside a prefab
            // asset is shared by every instance of that prefab.
            for node in &scene_nodes {
                if node.site.key == instance
                    || node.site.bus != instance_site.bus
                    || node.site.parents != instance_site.parents
                    || !node.object.class.ends_with("ReferenceObjectData")
                {
                    continue;
                }
                let targets: BTreeSet<Endpoint> =
                    graph.links(&node.site, EXTERNAL_SET)?.into_iter().collect();
                if targets == own {
                    linked.push(node.site.key.clone());
                }
            }
            for other in linked {
                let spawners = spawner_for(&mut graph, &scene_nodes, name, &|s| within(s, &other))?;
                for (occurrence, spawner) in spawners.iter().enumerate() {
                    linked_logics.push(spawned(
                        &mut graph,
                        &mut scenes,
                        name,
                        spawner,
                        occurrence,
                        &scene_nodes,
                        true,
                        None,
                    )?);
                }
            }
        }
        let mut scene_logics = Vec::new();
        for (scene_name, blueprint, scene_bundle, scene_catalog, r) in &scene_registries {
            let root = scenes
                .roots
                .get(scene_name)
                .ok_or_else(|| bad("scene-logic scene is not a startup scene"))?
                .clone();
            let nodes = graph.read(root)?;
            let spawners = spawner_for(&mut graph, &nodes, blueprint, &|_| true)?;
            let [spawner] = <[Site; 1]>::try_from(spawners)
                .map_err(|_| bad("ambiguous scene-logic spawner"))?;
            let layout = spawned(
                &mut graph,
                &mut scenes,
                blueprint,
                &spawner,
                0,
                &nodes,
                true,
                Some((*scene_bundle, scene_catalog.as_slice(), *r)),
            )?;
            scene_logics.push(SceneLogic {
                scene_content_key: crate::assets::scene_key(scene_name),
                layout,
            });
        }
        let mut ai = ai::extract(&mut graph, &instance_site, &manager_nodes, rival_links)?;
        if !ai.spawns.is_empty() {
            let unique = |class: &str| -> Result<Site, Error> {
                let nodes: Vec<_> = manager_nodes
                    .iter()
                    .filter(|n| n.object.class == class)
                    .collect();
                let [node] = nodes.as_slice() else {
                    return Err(bad("event AI manager missing or ambiguous"));
                };
                Ok(node.site.clone())
            };
            ai.managers = Some(crate::ai::managers(
                &mut graph,
                &mut scenes,
                &unique("AiParticipantManagerEntityData")?,
                &unique("AiActorManagerEntityData")?,
                ai_links,
            )?);
        }
        Ok(Event {
            award_values,
            ai,
            invitations: invitations::extract(&mut graph, &mut scenes)?,
            restart: restart::destinations(&mut graph, &instance_site)?,
            scene,
            scene_content_key,
            game_mode_id,
            launcher: launcher_serializer,
            start_volume,
            rewards,
            placement_rewards,
            game_mode,
            start_logics,
            game_mode_logics,
            sequences,
            scene_logics,
            linked_logics,
            area_notifications,
            scene_notifications,
            game_mode_notifications,
            dnf_timeout,
        })
    }
    /// The SpeedWall category of a game-mode ID: the `CategoryName` of the SpeedWall
    /// asset in the installed shape container whose `GameModeIds` name it (E996:
    /// `SpeedWall_SprintRace` for `gameplay/gamemodes/sprint_race_007`). The account's
    /// race-event record carries it as its type string.
    pub fn speed_wall_category(&mut self, game_mode_id: &str) -> Result<String, Error> {
        const CONTAINER: &str = "gameplay/speedwallshapecontainer";
        let wanted = game_mode_id.to_lowercase();
        let container = self
            .assets
            .objects(CONTAINER)?
            .first()
            .cloned()
            .ok_or_else(|| bad("speed wall container"))?;
        let walls = container
            .fields
            .get("SpeedWallData")
            .and_then(Value::as_array)
            .filter(|a| a.len() <= 256)
            .ok_or_else(|| bad("speed wall list"))?
            .to_vec();
        let mut found = Vec::new();
        for wall in &walls {
            let pointer = wall.as_pointer().ok_or_else(|| bad("speed wall pointer"))?;
            let Some(key) = self.assets.target(CONTAINER, pointer)? else {
                continue;
            };
            let object = self
                .assets
                .objects(&key.0)?
                .get(key.1)
                .cloned()
                .ok_or_else(|| bad("speed wall index"))?;
            if object.class != "SpeedWallAsset" {
                return Err(bad("speed wall class"));
            }
            let ids = object
                .fields
                .get("GameModeIds")
                .and_then(Value::as_array)
                .filter(|a| a.len() <= 1024)
                .ok_or_else(|| bad("speed wall game modes"))?;
            for id in ids {
                let pointer = id.as_pointer().ok_or_else(|| bad("game mode pointer"))?;
                if self
                    .assets
                    .target(&key.0, pointer)?
                    .is_some_and(|k| k.0 == wanted)
                {
                    let category = object
                        .fields
                        .get("CategoryName")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty() && s.len() <= 128)
                        .ok_or_else(|| bad("speed wall category"))?;
                    found.push(category.to_owned());
                }
            }
        }
        let [category] = <[String; 1]>::try_from(found)
            .map_err(|_| bad("missing or ambiguous speed wall category"))?;
        Ok(category)
    }
}
