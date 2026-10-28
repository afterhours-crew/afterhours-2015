// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Mission-chapter values the client sees through scene notifications. A
//! chapter's participant property accessor writes an expression over its
//! objectives' IsActive/IsCompleted outputs into a channel field of the intro
//! participant logic; the progression scene forwards that field through its
//! interface to another sub-world's participant property, whose client
//! notification the host sends when the value changes. Installed intro:
//! IsActive(IntroDriveToClub) to spawnpoints s4 (Club marker and garage-entry
//! box), IsActive(spike & amy) or IsActive(meet & race manu) to spawnpoints
//! s5 (entrance prompt gate), and not IsCompleted(spike & amy) to garage s24
//! (car-selection camera). Official E1026: s4 1 at 37.118 s, 0 at 290.521 s;
//! s24 true at 36.319 s, false at 651.925 s.
use crate::{Error, assets::AssetIndex, graph::*};
use nfs_frostbite::ebx::Value;
use std::collections::BTreeSet;

const INT_ACCESSOR: &str = "IntParticipantPropertyAccessorEntityData";
const BOOL_ACCESSOR: &str = "BoolParticipantPropertyAccessorEntityData";
const ACCESSOR_VALUE: u32 = 225_375_086;
/// Accessor links to the channel field it writes.
const INT_CHANNEL: u32 = 2_249_073_098;
const BOOL_CHANNEL: u32 = 2_526_680_631;
/// RaceObjective outputs.
const OBJECTIVE_ACTIVE: u32 = 1_010_522_579;
const OBJECTIVE_COMPLETED: u32 = 1_514_830_046;
const CAST_BOOL: u32 = 176_040_160;
const CAST_INT: u32 = 245_372_264;
const LOGIC_OUT: u32 = 193_453_899;
const NOT_IN: u32 = 5_862_146;
const MAX_NOTIFICATIONS: usize = 32;
/// A resolved notification before its serializer: kind, value, scene,
/// object, default and (scene, object) of the notification itself.
type Found = (Kind, Expr, String, Key, i32, (String, Key));
const BOX: &str = "RaceBoxTriggerEntityData";
const BOX_ENABLE: u32 = 2_342_790_116;
const BOX_DISABLE: u32 = 4_177_663_665;
const COMPARE_BOOL: u32 = 2_088_812_747;
const MAX_NODES: usize = 32;

/// A boolean over intro objectives (stable objective ids).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expr {
    Active(u64),
    Completed(u64),
    Not(Box<Expr>),
    Any(Vec<Expr>),
    All(Vec<Expr>),
}
impl Expr {
    pub fn eval(&self, objective: &impl Fn(u64) -> (bool, bool)) -> bool {
        match self {
            Self::Active(id) => objective(*id).0,
            Self::Completed(id) => objective(*id).1,
            Self::Not(e) => !e.eval(objective),
            Self::Any(all) => all.iter().any(|e| e.eval(objective)),
            Self::All(all) => all.iter().all(|e| e.eval(objective)),
        }
    }
    pub fn objectives(&self, out: &mut BTreeSet<u64>) {
        match self {
            Self::Active(id) | Self::Completed(id) => {
                out.insert(*id);
            }
            Self::Not(e) => e.objectives(out),
            Self::Any(all) | Self::All(all) => all.iter().for_each(|e| e.objectives(out)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// `IntParticipantPropertyClientNotification`: the value as a word.
    Int,
    /// `BoolPropertyClientNotification`: m0 true, m1 false.
    Bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    pub kind: Kind,
    pub value: Expr,
    pub scene_key: u32,
    pub serializer: usize,
    /// The target property's default: a value equal to it is no change.
    pub default: i32,
    /// The value enables a `RaceBoxTriggerEntityData` through client logic
    /// (installed: spawnpoints s4, the garage-entry box whose automatic
    /// request starts the return while it is enabled).
    pub entry_box: bool,
}

pub(crate) fn stable<S: Source>(
    graph: &mut Graph<S>,
    port: u32,
    site: &Site,
) -> Result<u64, Error> {
    let endpoints = graph.links(site, port)?;
    let [endpoint] = endpoints.as_slice() else {
        return Err(bad("chapter objective stable id"));
    };
    let object = graph.object(&endpoint.site)?;
    if object.class != "StableUniqueIdEntityData" {
        return Err(bad("chapter objective stable id class"));
    }
    match object.fields.get("StableUniqueId64") {
        Some(Value::UInt64(id)) if *id != 0 => Ok(*id),
        Some(Value::Int64(id)) if *id != 0 => Ok(*id as u64),
        _ => Err(bad("chapter objective stable id value")),
    }
}

fn expr<S: Source>(
    graph: &mut Graph<S>,
    stable_port: u32,
    endpoint: Endpoint,
    depth: usize,
) -> Result<Expr, Error> {
    if depth > MAX_NODES {
        return Err(bad("chapter value depth"));
    }
    let object = graph.object(&endpoint.site)?;
    let site = &endpoint.site;
    Ok(match (object.class.as_str(), endpoint.port) {
        ("RaceObjectiveEntityData", OBJECTIVE_ACTIVE) => {
            Expr::Active(stable(graph, stable_port, site)?)
        }
        ("RaceObjectiveEntityData", OBJECTIVE_COMPLETED) => {
            Expr::Completed(stable(graph, stable_port, site)?)
        }
        ("PropertyCastEntityData", CAST_INT) | ("PropertyCastEntityData", CAST_BOOL) => {
            let input = graph.input(site, CAST_BOOL)?;
            expr(graph, stable_port, input, depth + 1)?
        }
        ("NotEntityData", LOGIC_OUT) => {
            let input = graph.input(site, NOT_IN)?;
            Expr::Not(Box::new(expr(graph, stable_port, input, depth + 1)?))
        }
        ("OrEntityData" | "AndEntityData", LOGIC_OUT) => {
            let ports = graph.input_ports(site)?;
            if ports.len() < 2 || ports.len() > 16 {
                return Err(bad("chapter value inputs"));
            }
            let mut all = vec![];
            for port in ports {
                let input = graph.input(site, port)?;
                all.push(expr(graph, stable_port, input, depth + 1)?);
            }
            if object.class == "OrEntityData" {
                Expr::Any(all)
            } else {
                Expr::All(all)
            }
        }
        (class, port) => {
            return Err(bad(&format!("unsupported chapter value {class}:{port}")));
        }
    })
}

/// Whether a notification's client output drives a box trigger's Enable or
/// Disable through a CompareBool over Not/PropertyCast steps in its scene
/// (`scene`, its bus: layers share their scene's connections). Installed:
/// spawnpoints #17 (s4) -> cast #3 -> Not #13 -> CompareBool #12, whose
/// OnFalse enables and OnTrue disables RaceBoxTrigger #16.
fn drives_box<S: Source>(
    graph: &mut Graph<S>,
    scene: &str,
    notification: &Key,
) -> Result<bool, Error> {
    let nodes = graph.read(Site::root(scene))?;
    for node in nodes.iter().filter(|n| n.object.class == BOX) {
        for event in [BOX_ENABLE, BOX_DISABLE] {
            for (source, _) in graph.event_sources(&node.site, event)? {
                if graph.object(&source)?.class != "CompareBoolEntityData" {
                    continue;
                }
                let mut at = graph.input(&source, COMPARE_BOOL)?;
                for _ in 0..MAX_NODES {
                    let class = graph.object(&at.site)?.class;
                    let next = match class.as_str() {
                        "NotEntityData" => NOT_IN,
                        "PropertyCastEntityData" => {
                            let ports = graph.input_ports(&at.site)?;
                            let [port] = ports.as_slice() else {
                                break;
                            };
                            *port
                        }
                        _ => break,
                    };
                    at = graph.input(&at.site, next)?;
                }
                if at.site.key == *notification {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn default(object: &nfs_frostbite::ebx::Object) -> Result<i32, Error> {
    match object.fields.get("DefaultValue") {
        Some(Value::Bool(b)) => Ok(i32::from(*b)),
        Some(v) => Ok(bits32(v)? as i32),
        // An absent field holds the class default.
        None => Ok(0),
    }
}

impl AssetIndex {
    pub fn chapter_notifications(&mut self) -> Result<Vec<Notification>, Error> {
        let rules = self.identity.progression.clone();
        let stable_port = rules.ports["stable_id"];
        let gameplay = "levels/genesis01/gameplay";
        let mut bindings = std::collections::BTreeMap::new();
        let mut graph = Graph::new(&mut self.assets);
        let branch = graph.read(Site::root(&rules.intro))?;
        let scene = graph.read(Site::root(&rules.scene))?;
        let world = graph.read(Site::root(gameplay))?;
        // The progression sub-world's reference in the gameplay scene.
        let mut references = world.iter().filter(|n| {
            n.object.class == "SubWorldReferenceObjectData"
                && n.object
                    .fields
                    .get("BundleName")
                    .and_then(Value::as_str)
                    .is_some_and(|b| b.eq_ignore_ascii_case(&rules.scene))
        });
        let (Some(reference), None) = (references.next(), references.next()) else {
            return Err(bad("progression sub-world reference"));
        };
        let reference = reference.site.clone();
        let mut out: Vec<Found> = vec![];
        for node in branch.iter().filter(|n| {
            matches!(n.object.class.as_str(), INT_ACCESSOR | BOOL_ACCESSOR)
                && !n.site.parents.is_empty()
        }) {
            if !graph.input_ports(&node.site)?.contains(&ACCESSOR_VALUE) {
                continue;
            }
            let (kind, port) = if node.object.class == INT_ACCESSOR {
                (Kind::Int, INT_CHANNEL)
            } else {
                (Kind::Bool, BOOL_CHANNEL)
            };
            let channels = graph.links(&node.site, port)?;
            let [channel] = channels.as_slice() else {
                return Err(bad("chapter accessor channel"));
            };
            if graph.object(&channel.site)?.class != "LinkedSchematicChannelReferenceEntityData" {
                // A participant-logic-local property: no scene sees it.
                continue;
            }
            let asset = graph.target_field(&channel.site, "ChannelAsset")?;
            let source = graph.input(&node.site, ACCESSOR_VALUE)?;
            let value = expr(&mut graph, stable_port, source, 0)?;
            // The channel's instance in the progression scene forwards the
            // field through the scene interface.
            let mut instances = vec![];
            for n in scene
                .iter()
                .filter(|n| n.object.class == "LinkedSchematicChannelEntityData")
            {
                if graph.target_field(&n.site, "ChannelAsset")? == asset {
                    instances.push(n.site.clone());
                }
            }
            let [instance] = instances.as_slice() else {
                return Err(bad("chapter channel instance"));
            };
            for target in graph.link_targets(instance, channel.port)? {
                if graph.object(&target.site)?.class != "InterfaceDescriptorData" {
                    continue;
                }
                for property in graph.links(&reference, target.port)? {
                    let object = graph.object(&property.site)?;
                    if !object.class.ends_with("ParticipantPropertyEntityData") {
                        return Err(bad("chapter target property"));
                    }
                    let default = default(&object)?;
                    for source in graph.link_sources(&property.site)? {
                        if graph
                            .object(&source.site)?
                            .class
                            .ends_with("ClientNotificationEntityData")
                        {
                            if out.len() >= MAX_NOTIFICATIONS {
                                return Err(bad("chapter notification bound"));
                            }
                            out.push((
                                kind,
                                value.clone(),
                                source.site.bus.0.clone(),
                                source.site.key.clone(),
                                default,
                                (source.site.bus.0.clone(), source.site.key.clone()),
                            ));
                        }
                    }
                }
            }
        }
        let mut boxes = vec![];
        for (.., (scene, notification)) in &out {
            boxes.push(drives_box(&mut graph, scene, notification)?);
        }
        drop(graph);
        let mut result = vec![];
        for ((kind, value, scene, key, default, _), entry_box) in out.into_iter().zip(boxes) {
            if !bindings.contains_key(&scene) {
                let rows: std::collections::BTreeMap<(String, usize), usize> = self
                    .scene_bindings(&scene)?
                    .into_iter()
                    .map(|b| ((b.asset.to_lowercase(), b.object), b.serializer))
                    .collect();
                bindings.insert(scene.clone(), rows);
            }
            let serializer = *bindings[&scene]
                .get(&(key.0.to_lowercase(), key.1))
                .ok_or_else(|| bad("chapter notification serializer"))?;
            result.push(Notification {
                kind,
                value,
                scene_key: crate::assets::scene_key(&scene),
                serializer,
                default,
                entry_box,
            });
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
