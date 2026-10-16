// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Property declarations from modeled node contracts, not class-name heuristics.
//! A type inferred through a connection does not establish its producer behavior.
use super::{
    Calculation,
    bindings::{
        Layout,
        types::{Constraint, Endpoint},
    },
    events::{Composition, Identity},
    interfaces::Interface,
    realms::Realm,
};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::Object;
use nfs_world::scoring::{
    self as world,
    math::Type,
    nodes::{self, Definition},
    properties::{Kind, MAX_PORTS, Port},
    value::Value,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Declaration {
    pub inputs: Vec<(u32, Kind)>,
    pub outputs: Vec<(u32, Kind)>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub class: String,
    pub declaration: Declaration,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedLayout {
    pub layout: Layout,
    /// Explicit declarations at retained endpoints. Unbound declarations are
    /// omitted; they do not allocate private outputs or input defaults.
    pub constraints: Vec<Constraint>,
    /// None means insufficient evidence, not a default scalar type.
    pub kinds: Vec<Option<Kind>>,
}

fn scalar_kind(kind: Type) -> Kind {
    match kind {
        Type::Boolean => Kind::Boolean,
        Type::Integer => Kind::Integer,
        Type::Float => Kind::Float,
    }
}
fn value_kind(value: Value) -> Kind {
    match value {
        Value::Boolean(_) => Kind::Boolean,
        Value::Integer(_) => Kind::Integer,
        Value::Float(_) => Kind::Float,
        Value::Vec3(_) => Kind::Vec3,
        Value::Vec4(_) => Kind::Vec4,
        Value::Transform(_) => Kind::Transform,
    }
}

/// Read the known property subset. None denotes an unmodeled class. An empty
/// declaration denotes a modeled class with no properties in this subset.
/// Unknown fields of known classes are still undeclared, never guessed.
pub fn compile(object: &Object) -> Result<Option<Declaration>, Error> {
    let mut d = Declaration::default();
    match object.class.as_str() {
        "MathEntityData" | "CurveEntityData" => match super::compile(object)? {
            Calculation::Math { program, .. } => {
                d.inputs
                    .extend(program.inputs().map(|(p, k)| (p, scalar_kind(k))));
                d.outputs
                    .push((nodes::OUT, scalar_kind(program.output_type())));
            }
            Calculation::Curve { .. } => {
                d.inputs.push((nodes::INPUT, Kind::Float));
                d.outputs.push((nodes::OUTPUT, Kind::Float));
            }
        },
        "MathOpEntityData" | "VectorMathOpEntityData" => {
            match super::arithmetic::compile(object)? {
                super::arithmetic::Configuration::Arithmetic(operations) => {
                    d.inputs.push((world::arithmetic::IN0, Kind::Float));
                    for slot in 1..=operations.len() {
                        let port = world::boolean_fold::input_port(slot as u16)
                            .map_err(|e| bad(&e.to_string()))?;
                        d.inputs.push((port, Kind::Float));
                    }
                    d.outputs.push((nodes::OUT, Kind::Float));
                }
                super::arithmetic::Configuration::VectorLength => {
                    d.inputs.push((world::vector_length::INPUT, Kind::Vec3));
                    d.outputs.extend([
                        (world::vector_length::LENGTH, Kind::Float),
                        (world::vector_length::NORMAL, Kind::Vec3),
                    ]);
                }
            }
        }
        "AndEntityData" | "OrEntityData" => {
            let config = super::boolean_fold::compile(object)?;
            for slot in 1..=config.input_count {
                let port =
                    world::boolean_fold::input_port(slot).map_err(|e| bad(&e.to_string()))?;
                d.inputs.push((port, Kind::Boolean));
            }
            d.outputs.push((nodes::OUT, Kind::Boolean));
        }
        "SyncedBoolEntityData" | "SyncedFloatEntityData" | "SyncedIntEntityData" => {
            let kind = match super::synced::compile(object)? {
                world::synced::Kind::Boolean => Kind::Boolean,
                world::synced::Kind::Float => Kind::Float,
                world::synced::Kind::Integer => Kind::Integer,
            };
            d.inputs.push((nodes::IN, kind));
            d.outputs.push((nodes::OUT, kind));
        }
        "PropertyGateEntityData" => {
            super::property_gate::compile(object)?;
            for channel in world::property_gate::CHANNELS {
                d.inputs.push((channel.input(), channel.kind()));
                d.outputs.push((channel.output(), channel.kind()));
            }
        }
        "StopWatchEntityData" => {
            super::stopwatch::compile(object)?;
            d.inputs.extend([
                (world::stopwatch::MULTIPLIER, Kind::Float),
                (world::stopwatch::TRIGGER_TIME, Kind::Float),
            ]);
            d.outputs.push((world::stopwatch::TIME, Kind::Float));
        }
        "RaceVehicleRoadQueryEntityData" => {
            super::road::compile(object)?;
            d.inputs
                .push((world::road::SPEED_LIMIT_OVERRIDE, Kind::Float));
            d.outputs.push((world::road::SPEED_LIMIT, Kind::Float));
        }
        "RaceVehicleCollisionQueryEntityData" => {
            d.outputs.extend([
                (world::collision::QUERYING, Kind::Boolean),
                (world::collision::VALUE, Kind::Integer),
            ]);
        }
        "RaceVehicleNearMissQueryEntityData" => {
            d.outputs.extend(
                [0xad59_4576, 0x445f_266b, 0xd70a_7f0c, 0xd887_31d6].map(|p| (p, Kind::Float)),
            );
        }
        "RaceVehicleAccumulatingCollisionsQueryEntityData" => {
            d.outputs.push((1_977_761_241, Kind::Integer));
        }
        "PropertyCastEntityData" => {
            // The numeric subset only; string bindings remain undeclared.
            for channel in world::property_cast::CHANNELS {
                d.inputs.push((channel.input(), channel.kind()));
                d.outputs.push((channel.output(), channel.kind()));
            }
        }
        "ScoreEntityData" => {
            d.inputs.push((world::score::SCORE, Kind::Integer));
        }
        class if super::nodes::supported(class) => {
            let config = super::nodes::compile(object)?;
            d.inputs
                .extend(config.defaults.into_iter().map(|(p, v)| (p, value_kind(v))));
            let output = match config.definition {
                Definition::Cache => Some((nodes::CACHE_OUTPUT, Kind::Float)),
                Definition::IntegerCache => Some((nodes::CACHE_OUTPUT, Kind::Integer)),
                Definition::BooleanCache { .. } => Some((nodes::CACHE_OUTPUT, Kind::Boolean)),
                Definition::Accumulator | Definition::ConditionalFloat => {
                    Some((nodes::OUTPUT, Kind::Float))
                }
                Definition::Float { .. } => Some((nodes::FLOAT_VALUE, Kind::Float)),
                Definition::Boolean { .. } => Some((nodes::FLOAT_VALUE, Kind::Boolean)),
                Definition::Integer { .. } => Some((nodes::FLOAT_VALUE, Kind::Integer)),
                Definition::Gate { .. }
                | Definition::Splitter { .. }
                | Definition::Compare { .. }
                | Definition::CompareBoolean { .. } => None,
            };
            d.outputs.extend(output);
        }
        _ => return Ok(None),
    }
    // Multiple reads of one Math input are legitimate, but incompatible reads
    // cannot bind to the same property. Check even if that input is unconnected.
    for ports in [&mut d.inputs, &mut d.outputs] {
        let mut unique = BTreeMap::new();
        for &(port, kind) in ports.iter() {
            if unique.insert(port, kind).is_some_and(|prior| prior != kind) {
                return Err(bad("conflicting node property declaration"));
            }
        }
        *ports = unique.into_iter().collect();
    }
    Ok(Some(d))
}

impl AssetIndex {
    /// Read supported declarations, retaining prefab occurrence identity. This
    /// does not validate or implement classes absent from the returned list.
    pub fn scoring_property_declarations(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        let mut result = Vec::new();
        let mut count = 0usize;
        for node in Graph::new(&mut self.assets).read(Site::root(blueprint))? {
            if let Some(declaration) = compile(&node.object)? {
                count = count
                    .saturating_add(declaration.inputs.len())
                    .saturating_add(declaration.outputs.len());
                if count > MAX_PORTS {
                    return Err(bad("property declaration port bound"));
                }
                result.push(Node {
                    identity: Identity::from(&node.site),
                    class: node.object.class,
                    declaration,
                });
            }
        }
        Ok(result)
    }
}

impl Composition {
    /// Match declarations to this composition and infer connected cell types.
    /// Interface defaults constrain types only where their field is retained.
    /// Unknown endpoints remain visible through an unresolved cell or absence
    /// from constraints, even if another endpoint establishes that cell's type.
    pub fn typed_property_layout(
        &self,
        interfaces: &[Interface],
        realm: Realm,
        declarations: &[Node],
    ) -> Result<TypedLayout, Error> {
        if declarations.len() > self.nodes.len() {
            return Err(bad("property declaration node bound"));
        }
        let layout = self.property_layout(interfaces, realm)?;
        let mut constraints = Vec::new();
        let mut seen = BTreeSet::new();
        let mut count = 0usize;
        for node in declarations {
            let index = self
                .nodes
                .iter()
                .position(|n| n.identity == node.identity && n.class == node.class)
                .ok_or_else(|| bad("foreign property declaration"))? as u16;
            if !seen.insert(index) {
                return Err(bad("duplicate property declaration"));
            }
            count = count
                .saturating_add(node.declaration.inputs.len())
                .saturating_add(node.declaration.outputs.len());
            if count > MAX_PORTS {
                return Err(bad("property declaration port bound"));
            }
            for (ports, input) in [
                (&node.declaration.inputs, true),
                (&node.declaration.outputs, false),
            ] {
                let mut kinds = BTreeMap::new();
                for &(field, kind) in ports {
                    if kinds.insert(field, kind).is_some_and(|prior| prior != kind) {
                        return Err(bad("conflicting node property declaration"));
                    }
                    let port = Port { node: index, field };
                    let endpoint = if input {
                        Endpoint::Input(port)
                    } else {
                        Endpoint::Output(port)
                    };
                    add(&layout, &mut constraints, endpoint, kind);
                }
            }
        }
        for interface in interfaces {
            let index = self
                .nodes
                .iter()
                .position(|n| n.identity == interface.identity)
                .ok_or_else(|| bad("property interface identity"))? as u16;
            for field in &interface.fields {
                if let Some(value) = field.default {
                    // A retained descriptor port is an internal source. Source
                    // declarations without an output cell do not create one.
                    add(
                        &layout,
                        &mut constraints,
                        Endpoint::Output(Port {
                            node: index,
                            field: field.id,
                        }),
                        value_kind(value),
                    );
                }
            }
        }
        let kinds = layout.property_types(&constraints)?;
        Ok(TypedLayout {
            layout,
            constraints,
            kinds,
        })
    }
}
fn add(layout: &Layout, constraints: &mut Vec<Constraint>, endpoint: Endpoint, kind: Kind) {
    let retained = match endpoint {
        Endpoint::Input(port) => layout.inputs.contains_key(&port),
        Endpoint::Output(port) => layout.outputs.contains_key(&port),
    };
    if retained {
        constraints.push(Constraint { endpoint, kind });
    }
}

#[cfg(test)]
mod tests;
