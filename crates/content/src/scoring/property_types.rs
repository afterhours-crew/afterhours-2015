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
    graph::{Graph, Site, Source, bad},
};
use nfs_frostbite::ebx::{Object, Value as EbxValue};
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
        Type::Vec3 => Kind::Vec3,
    }
}
fn value_kind(value: Value) -> Kind {
    match value {
        Value::Text(_) => Kind::Text,
        Value::Boolean(_) => Kind::Boolean,
        Value::Integer(_) => Kind::Integer,
        Value::Unsigned32(_) => Kind::Unsigned32,
        Value::Unsigned64(_) => Kind::Unsigned64,
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
        "DNFGameModeStateTransitionEntityData" => d.inputs.push((0xe06c_b1a0, Kind::Float)),
        "SpawnAiForGameModeStateEntityData" => d.outputs.push((0x7d32_e2e7, Kind::Integer)),
        "DistanceReporterEntityData" => d.outputs.push((0xa52c_76ce, Kind::Float)),
        "GameModeEntityData" | "StateReporterEntityData" | "CompoundStateReporterEntityData" => {
            d.outputs.push((0xe6d6_7e59, Kind::Integer));
        }
        "RaceVehicleBurnoutQueryEntityData" => d.outputs.extend([
            (world::burnout::ACTIVE, Kind::Boolean),
            (world::burnout::COUNT, Kind::Integer),
            (world::burnout::TIME, Kind::Float),
        ]),
        "RaceVehicleBurnoutTendencyOverrideEntityData" => d.inputs.extend([
            (world::burnout_override::ENABLED, Kind::Boolean),
            (world::burnout_override::VALUE, Kind::Float),
        ]),
        "ItemInfoAccessorEntityData" => d.outputs.extend([
            (world::item_info::ITEM_ID, Kind::Unsigned64),
            (world::item_info::ITEM_NAME, Kind::Text),
            (world::item_info::TEXT, Kind::Text),
            (world::item_info::VALID, Kind::Boolean),
        ]),
        "RaceVehicleInfoQueryEntityData" => d.outputs.extend([
            (world::vehicle_info::BUNDLE, Kind::Unsigned32),
            (world::vehicle_info::BUNDLE_SIGNED, Kind::Integer),
            (world::vehicle_info::SHORT_NAME, Kind::Text),
        ]),
        "VehicleTuningQueryEntityData" => d
            .outputs
            .push((world::interpreter::VEHICLE_TUNING, Kind::Float)),
        "ParticipantContextReporterEntityData" => {
            super::boolean(&object.fields, "Active")?;
            d.inputs
                .push((world::interpreter::CONTEXT_ACTIVE, Kind::Boolean));
            d.outputs
                .push((world::interpreter::CONTEXT_MATCH, Kind::Boolean));
        }
        "ParticipantLogicParticipantTransitionEntityData"
        | "SetTeleportLocationEntityData"
        | "TriggeredParticipantTransitionEntityData"
        | "ParticipantEventChannelReferenceEntityData" => (),
        "CurveWrapperEntityData" => {
            super::scalar(&object.fields, "Input")?;
            d.inputs.push((nodes::INPUT, Kind::Float));
            d.outputs.push((nodes::OUTPUT, Kind::Float));
        }
        "EventDetailsInfoAccessorEntityData" => d.outputs.extend([
            (world::interpreter::EVENT_ID, Kind::Unsigned32),
            (world::interpreter::EVENT_ID_STRING, Kind::Text),
        ]),
        // Name is also published as a native CString on the server, but the
        // installed event consumers are client presentation/telemetry sinks.
        // Keep that port undeclared: a modeled string consumer must fail closed.
        "ParticipantNameProviderEntityData" => (),
        "ParticipantSetCounterEntityData" => d
            .outputs
            .push((world::participant_sets::COUNT, Kind::Integer)),
        "ParticipantSetAdapterEntityData" => (),
        "ParticipantSetPredicateEntityData" => {
            super::sets::predicate(object)?;
        }
        "IntParticipantPropertySorterQueryByIndexEntityData" => {
            super::rank_query::configuration(object)?;
            // The server serializes the selected participant and value; Name
            // and Value are client properties, not server graph outputs.
            d.inputs.push((world::rank_query::INDEX, Kind::Integer));
        }
        "GameIdProviderEntityData" => {
            // All normal-event compositions consume only GameId. The native
            // secondary Boolean (0bc3140c) depends on an unresolved experience
            // constant and remains undeclared: connecting it fails compilation.
            d.outputs
                .push((world::interpreter::GAME_ID, Kind::Unsigned64));
        }
        "BankTransactionEntityData" => {
            super::bank::configuration(object)?;
            d.inputs.push((world::bank::AMOUNT, Kind::Integer));
        }
        "ParticipantSetAccessorEntityData" => {
            d.outputs
                .push((world::participant_sets::CONTAINS, Kind::Boolean));
        }
        "RaceVehicleDraftQueryEntityData" => {
            super::draft::compile(object)?;
            d.inputs
                .extend(world::draft::INPUTS.map(|p| (p, Kind::Float)));
            d.outputs.extend(
                world::vehicle_query::Query::Draft
                    .outputs()
                    .iter()
                    .map(|o| (o.field, o.kind)),
            );
        }
        "RaceVehiclePhysicsQueryEntityData" | "RaceVehicleDriftQueryEntityData" => {
            let query = if object.class == "RaceVehiclePhysicsQueryEntityData" {
                world::vehicle_query::Query::Physics
            } else {
                world::vehicle_query::Query::Drift
            };
            d.outputs
                .extend(query.outputs().iter().map(|o| (o.field, o.kind)));
        }
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
        "SortAndRankIntParticipantPropertyEntityData" => {
            super::arithmetic::compile(object)?;
            d.outputs.extend([
                (world::integers::RANK, Kind::Integer),
                (world::integers::COUNT, Kind::Integer),
            ]);
        }
        "GotoWaypointCommandEntityData" => {
            d.inputs
                .push((world::ai_commands::LOCATION_TRANSFORM, Kind::Transform));
        }
        "MathOpEntityData"
        | "ScaleClampEntityData"
        | "StringEntityData"
        | "FloatSelectEntityData"
        | "RandomFloatEntityData"
        | "SwitchPropertyFloatEntityData"
        | "VectorMathOpEntityData"
        | "MathIntOpEntityData"
        | "IntHubEntityData"
        | "FloatHubEntityData"
        | "RankIntegersEntityData" => match super::arithmetic::compile(object)? {
            super::arithmetic::Configuration::ScaleClamp(_) => {
                d.inputs.extend(
                    world::scale_clamp::INPUTS
                        .into_iter()
                        .map(|port| (port, Kind::Float)),
                );
                d.outputs.push((world::scale_clamp::OUT, Kind::Float));
            }
            super::arithmetic::Configuration::StringConstant(_) => {
                d.outputs.push((0xc5e4_e950, Kind::Text));
            }
            super::arithmetic::Configuration::FloatSelect { .. } => {
                for port in [
                    world::float_select::POSITIVE,
                    world::float_select::NEGATIVE,
                    world::float_select::SELECT,
                ] {
                    d.inputs.push((port, Kind::Float));
                }
                d.outputs.push((world::float_select::OUT, Kind::Float));
            }
            super::arithmetic::Configuration::RandomFloat { .. } => {
                d.inputs.extend([
                    (world::random_float::MIN, Kind::Float),
                    (world::random_float::MAX, Kind::Float),
                ]);
                d.outputs.push((world::random_float::OUT, Kind::Float));
            }
            super::arithmetic::Configuration::ParticipantRank { .. } => {
                unreachable!("separate class arm")
            }
            super::arithmetic::Configuration::FloatSwitch(definition) => {
                for entry in definition.entries {
                    for field in [entry.property, entry.event] {
                        if !d.inputs.iter().any(|(p, _)| *p == field) {
                            d.inputs.push((field, Kind::Float));
                        }
                    }
                }
                d.outputs.push((world::float_switch::OUTPUT, Kind::Float));
            }
            super::arithmetic::Configuration::IntegerHub { count, .. } => {
                d.inputs.push((world::integer_hub::SELECT, Kind::Integer));
                for slot in 0..count {
                    d.inputs
                        .push((world::integers::port("In", slot), Kind::Integer));
                }
                d.outputs.push((nodes::OUT, Kind::Integer));
            }
            super::arithmetic::Configuration::FloatHub { count, .. } => {
                d.inputs.push((world::integer_hub::SELECT, Kind::Integer));
                for slot in 0..count {
                    d.inputs
                        .push((world::integers::port("In", slot), Kind::Float));
                }
                d.outputs.push((nodes::OUT, Kind::Float));
            }
            super::arithmetic::Configuration::IntegerArithmetic(operations) => {
                for slot in 0..=operations.len() {
                    d.inputs
                        .push((world::integers::port("In", slot as u16), Kind::Integer));
                }
                d.outputs.push((nodes::OUT, Kind::Integer));
            }
            super::arithmetic::Configuration::VectorDot => {
                d.inputs.extend([
                    (world::vector_dot::LEFT, Kind::Vec3),
                    (world::vector_dot::RIGHT, Kind::Vec3),
                ]);
                d.outputs.push((world::vector_dot::OUT, Kind::Float));
            }
            super::arithmetic::Configuration::IntegerSum(count) => {
                for slot in 0..count {
                    d.inputs
                        .push((world::integers::port("In", slot), Kind::Integer));
                }
                d.outputs.push((nodes::OUT, Kind::Integer));
            }
            super::arithmetic::Configuration::RankIntegers { count, .. } => {
                for slot in 0..count {
                    d.inputs
                        .push((world::integers::port("In", slot), Kind::Integer));
                    d.outputs
                        .push((world::integers::port("Out", slot), Kind::Integer));
                }
            }
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
        },
        "AndEntityData" | "OrEntityData" => {
            let config = super::boolean_fold::compile(object)?;
            for slot in 1..=config.input_count {
                let port =
                    world::boolean_fold::input_port(slot).map_err(|e| bad(&e.to_string()))?;
                d.inputs.push((port, Kind::Boolean));
            }
            d.outputs.push((nodes::OUT, Kind::Boolean));
        }
        "SyncedBoolEntityData"
        | "SyncedFloatEntityData"
        | "SyncedIntEntityData"
        | "SyncedStringEntityData" => {
            let kind = match super::synced::compile(object)? {
                world::synced::Kind::Boolean => Kind::Boolean,
                world::synced::Kind::Float => Kind::Float,
                world::synced::Kind::Integer => Kind::Integer,
                world::synced::Kind::Text => Kind::Text,
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
        "RaceTimeProviderEntityData" => d.outputs.push((world::stopwatch::TIME, Kind::Float)),
        "TransformSplitterEntityData" => {
            d.inputs.push((0x8752_5469, Kind::Transform));
            d.outputs.extend(
                [0x7c83_f1be, 0x0059_7380, 0x7667_213e, 0x0d8a_a67f]
                    .map(|field| (field, Kind::Vec3)),
            );
        }
        "RaceVehicleRoadDrivingQueryEntityData" => {
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
        "VehicleProximitySetFilterEntityData" => {
            super::proximity::compile(object)?;
            d.inputs.push((world::proximity::RADIUS, Kind::Float));
            d.outputs.push((world::proximity::COUNT, Kind::Integer));
        }
        "VehicleStatsProviderEntityData" => {
            d.outputs.extend([
                (world::stats::TOP_SPEED, Kind::Integer),
                (world::stats::STOCK_TOP_SPEED, Kind::Integer),
                (world::stats::ZERO_TO_SIXTY, Kind::Float),
                (world::stats::STOCK_ZERO_TO_SIXTY, Kind::Float),
            ]);
            d.outputs
                .extend(world::stats::UNPUBLISHED_FLOATS.map(|p| (p, Kind::Float)));
        }
        "RaceVehicleNosQueryEntityData" => {
            d.outputs.extend(
                [
                    world::nos::ENGAGED_LEVEL,
                    world::nos::DISENGAGED_LEVEL,
                    world::nos::CAPACITY,
                ]
                .map(|p| (p, Kind::Float)),
            );
        }
        "RaceVehicleNosBuildupEntityData" => {
            super::buildup::compile(object)?;
            d.inputs.extend(
                [
                    world::buildup::DRIFTING,
                    world::buildup::DRAFTING,
                    world::buildup::ONCOMING,
                    world::buildup::AIRBORNE,
                ]
                .map(|p| (p, Kind::Boolean)),
            );
            d.inputs
                .extend([world::buildup::SPEED, world::buildup::PACING].map(|p| (p, Kind::Float)));
        }
        "RaceVehicleDonutQueryEntityData" => {
            super::donut::compile(object)?;
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
                Definition::IntegerCache { .. } => Some((nodes::CACHE_OUTPUT, Kind::Integer)),
                Definition::BooleanCache { .. } => Some((nodes::CACHE_OUTPUT, Kind::Boolean)),
                Definition::Accumulator | Definition::ConditionalFloat => {
                    Some((nodes::OUTPUT, Kind::Float))
                }
                Definition::ConditionalInteger => Some((nodes::OUTPUT, Kind::Integer)),
                Definition::Float { .. } => Some((nodes::FLOAT_VALUE, Kind::Float)),
                Definition::Boolean { .. } => Some((nodes::FLOAT_VALUE, Kind::Boolean)),
                Definition::Integer { .. } => Some((nodes::FLOAT_VALUE, Kind::Integer)),
                Definition::ParticipantProperty { .. } => {
                    Some((nodes::OUTPUT_VALUE, Kind::Boolean))
                }
                Definition::IntegerParticipantProperty => {
                    Some((nodes::OUTPUT_VALUE, Kind::Integer))
                }
                Definition::FloatParticipantProperty => Some((nodes::OUTPUT_VALUE, Kind::Float)),
                Definition::Not => Some((nodes::OUT, Kind::Boolean)),
                Definition::Clamp { .. } => Some((nodes::OUT, Kind::Float)),
                Definition::Gate { .. }
                | Definition::AllEvents { .. }
                | Definition::Splitter { .. }
                | Definition::Compare { .. }
                | Definition::CompareIntegerRange { .. }
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
        let mut graph = Graph::new(&mut self.assets);
        for node in graph.read(Site::root(blueprint))? {
            let declaration = if matches!(
                node.object.class.as_str(),
                "LinkedSchematicChannelEntityData" | "LinkedSchematicChannelReferenceEntityData"
            ) {
                let key = graph.target_field(&node.site, "ChannelAsset")?;
                Some(channel_declaration(&graph.source.object(&key)?)?)
            } else {
                compile(&node.object)?
            };
            if let Some(declaration) = declaration {
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

/// A channel asset declares scalar types independently of runtime values and
/// connection direction. Unknown hashes remain untyped; they are not floats.
fn channel_declaration(object: &Object) -> Result<Declaration, Error> {
    if object.class != "SchematicChannelAsset" {
        return Err(bad("property channel asset class"));
    }
    let rows = super::field(&object.fields, "Properties")?
        .as_array()
        .filter(|r| r.len() <= MAX_PORTS)
        .ok_or_else(|| bad("property channel declarations"))?;
    let mut d = Declaration::default();
    let mut seen = BTreeSet::new();
    for row in rows {
        let fields = row
            .as_fields()
            .ok_or_else(|| bad("property channel declaration"))?;
        let id = super::bits(super::field(fields, "Id")?)?;
        if !seen.insert(id) {
            return Err(bad("duplicate property channel declaration"));
        }
        let realm = match super::field(fields, "Realm")? {
            EbxValue::Enum {
                value,
                name: Some(name),
            } => match (*value, name.as_str()) {
                (0, "Realm_Client") | (3, "Realm_None") => false,
                (1, "Realm_Server") | (2, "Realm_ClientAndServer") => true,
                _ => return Err(bad("property channel realm")),
            },
            _ => return Err(bad("property channel realm")),
        };
        // Native type-name hashes (djb2-XOR), as recorded in FieldTypeHash.
        let kind = match super::bits(super::field(fields, "FieldTypeHash")?)? {
            0x0cc9_7257 => Kind::Integer,    // Int32
            0x7f39_a7b4 => Kind::Float,      // Float32
            0x9638_b221 => Kind::Boolean,    // Boolean
            0x4d2d_e673 => Kind::Text,       // CString
            0xb0bc_3c22 => Kind::Unsigned32, // Uint32
            0xb0bc_3c81 => Kind::Unsigned64, // Uint64
            0x7c87_4d06 => Kind::Vec3,
            0x7c87_4d01 => Kind::Vec4,
            0xe33d_ff14 => Kind::Transform, // LinearTransform
            _ => continue,
        };
        if realm {
            d.inputs.push((id, kind));
            d.outputs.push((id, kind));
        }
    }
    Ok(d)
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
                if let Some(value) = field.default.clone() {
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
