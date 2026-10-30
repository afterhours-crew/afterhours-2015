// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed tutorials: the gameplay scene's tutorial spawners
//! (`layer34_tutorial_schematic`). Each spawner creates one tutorial logic
//! for the local participant while its predicate holds: the tutorial is not
//! seen, and, for gated tutorials, an intro-set IdentifiedBool is true or the
//! participant is inside the Club narrative location. The logic binds one
//! gameplay-scene channel serializer; the client accepts its prompt with the
//! logic's EventSync id (official E1026: 32 ungated logics at 34.631 s,
//! earningreputation at 891.324 s, accepted 893.235 s, deleted 893.323 s).
use crate::{Error, assets::AssetIndex, graph::*};
use nfs_frostbite::ebx::Value;
use std::collections::{BTreeMap, BTreeSet};
mod distance;
pub mod gates;
pub use distance::Distance;

const GAMEPLAY: &str = "levels/genesis01/gameplay";
const LAYER: &str = "levels/genesis01/gameplay/layer34_tutorial_schematic";
const SPAWNER: &str = "ParticipantLogicSpawnerEntityData";
const SET_SPAWNER: &str = "ParticipantLogicSpawnerSetControlledEntityData";
const PREDICATE: &str = "BooleanOperationPredicateEntityData";
const PROPERTY: &str = "BoolParticipantPropertyEntityData";
const IDENTIFIED: &str = "IdentifiedBoolEntityData";
const NOTIFICATION: &str = "BoolPropertyClientNotificationEntityData";
const SET_PREDICATE: &str = "ParticipantSetPredicateEntityData";
const SET_FROM_PROPERTY: &str = "ParticipantSetFromBPPEntityData";
const VOLUME_SET: &str = "VolumeParticipantSetFilterEntityData";
/// Spawner link to the channel instance its logic binds.
const SPAWNER_CHANNEL: u32 = 2_522_390_424;
/// Operand links of a predicate (operand 0, operand 1).
const OPERANDS: [u32; 2] = [177_557, 177_556];
/// IdentifiedBool link to the gate property it feeds.
const IDENTIFIED_TARGET: u32 = 3_959_795_656;
const REPORTER: &str = "ParticipantContextReporterEntityData";
/// CompareBool input property and its change outputs.
const COMPARE_BOOL: u32 = 2_088_812_747;
const ON_TRUE: u32 = 2_861_454_674;
const ON_FALSE: u32 = 4_213_085_657;
const MAX_ROWS: usize = 16;
const MAX_TUTORIALS: usize = 64;
const MAX_DEPTH: usize = 8;
/// Spawner link to its predicate.
#[cfg(test)]
const SPAWNER_PREDICATE: u32 = 2_802_306_239;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Gate {
    None,
    /// An intro-set per-participant IdentifiedBool (its asset name).
    Identified(String),
    /// Inside a narrative location (participant-set intersection with the
    /// location's volume set; installed: the Club, narrativelocations s24).
    Location(Volume),
}

/// A volume participant-set filter by scene and serializer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Volume {
    pub scene_key: u32,
    pub serializer: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tutorial {
    /// The tutorial logic blueprint.
    pub blueprint: String,
    /// Gameplay scene serializer of the logic's channel.
    pub channel: usize,
    /// Durable identity: the seen property's layer object.
    pub seen: usize,
    pub gate: Gate,
    /// Gameplay scene serializer of the seen value's client notification.
    pub notification: Option<usize>,
    /// The client's accept: the logic's EventSync id (Flags & 0x1FFFFFF).
    pub accept: Option<u32>,
    /// A server context reporter's networked prompt rows.
    pub context: Option<Context>,
    /// A server driven-distance check (accelerate).
    pub distance: Option<Distance>,
}

/// A server `ParticipantContextReporter` feeding a server CompareBool that
/// triggers on change: true while the participant is held by a mode with any
/// of `categories`. Its networked rows (bus, event-table ordinal) fire on the
/// change to true or to false (authoredevents: [3] / [0], E1026 574.121 s).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Context {
    pub categories: BTreeSet<u32>,
    pub on_true: Vec<(u32, u32)>,
    pub on_false: Vec<(u32, u32)>,
}

enum Expr {
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Seen(usize),
    Gate(String),
    Location(Box<Expr>, Site),
}

fn class<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<String, Error> {
    Ok(graph.object(site)?.class)
}

/// The one immediate link target (tutorial links stay inside the layer).
fn single<S: Source>(graph: &mut Graph<S>, site: &Site, port: u32) -> Result<Site, Error> {
    let links = graph.link_targets(site, port)?;
    let [one] = links.as_slice() else {
        return Err(bad("tutorial link count"));
    };
    Ok(one.site.clone())
}

fn expr<S: Source>(graph: &mut Graph<S>, site: &Site, depth: usize) -> Result<Expr, Error> {
    if depth > MAX_DEPTH {
        return Err(bad("tutorial predicate depth"));
    }
    let object = graph.object(site)?;
    match object.class.as_str() {
        PREDICATE => {
            let operation = object.fields.get("Operation").and_then(|v| match v {
                Value::Enum { name, .. } => name.clone(),
                Value::String(s) => Some(s.clone()),
                _ => None,
            });
            match operation.as_deref() {
                Some(op) if op.ends_with("NOT") => {
                    let operand = single(graph, site, OPERANDS[0])?;
                    Ok(Expr::Not(Box::new(expr(graph, &operand, depth + 1)?)))
                }
                Some(op) if op.ends_with("AND") => {
                    let a = single(graph, site, OPERANDS[0])?;
                    let b = single(graph, site, OPERANDS[1])?;
                    Ok(Expr::And(
                        Box::new(expr(graph, &a, depth + 1)?),
                        Box::new(expr(graph, &b, depth + 1)?),
                    ))
                }
                _ => Err(bad("unsupported tutorial predicate operation")),
            }
        }
        PROPERTY => {
            let mut gate = None;
            for source in graph.link_sources(site)? {
                if source.port == IDENTIFIED_TARGET
                    && class(graph, &source.site)? == IDENTIFIED
                    && gate
                        .replace(graph.target_field(&source.site, "BoolIdentifier")?.0)
                        .is_some()
                {
                    return Err(bad("tutorial gate sources"));
                }
            }
            Ok(match gate {
                Some(asset) => Expr::Gate(asset),
                None => Expr::Seen(site.key.1),
            })
        }
        SET_PREDICATE => {
            // INTERSECTION(narrative location set, set from NOT(seen)).
            let mut seen = None;
            let mut volume = None;
            for port in OPERANDS {
                for link in graph.link_targets(site, port)? {
                    if class(graph, &link.site)? != SET_FROM_PROPERTY {
                        // The location's set through the sub-world interface.
                        let sets = graph.links(site, port)?;
                        let [set] = sets.as_slice() else {
                            return Err(bad("tutorial location set link"));
                        };
                        if class(graph, &set.site)? != VOLUME_SET
                            || volume.replace(set.site.clone()).is_some()
                        {
                            return Err(bad("tutorial location volume"));
                        }
                        continue;
                    }
                    if class(graph, &link.site)? == SET_FROM_PROPERTY {
                        let inner = graph
                            .link_ports(&link.site)?
                            .into_iter()
                            .map(|p| single(graph, &link.site, p))
                            .collect::<Result<Vec<_>, _>>()?;
                        let [inner] = inner.as_slice() else {
                            return Err(bad("tutorial location set source"));
                        };
                        if seen.replace(expr(graph, inner, depth + 1)?).is_some() {
                            return Err(bad("tutorial location set count"));
                        }
                    }
                }
            }
            Ok(Expr::Location(
                Box::new(seen.ok_or_else(|| bad("tutorial location set"))?),
                volume.ok_or_else(|| bad("tutorial location volume"))?,
            ))
        }
        _ => Err(bad("unsupported tutorial predicate node")),
    }
}

enum Shape {
    Plain(Gate),
    Location(Site),
}

/// (seen property, gate) of the installed predicate shapes.
fn shape(expr: Expr) -> Result<(usize, Shape), Error> {
    let plain = |(seen, gate)| (seen, Shape::Plain(gate));
    Ok(match expr {
        Expr::Location(inner, site) => match inner_shape(*inner)? {
            (seen, Gate::None) => (seen, Shape::Location(site)),
            _ => return Err(bad("tutorial predicate shape")),
        },
        other => plain(inner_shape(other)?),
    })
}

fn inner_shape(expr: Expr) -> Result<(usize, Gate), Error> {
    match expr {
        Expr::Not(inner) => match *inner {
            Expr::Seen(seen) => Ok((seen, Gate::None)),
            _ => Err(bad("tutorial predicate shape")),
        },
        Expr::And(a, b) => match (*a, *b) {
            (Expr::Not(s), Expr::Gate(g)) | (Expr::Gate(g), Expr::Not(s)) => match *s {
                Expr::Seen(seen) => Ok((seen, Gate::Identified(g))),
                _ => Err(bad("tutorial predicate shape")),
            },
            _ => Err(bad("tutorial predicate shape")),
        },
        _ => Err(bad("tutorial predicate shape")),
    }
}

impl AssetIndex {
    pub fn tutorials(&mut self) -> Result<Vec<Tutorial>, Error> {
        self.tutorials_in(GAMEPLAY, LAYER)
    }
    /// The spawners of `layer`, a layer of `scene` (the scene itself in
    /// constructed fixtures, whose writer exports one object per partition).
    fn tutorials_in(&mut self, scene: &str, layer: &str) -> Result<Vec<Tutorial>, Error> {
        let bindings: BTreeMap<(String, usize), usize> = self
            .scene_bindings(scene)?
            .into_iter()
            .map(|b| ((b.asset.to_lowercase(), b.object), b.serializer))
            .collect();
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(scene))?;
        let mut out = vec![];
        let mut locations = vec![];
        for node in nodes.iter().filter(|n| {
            matches!(n.object.class.as_str(), SPAWNER | SET_SPAWNER)
                && n.site.key.0.eq_ignore_ascii_case(layer)
        }) {
            if out.len() >= MAX_TUTORIALS {
                return Err(bad("tutorial bound"));
            }
            let blueprint = graph
                .target_field(&node.site, "ParticipantLogicBlueprint")?
                .0
                .to_lowercase();
            // The logic's own channel instance; other links of this port
            // feed further channel fields of the logic (cooldown: 3 links).
            let mut own = vec![];
            for link in graph.link_targets(&node.site, SPAWNER_CHANNEL)? {
                if link.site.key.0.eq_ignore_ascii_case(layer)
                    && class(&mut graph, &link.site)? == "LinkedSchematicChannelEntityData"
                {
                    own.push(link.site);
                }
            }
            let [channel_site] = own.as_slice() else {
                return Err(bad("tutorial channel count"));
            };
            let channel = *bindings
                .get(&(channel_site.key.0.to_lowercase(), channel_site.key.1))
                .ok_or_else(|| bad("tutorial channel serializer"))?;
            let predicates: Vec<u32> = graph
                .link_ports(&node.site)?
                .into_iter()
                .filter(|p| *p != SPAWNER_CHANNEL)
                .collect();
            let [port] = predicates.as_slice() else {
                return Err(bad("tutorial spawner predicate count"));
            };
            let predicate = single(&mut graph, &node.site, *port)?;
            let (seen, shape) = shape(expr(&mut graph, &predicate, 0)?)?;
            if (node.object.class == SET_SPAWNER) != matches!(shape, Shape::Location(_)) {
                return Err(bad("tutorial spawner kind"));
            }
            let gate = match shape {
                Shape::Plain(gate) => gate,
                Shape::Location(site) => {
                    locations.push((out.len(), site));
                    Gate::Location(Volume {
                        scene_key: 0,
                        serializer: 0,
                    })
                }
            };
            let seen_site = Site {
                key: (node.site.key.0.clone(), seen),
                ..node.site.clone()
            };
            let mut notification = None;
            for source in graph.link_sources(&seen_site)? {
                if class(&mut graph, &source.site)? == NOTIFICATION {
                    let serializer = *bindings
                        .get(&(source.site.key.0.to_lowercase(), source.site.key.1))
                        .ok_or_else(|| bad("tutorial notification serializer"))?;
                    if notification.replace(serializer).is_some() {
                        return Err(bad("tutorial notification count"));
                    }
                }
            }
            out.push(Tutorial {
                blueprint,
                channel,
                seen,
                gate,
                notification,
                accept: None,
                context: None,
                distance: None,
            });
        }
        let mut volumes = vec![];
        for (index, site) in locations {
            volumes.push((index, location_volume(&mut graph, &site)?));
        }
        drop(graph);
        for (index, (scene, nth)) in volumes {
            let mut bound = self
                .scene_bindings(&scene)?
                .into_iter()
                .filter(|b| b.class == VOLUME_SET);
            let serializer = bound
                .nth(nth)
                .ok_or_else(|| bad("tutorial location serializer"))?
                .serializer;
            out[index].gate = Gate::Location(Volume {
                scene_key: crate::assets::scene_key(&scene),
                serializer,
            });
        }
        // A notification reports one seen value.
        let mut notified = std::collections::BTreeSet::new();
        if !out
            .iter()
            .filter_map(|t| t.notification)
            .all(|n| notified.insert(n))
        {
            return Err(bad("tutorial notification shared"));
        }
        for tutorial in &mut out {
            let network = self.network_events(&tutorial.blueprint)?;
            let mut graph = Graph::new(&mut self.assets);
            let nodes = graph.read(Site::root(&tutorial.blueprint))?;
            tutorial.context = context(&mut graph, &nodes, &network)?;
            tutorial.distance = distance::distance(&mut graph, &nodes, &network)?;
            let mut accept = nodes
                .iter()
                .filter(|n| n.object.class == "EventSyncEntityData")
                .map(|n| match n.object.fields.get("Flags") {
                    Some(v) => bits32(v).map(|f| f & 0x1ff_ffff),
                    None => Err(bad("tutorial event sync flags")),
                });
            tutorial.accept = accept.next().transpose()?;
            if accept.next().is_some() {
                return Err(bad("tutorial event sync count"));
            }
        }
        Ok(out)
    }
}

fn server(object: &nfs_frostbite::ebx::Object) -> bool {
    matches!(object.fields.get("Realm"),
        Some(Value::Enum { name: Some(n), .. }) if n == "Realm_Server")
}

/// The logic's server context reporter, when a server CompareBool on it has
/// networked rows.
fn context<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    network: &[crate::logic_events::Event],
) -> Result<Option<Context>, Error> {
    let mut found = None;
    for compare in nodes
        .iter()
        .filter(|n| n.object.class == "CompareBoolEntityData" && server(&n.object))
    {
        let sources = graph.property_sources(&compare.site, COMPARE_BOOL)?;
        let [source] = sources.as_slice() else {
            continue;
        };
        let reporter = graph.object(&source.site)?;
        if reporter.class != REPORTER || !server(&reporter) {
            continue;
        }
        let rows = |port| -> Vec<(u32, u32)> {
            network
                .iter()
                .filter(|e| {
                    e.source.index == compare.site.key.1
                        && e.source.asset.eq_ignore_ascii_case(&compare.site.key.0)
                        && e.source.port == port
                })
                .map(|e| (e.bus, e.ordinal))
                .collect()
        };
        let (on_true, on_false) = (rows(ON_TRUE), rows(ON_FALSE));
        if on_true.is_empty() && on_false.is_empty() {
            continue;
        }
        if on_true.len() + on_false.len() > MAX_ROWS {
            return Err(bad("tutorial context row bound"));
        }
        let categories = crate::events::intro::results::categories(graph, &source.site)?;
        if categories.is_empty()
            || found
                .replace(Context {
                    categories,
                    on_true,
                    on_false,
                })
                .is_some()
        {
            return Err(bad("tutorial context reporter"));
        }
    }
    Ok(found)
}

/// The scene of a location volume reached through a sub-world reference,
/// and its instance's position among that scene's volume filters (scene
/// bindings follow construction order).
fn location_volume<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<(String, usize), Error> {
    let world = site
        .parents
        .first()
        .ok_or_else(|| bad("tutorial location scene"))?
        .reference
        .clone();
    let object = graph.source.object(&world)?;
    if object.class != "SubWorldReferenceObjectData" {
        return Err(bad("tutorial location reference"));
    }
    let scene = object
        .fields
        .get("BundleName")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 512)
        .ok_or_else(|| bad("tutorial location scene name"))?
        .to_lowercase();
    let tail = &site.parents[1..];
    let nodes = graph.read(Site::root(&scene))?;
    let filters: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.object.class == VOLUME_SET)
        .collect();
    let position = filters
        .iter()
        .position(|n| {
            n.site.key == site.key
                && n.site.parents.len() == tail.len()
                && n.site
                    .parents
                    .iter()
                    .zip(tail)
                    .all(|(a, b)| a.reference == b.reference)
        })
        .ok_or_else(|| bad("tutorial location instance"))?;
    Ok((scene, position))
}

#[cfg(test)]
mod tests;
