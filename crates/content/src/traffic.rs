// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed traffic personas, manager bindings and road spline geometry.
//! Runtime allocation, population and simulation policy belong to the host.
use crate::{
    Error,
    assets::{AssetIndex, scene_key},
    scenes,
    vehicle_profiles::traffic as vehicles,
};
use nfs_frostbite::ebx::{Object, Value};
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PERSONAS: usize = 128;
pub const MAX_SPLINES: usize = 512;
pub const MAX_SEGMENTS: usize = 16384;
pub const MAX_POINTS: usize = 131072;
const MAX_SEGMENT_POINTS: usize = 4096;
type Key = (String, usize);
mod curve;
pub use curve::Curve;
mod links;
pub use links::Links;

#[derive(Clone, Debug)]
pub struct Persona {
    pub name: String,
    pub index: u16,
    pub vehicle: String,
    pub tags: Vec<[u8; 16]>,
}
#[derive(Clone, Debug)]
pub struct Segment {
    /// Authored desired speed, retained in the asset's native units.
    pub speed: f32,
    pub points: Vec<[f32; 3]>,
    pub curve: Curve,
}
#[derive(Clone, Debug)]
pub struct Spline {
    /// Owning startup scene and local asset catalog identity, independently of
    /// this road's position in the manager's population list.
    pub scene: u32,
    pub index: u16,
    pub catalog_count: u16,
    pub density: u16,
    pub tags: Vec<[u8; 16]>,
    pub segments: Vec<Segment>,
}
#[derive(Clone, Debug)]
pub struct Content {
    pub managers: nfs_world::replication::players::ai::Managers,
    pub spline_class: u16,
    pub density: f32,
    pub personas: Vec<Persona>,
    pub vehicles: BTreeMap<String, vehicles::Definition>,
    pub splines: Vec<Spline>,
}
impl Content {
    pub fn load(path: &std::path::Path) -> Result<Self, Error> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take((16 << 20) + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16 << 20 {
            return Err(bad("content byte bound"));
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| bad("JSON"))?)
    }
    pub fn from_json(v: &Json) -> Result<Self, Error> {
        json_keys(
            v,
            &[
                "format",
                "version",
                "build_sha256",
                "scene",
                "spline_class",
                "managers",
                "density",
                "personas",
                "vehicles",
                "splines",
            ],
        )?;
        if v["format"] != "nfs-traffic-content"
            || v["version"] != 4
            || v["build_sha256"] != nfs_services::SUPPORTED_BUILD_SHA256
        {
            return Err(bad("content identity"));
        }
        json_keys(&v["managers"], &["participants", "actors", "alive"])?;
        let managers = nfs_world::replication::players::ai::Managers {
            alive: crate::ai::state_from_json(&v["managers"]["alive"])?,
            scene: number(&v["scene"])?,
            participants: number(&v["managers"]["participants"])?,
            actors: number(&v["managers"]["actors"])?,
        };
        if managers.participants == managers.actors
            || (managers.alive.scene == managers.scene
                && [managers.participants, managers.actors].contains(&managers.alive.serializer))
            || managers.participants >= nfs_world::replication::sublevel::MAX_SERIALIZERS
            || managers.actors >= nfs_world::replication::sublevel::MAX_SERIALIZERS
        {
            return Err(bad("manager ordinals"));
        }
        let spline_class = number(&v["spline_class"])?;
        if spline_class == 0 {
            return Err(bad("spline class"));
        }
        let density = json_float(&v["density"])?;
        if !(0. ..=100.).contains(&density) {
            return Err(bad("density"));
        }
        let mut vehicles = BTreeMap::new();
        for value in json_array(&v["vehicles"], MAX_PERSONAS)? {
            let d =
                vehicles::Definition::from_json(value).map_err(|_| bad("vehicle definition"))?;
            if d.bundle != d.bundle.to_lowercase() || vehicles.insert(d.bundle.clone(), d).is_some()
            {
                return Err(bad("duplicate vehicle bundle"));
            }
        }
        let mut personas = Vec::new();
        let mut names = BTreeSet::new();
        for value in json_array(&v["personas"], MAX_PERSONAS)? {
            json_keys(value, &["name", "index", "vehicle", "tags"])?;
            let name = json_text(&value["name"])?;
            let vehicle = json_text(&value["vehicle"])?;
            let index = number(&value["index"])?;
            if name.len() > 64
                || !names.insert(name.clone())
                || index as usize != personas.len()
                || !vehicles.contains_key(&vehicle)
            {
                return Err(bad("persona identity"));
            }
            personas.push(Persona {
                name,
                index,
                vehicle,
                tags: json_tags(&value["tags"])?,
            });
        }
        if personas.is_empty()
            || vehicles.len()
                != personas
                    .iter()
                    .map(|p| &p.vehicle)
                    .collect::<BTreeSet<_>>()
                    .len()
        {
            return Err(bad("persona vehicle closure"));
        }
        let mut splines = Vec::new();
        let mut identities = BTreeSet::new();
        let mut catalogs = BTreeMap::new();
        let mut total_segments = 0;
        let mut total_points = 0;
        for value in json_array(&v["splines"], MAX_SPLINES)? {
            json_keys(
                value,
                &[
                    "scene",
                    "index",
                    "catalog_count",
                    "density",
                    "tags",
                    "segments",
                ],
            )?;
            let scene = number::<u32>(&value["scene"])?;
            let index = number(&value["index"])?;
            let catalog_count = number::<u16>(&value["catalog_count"])?;
            let density = number(&value["density"])?;
            if index >= catalog_count
                || catalog_count as usize > MAX_SPLINES
                || density > 10000
                || !identities.insert((scene, index))
                || catalogs
                    .insert(scene, catalog_count)
                    .is_some_and(|previous| previous != catalog_count)
            {
                return Err(bad("spline identity/density"));
            }
            let mut segments = Vec::new();
            for value in json_array(&value["segments"], MAX_SEGMENTS)? {
                total_segments += 1;
                if total_segments > MAX_SEGMENTS {
                    return Err(bad("segment bound"));
                }
                json_keys(
                    value,
                    &["speed", "points", "controls", "times", "distances"],
                )?;
                let speed = json_float(&value["speed"])?;
                if !(0. ..=500.).contains(&speed) {
                    return Err(bad("segment speed"));
                }
                let values = json_array(&value["points"], MAX_SEGMENT_POINTS)?;
                if values.len() < 2 {
                    return Err(bad("segment points"));
                }
                total_points += values.len();
                if total_points > MAX_POINTS {
                    return Err(bad("point bound"));
                }
                let mut points = Vec::new();
                for p in values {
                    if json_array(p, 3)?.len() != 3 {
                        return Err(bad("point"));
                    }
                    points.push([json_float(&p[0])?, json_float(&p[1])?, json_float(&p[2])?]);
                }
                let controls = json_array(&value["controls"], 4)?
                    .iter()
                    .map(|v| {
                        if json_array(v, 3)?.len() != 3 {
                            return Err(bad("curve control"));
                        }
                        Ok([json_float(&v[0])?, json_float(&v[1])?, json_float(&v[2])?])
                    })
                    .collect::<Result<Vec<_>, Error>>()?
                    .try_into()
                    .map_err(|_| bad("curve controls"))?;
                let floats = |key| {
                    json_array(&value[key], MAX_SEGMENT_POINTS)?
                        .iter()
                        .map(json_float)
                        .collect::<Result<Vec<_>, Error>>()
                };
                let curve = Curve::new(controls, floats("times")?, floats("distances")?)?;
                curve.validate_points(&points)?;
                segments.push(Segment {
                    speed,
                    points,
                    curve,
                });
            }
            splines.push(Spline {
                scene,
                index,
                catalog_count,
                density,
                tags: json_tags(&value["tags"])?,
                segments,
            });
        }
        if splines.is_empty() {
            return Err(bad("spline count"));
        }
        Ok(Self {
            managers,
            spline_class,
            density,
            personas,
            vehicles,
            splines,
        })
    }
}
fn json_keys(v: &Json, keys: &[&str]) -> Result<(), Error> {
    if v.as_object()
        .is_none_or(|o| o.len() != keys.len() || keys.iter().any(|k| !o.contains_key(*k)))
    {
        return Err(bad("JSON fields"));
    }
    Ok(())
}
fn number<T: TryFrom<u64>>(v: &Json) -> Result<T, Error> {
    T::try_from(v.as_u64().ok_or_else(|| bad("integer"))?).map_err(|_| bad("integer range"))
}
fn json_float(v: &Json) -> Result<f32, Error> {
    v.as_f64()
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
        .ok_or_else(|| bad("float"))
}
fn json_text(v: &Json) -> Result<String, Error> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or_else(|| bad("text"))
}
fn json_array(v: &Json, max: usize) -> Result<&[Json], Error> {
    v.as_array()
        .filter(|a| a.len() <= max)
        .map(Vec::as_slice)
        .ok_or_else(|| bad("array bound"))
}
fn json_tags(v: &Json) -> Result<Vec<[u8; 16]>, Error> {
    let mut tags = Vec::new();
    for value in json_array(v, 128)? {
        let value = json_text(value)?;
        if value.len() != 32 || !value.is_ascii() {
            return Err(bad("tag GUID"));
        }
        let mut tag = [0; 16];
        for (i, b) in tag.iter_mut().enumerate() {
            *b = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|_| bad("tag GUID"))?;
        }
        if tags.contains(&tag) {
            return Err(bad("duplicate tag"));
        }
        tags.push(tag);
    }
    Ok(tags)
}

fn bad(message: &str) -> Error {
    Error::Content(format!("traffic: {message}"))
}
fn object(index: &mut AssetIndex, key: &Key) -> Result<Object, Error> {
    index
        .assets
        .objects(&key.0)?
        .get(key.1)
        .cloned()
        .ok_or_else(|| bad("object index"))
}
fn target(index: &mut AssetIndex, source: &str, value: &Value) -> Result<Option<Key>, Error> {
    index
        .assets
        .target(source, value.as_pointer().ok_or_else(|| bad("pointer"))?)
}
fn field<'a>(object: &'a Object, name: &str) -> Result<&'a Value, Error> {
    object.fields.get(name).ok_or_else(|| bad(name))
}
fn follow(
    index: &mut AssetIndex,
    key: &Key,
    name: &str,
    class: &str,
) -> Result<(Key, Object), Error> {
    let source = object(index, key)?;
    let next = target(index, &key.0, field(&source, name)?)?
        .ok_or_else(|| bad("null required reference"))?;
    let out = object(index, &next)?;
    if out.class != class {
        return Err(bad("reference class"));
    }
    Ok((next, out))
}
fn text(value: &Value) -> Result<String, Error> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or_else(|| bad("text"))
}
fn float(value: &Value) -> Result<f32, Error> {
    match value {
        Value::Float32(v) if v.is_finite() => Ok(*v),
        _ => Err(bad("finite float")),
    }
}
fn tags(index: &mut AssetIndex, key: &Key, source: &Object) -> Result<Vec<String>, Error> {
    let values = field(source, "VehicleTags")?
        .as_array()
        .filter(|a| a.len() <= 128)
        .ok_or_else(|| bad("tags"))?;
    let mut seen = BTreeSet::new();
    for value in values {
        let key = target(index, &key.0, value)?.ok_or_else(|| bad("null tag"))?;
        let tag = object(index, &key)?;
        let guid = tag.guid.ok_or_else(|| bad("unexported tag"))?;
        if !seen.insert(nfs_frostbite::hex(&guid)) {
            return Err(bad("duplicate tag"));
        }
    }
    Ok(seen.into_iter().collect())
}
fn points(value: &Value) -> Result<Vec<[f32; 3]>, Error> {
    let values = value
        .as_array()
        .filter(|a| a.len() >= 2 && a.len() <= MAX_SEGMENT_POINTS)
        .ok_or_else(|| bad("segment points"))?;
    values
        .iter()
        .map(|v| {
            let f = v.as_fields().ok_or_else(|| bad("point"))?;
            Ok([
                float(f.get("x").ok_or_else(|| bad("x"))?)?,
                float(f.get("y").ok_or_else(|| bad("y"))?)?,
                float(f.get("z").ok_or_else(|| bad("z"))?)?,
            ])
        })
        .collect()
}

/// Extract installed definitions. The spline class identifier comes from the
/// executable's audited build profile, independently of runtime bundle IDs.
pub fn extract(index: &mut AssetIndex) -> Result<Json, Error> {
    let spline_class = index.identity.traffic_spline_class_id;
    if spline_class == 0 {
        return Err(bad("spline class"));
    }
    let rules = index.identity.scenes.clone();
    let directory = rules
        .level
        .rsplit_once('/')
        .ok_or_else(|| bad("level directory"))?
        .0;
    let gameplay = format!("{directory}/gameplay").to_lowercase();
    let root = index.scene(&rules.level)?;
    let traffic_bundles: BTreeSet<_> = root
        .level_references
        .into_iter()
        .map(|n| n.to_lowercase())
        .collect();
    let (_, instances) = scenes::construct(&mut index.assets, &rules, &gameplay, true)?;
    let startup = index.startup_entries(&rules.level)?;
    let scene_names = startup
        .iter()
        .filter(|s| s.kind == crate::world::StartupKind::SubLevel)
        .map(|s| s.name.to_lowercase())
        .collect();
    let ports = index.identity.traffic_links;
    let alive = {
        use crate::graph::{
            Graph,
            scenes::{SceneLookup, scene_roots},
        };
        let mut graph = Graph::new(&mut index.assets);
        let roots = scene_roots(&mut graph, &rules.level, &startup)?;
        let managers: Vec<_> = graph
            .read(
                roots
                    .get(&gameplay)
                    .ok_or_else(|| bad("gameplay scene"))?
                    .clone(),
            )?
            .into_iter()
            .filter(|n| n.object.class == "AiParticipantManagerEntityData")
            .collect();
        if managers.len() != 1 {
            return Err(bad("participant manager count"));
        }
        let mut lookup = SceneLookup {
            roots,
            rules: &rules,
            endpoints: BTreeMap::new(),
            loaded: BTreeSet::new(),
        };
        crate::ai::resolve(
            &mut graph,
            &mut lookup,
            &managers[0].site,
            index.identity.ai_links,
        )?
    };
    let linked = links::resolve(
        &mut crate::graph::Graph::new(&mut index.assets),
        crate::graph::Site::root(&gameplay),
        &scene_names,
        ports,
    )?;
    let mut participant_manager = None;
    let mut actor_manager = None;
    let mut density = None;
    let mut personas = Vec::new();
    let mut definitions = BTreeMap::new();
    let mut names = BTreeSet::new();
    for instance in instances {
        let key = (instance.asset, instance.index);
        let value = object(index, &key)?;
        match value.class.as_str() {
            "AiParticipantManagerEntityData" => {
                if participant_manager
                    .replace(
                        instance
                            .serializer
                            .ok_or_else(|| bad("participant manager serializer"))?,
                    )
                    .is_some()
                {
                    return Err(bad("duplicate participant manager"));
                }
            }
            "AiActorManagerEntityData" => {
                if actor_manager
                    .replace(
                        instance
                            .serializer
                            .ok_or_else(|| bad("actor manager serializer"))?,
                    )
                    .is_some()
                {
                    return Err(bad("duplicate actor manager"));
                }
            }
            "TrafficManagerEntityData" => {
                let d = float(field(&value, "Density")?)?;
                if !(0. ..=100.).contains(&d) || density.replace(d).is_some() {
                    return Err(bad("traffic density"));
                }
            }
            _ => (),
        }
    }
    for key in linked.personas {
        let value = object(index, &key)?;
        let (item, _) = follow(index, &key, "VehicleItemData", "RaceVehicleItemData")?;
        let (_, reference) = follow(index, &item, "RaceVehicle", "RaceVehicleReference")?;
        let bundle = text(
            field(&reference, "BundleReference")?
                .as_fields()
                .and_then(|f| f.get("Name"))
                .ok_or_else(|| bad("bundle reference"))?,
        )?
        .to_lowercase();
        if !traffic_bundles.contains(&bundle) {
            return Err(bad("traffic vehicle bundle is not preloaded"));
        }
        let name = text(field(&value, "PersonaName")?)?;
        if name.len() > 64 || !names.insert(name.clone()) || personas.len() >= MAX_PERSONAS {
            return Err(bad("persona names/count"));
        }
        // Traffic uses stock vehicles. Refuse authored modifications until
        // their state is modeled instead of silently dropping them.
        for name in [
            "OverrideCarColor",
            "OverridePerformance",
            "OverrideGripValue",
        ] {
            if field(&value, name)?.as_bool() != Some(false) {
                return Err(bad("modified traffic persona"));
            }
        }
        if field(&value, "AdditionalItems")?
            .as_array()
            .is_none_or(|a| !a.is_empty())
        {
            return Err(bad("traffic additional items"));
        }
        if !definitions.contains_key(&bundle) {
            definitions.insert(bundle.clone(), vehicles::extract(index, &bundle)?);
        }
        personas.push(json!({"name":name,"index":personas.len(),"vehicle":bundle,"tags":tags(index,&key,&value)?}));
    }
    if personas.is_empty() {
        return Err(bad("no traffic personas"));
    }
    let mut registries = BTreeMap::new();
    let mut splines = Vec::new();
    let mut total_segments = 0;
    let mut total_points = 0;
    for (scene, key) in linked.roads {
        if !registries.contains_key(&scene) {
            let registered = index.registered_objects(&scene, "TrafficSplineEntityData")?;
            if registered.is_empty() || registered.len() > MAX_SPLINES {
                return Err(bad("spline catalog count"));
            }
            registries.insert(scene.clone(), registered);
        }
        let registered = &registries[&scene];
        let ordinal = registered
            .iter()
            .position(|k| *k == key)
            .ok_or_else(|| bad("linked spline is not registered in its scene"))?;
        let count = registered.len();
        let value = object(index, &key)?;
        let density = match field(&value, "DesiredDensity")? {
            Value::Int32(n) if (0..=10000).contains(n) => *n,
            _ => return Err(bad("spline density")),
        };
        let mut segments = Vec::new();
        let (speed_key, speeds) =
            follow(index, &key, "SpeedSegmentAsset", "SplineSpeedSegmentsAsset")?;
        let entries = field(&speeds, "SpeedSegments")?
            .as_array()
            .ok_or_else(|| bad("speed segments"))?;
        for entry in entries {
            total_segments += 1;
            if total_segments > MAX_SEGMENTS {
                return Err(bad("segment bound"));
            }
            let segment_key =
                target(index, &speed_key.0, entry)?.ok_or_else(|| bad("null segment"))?;
            let segment = object(index, &segment_key)?;
            if segment.class != "SplineSpeedSegment" {
                return Err(bad("segment class"));
            }
            let speed = float(field(&segment, "DesiredSpeed")?)?;
            if !(0. ..=500.).contains(&speed) {
                return Err(bad("desired speed"));
            }
            let p = points(
                field(&segment, "Spline")?
                    .as_fields()
                    .and_then(|f| f.get("SegmentPoints"))
                    .ok_or_else(|| bad("spline points"))?,
            )?;
            total_points += p.len();
            if total_points > MAX_POINTS {
                return Err(bad("point bound"));
            }
            let spline = field(&segment, "Spline")?
                .as_fields()
                .ok_or_else(|| bad("spline"))?;
            let base = spline
                .get("MatrixFormData")
                .and_then(Value::as_fields)
                .and_then(|v| v.get("BaseData"))
                .and_then(Value::as_fields)
                .ok_or_else(|| bad("curve controls"))?;
            let controls = ["P0", "P1", "P2", "P3"]
                .into_iter()
                .map(|name| {
                    let f = base
                        .get(name)
                        .and_then(Value::as_fields)
                        .ok_or_else(|| bad("curve control"))?;
                    Ok([
                        float(f.get("x").ok_or_else(|| bad("x"))?)?,
                        float(f.get("y").ok_or_else(|| bad("y"))?)?,
                        float(f.get("z").ok_or_else(|| bad("z"))?)?,
                    ])
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let floats = |key| {
                spline
                    .get(key)
                    .and_then(Value::as_array)
                    .filter(|v| v.len() < MAX_SEGMENT_POINTS)
                    .ok_or_else(|| bad("curve table"))?
                    .iter()
                    .map(float)
                    .collect::<Result<Vec<_>, Error>>()
            };
            segments.push(json!({"speed":speed,"points":p,"controls":controls,"times":floats("SegmentEndTime")?,"distances":floats("SegmentEndDistance")?}));
        }
        splines.push(
            json!({"scene":scene_key(&scene),"index":ordinal,"catalog_count":count,
            "density":density,"tags":tags(index,&key,&value)?,"segments":segments}),
        );
    }
    Ok(
        json!({"format":"nfs-traffic-content","version":4,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "scene":scene_key(&gameplay),"spline_class":spline_class,
        "managers":{"participants":participant_manager.ok_or_else(||bad("missing participant manager"))?,"actors":actor_manager.ok_or_else(||bad("missing actor manager"))?,"alive":crate::ai::state_json(alive)},
        "density":density.ok_or_else(||bad("missing traffic manager"))?,"personas":personas,"vehicles":definitions.into_values().collect::<Vec<_>>(),"splines":splines}),
    )
}

pub(crate) fn build(index: &mut AssetIndex) -> Result<Vec<u8>, Error> {
    let value = extract(index)?;
    Content::from_json(&value)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| bad("JSON encoding"))?;
    if bytes.len() > 16 << 20 {
        return Err(bad("content byte bound"));
    }
    Ok(bytes)
}
