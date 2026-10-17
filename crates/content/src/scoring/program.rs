// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compile one installed scoring composition into an executable `Program` for
//! the server realm. This joins the strict class readers with the typed
//! property layout and derives the verified startup order. It does not run the
//! graph, award REP or enable a participant.
use super::{
    Calculation, arithmetic, awards, bindings, boolean_fold, creation, delay, donut, draft,
    events::{Composition, Identity},
    interfaces::{Access, Interface},
    nodes, property_gate, property_types, proximity, realms, road, stopwatch, synced,
};
use crate::{Error, assets::AssetIndex, graph::bad};
use nfs_world::scoring::{
    self as world,
    program::{self, Behavior, Keeper, Node, Program, Step, Unmodeled},
    properties::{DefaultMode, Definition, Kind, Port, Registration},
    property_cast,
    value::Value,
    vehicle_query::{Bindings as QueryBindings, Query},
};
use std::collections::{BTreeMap, BTreeSet};

/// Classes created on the server whose behavior is not modeled. Their outputs
/// stay unready, so consumers fall back to authored values; the runtime logs
/// them as unsupported. Adding a class here is a documented decision, not a
/// silent default.
pub const UNSUPPORTED: &[&str] = &[
    "RaceVehicleNosBuildupEntityData",
    "RaceVehicleSlamAParticipantCollisionEvaluatorEntityData",
    "ParticipantSetCounterEntityData",
    "PlayerInfoAccessorEntityData",
    "TelemetryHookEntityData",
    "PropertyEncodeEntityData",
    "LinkedSchematicChannelEntityData",
    "LinkedSchematicChannelReferenceEntityData",
    "LinkedSchematicChannelRelayEntityData",
    "AchievementEntityData",
];
/// Created on the server but with no behavior of their own in this graph.
pub const INERT: &[&str] = &[
    "ParticipantLogicEntityData",
    "ParticipantLogicControllableProviderEntityData",
    "ParticipantLogicParticipantProviderEntityData",
    "LogicPrefabReferenceObjectData",
];

/// Everything the compiler reads from the installed composition. Tests supply
/// constructed values; `AssetIndex::scoring_program` fills it from the game.
#[derive(Clone, Debug, PartialEq)]
pub struct Sources {
    pub composition: Composition,
    pub interfaces: Vec<Interface>,
    pub declarations: Vec<property_types::Node>,
    pub creation: Vec<creation::Node>,
    pub scalars: Vec<nodes::Node>,
    pub calculations: Vec<super::Node>,
    pub arithmetic: Vec<arithmetic::Node>,
    pub folds: Vec<boolean_fold::Node>,
    pub gates: Vec<property_gate::Node>,
    pub delays: Vec<delay::Node>,
    pub stopwatches: Vec<stopwatch::Node>,
    pub synced: Vec<synced::Node>,
    pub awards: Vec<awards::Award>,
    pub roads: Vec<road::Node>,
    pub drafts: Vec<draft::Node>,
    pub donuts: Vec<donut::Node>,
    pub proximities: Vec<proximity::Node>,
}

fn identity(identity: &Identity) -> program::Identity {
    program::Identity {
        asset: identity.asset.clone(),
        index: identity.index as u32,
        instances: identity
            .instances
            .iter()
            .map(|(asset, index)| (asset.clone(), *index as u32))
            .collect(),
    }
}

struct Index<'a> {
    composition: &'a Composition,
    by_identity: BTreeMap<&'a Identity, u16>,
}
impl Index<'_> {
    fn lookup(&self, identity: &Identity) -> Result<u16, Error> {
        self.by_identity.get(identity).copied().ok_or_else(|| {
            bad(&format!(
                "foreign scoring node {}:{}",
                identity.asset, identity.index
            ))
        })
    }
    fn node(&self, identity: &Identity, class: &str) -> Result<u16, Error> {
        let node = self.lookup(identity)?;
        if self.composition.nodes[usize::from(node)].class != class {
            return Err(bad("scoring node class mismatch"));
        }
        Ok(node)
    }
}

struct Ports {
    inputs: Vec<Vec<u32>>,
    outputs: Vec<Vec<u32>>,
}
impl Ports {
    fn new(nodes: usize, layout: &bindings::Layout) -> Self {
        let mut inputs = vec![Vec::new(); nodes];
        let mut outputs = vec![Vec::new(); nodes];
        for port in layout.inputs.keys() {
            inputs[usize::from(port.node)].push(port.field);
        }
        for port in layout.outputs.keys() {
            outputs[usize::from(port.node)].push(port.field);
        }
        Self { inputs, outputs }
    }
    fn has_output(&self, node: u16, field: u32) -> bool {
        self.outputs[usize::from(node)].contains(&field)
    }
}

fn slots(fields: &[u32], count: u16, zero: Option<u32>) -> Result<Vec<u16>, Error> {
    let mut slots = Vec::with_capacity(fields.len());
    for &field in fields {
        if Some(field) == zero {
            slots.push(0);
            continue;
        }
        let slot = (1..=count)
            .find(|&slot| world::boolean_fold::input_port(slot).ok() == Some(field))
            .ok_or_else(|| bad("unknown numbered input port"))?;
        slots.push(slot);
    }
    Ok(slots)
}

fn assign(behaviors: &mut [Option<Behavior>], node: u16, behavior: Behavior) -> Result<(), Error> {
    if behaviors[usize::from(node)].replace(behavior).is_some() {
        return Err(bad("scoring node compiled twice"));
    }
    Ok(())
}

fn lift(e: world::Error) -> Error {
    bad(&e.to_string())
}

/// Compile the server realm. Every server-created class must be modeled, inert
/// or explicitly unsupported; an unknown class fails. Untyped cells are kept
/// out of the Store only when no modeled node touches them, and are reported.
pub fn compile(sources: &Sources) -> Result<Program, Error> {
    let realm = realms::Realm::Server;
    let composition = &sources.composition;
    let typed =
        composition.typed_property_layout(&sources.interfaces, realm, &sources.declarations)?;
    let index = Index {
        composition,
        by_identity: composition
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (&n.identity, i as u16))
            .collect(),
    };
    let count = composition.nodes.len();
    let ports = Ports::new(count, &typed.layout);
    let mut behaviors: Vec<Option<Behavior>> = vec![None; count];
    let mut defaults: Vec<Vec<(u32, Value)>> = vec![Vec::new(); count];
    for n in &sources.scalars {
        let id = index.lookup(&n.identity)?;
        defaults[usize::from(id)].extend(n.configuration.defaults.iter().copied());
        assign(
            &mut behaviors,
            id,
            Behavior::Scalar(n.configuration.definition),
        )?;
    }
    for n in &sources.calculations {
        let identity = Identity {
            asset: n.asset.clone(),
            index: n.index,
            instances: n.instances.clone(),
        };
        let behavior = match &n.calculation {
            Calculation::Math {
                program,
                evaluate_on_event,
            } => Behavior::Math {
                program: program.clone(),
                evaluate_on_event: *evaluate_on_event,
            },
            Calculation::Curve {
                curve,
                apply,
                default_input,
            } => Behavior::Curve {
                curve: curve.clone(),
                apply: *apply,
                default_input: *default_input,
            },
        };
        let class = match behavior {
            Behavior::Math { .. } => "MathEntityData",
            _ => "CurveEntityData",
        };
        let id = index.node(&identity, class)?;
        if let Behavior::Curve { default_input, .. } = behavior {
            defaults[usize::from(id)].push((world::nodes::INPUT, Value::Float(default_input)));
        }
        assign(&mut behaviors, id, behavior)?;
    }
    for n in &sources.arithmetic {
        let id = index.lookup(&n.identity)?;
        let behavior = match &n.configuration {
            arithmetic::Configuration::Arithmetic(operations) => Behavior::Arithmetic {
                operations: operations.clone(),
                connected: slots(
                    &ports.inputs[usize::from(id)],
                    operations.len() as u16,
                    Some(world::arithmetic::IN0),
                )?,
            },
            arithmetic::Configuration::VectorLength => Behavior::VectorLength {
                length_bound: ports.has_output(id, world::vector_length::LENGTH),
                normal_bound: ports.has_output(id, world::vector_length::NORMAL),
            },
        };
        assign(&mut behaviors, id, behavior)?;
    }
    for n in &sources.folds {
        let id = index.lookup(&n.identity)?;
        let connected = slots(
            &ports.inputs[usize::from(id)],
            n.configuration.input_count,
            None,
        )?;
        assign(
            &mut behaviors,
            id,
            Behavior::Fold {
                all: n.configuration.all,
                input_count: n.configuration.input_count,
                connected,
            },
        )?;
    }
    for (i, n) in composition.nodes.iter().enumerate() {
        if n.class == "PropertyCastEntityData" {
            let bindings = property_cast::Bindings::new(&ports.inputs[i], &ports.outputs[i])
                .map_err(|_| bad("unsupported property cast binding"))?;
            assign(&mut behaviors, i as u16, Behavior::Cast(bindings))?;
        }
    }
    for n in &sources.gates {
        let id = index.lookup(&n.identity)?;
        let outputs = world::property_gate::CHANNELS
            .into_iter()
            .filter(|c| ports.has_output(id, c.output()))
            .collect();
        for (channel, value) in world::property_gate::CHANNELS
            .into_iter()
            .zip(n.configuration.defaults)
        {
            defaults[usize::from(id)].push((channel.input(), value));
        }
        assign(
            &mut behaviors,
            id,
            Behavior::Gate {
                definition: n.configuration.definition,
                outputs,
            },
        )?;
    }
    for n in &sources.delays {
        let id = index.lookup(&n.identity)?;
        defaults[usize::from(id)].push((world::delay::DELAY, Value::Float(n.configuration.delay)));
        assign(
            &mut behaviors,
            id,
            Behavior::Delay(n.configuration.definition),
        )?;
    }
    for n in &sources.stopwatches {
        let id = index.lookup(&n.identity)?;
        defaults[usize::from(id)].extend([
            (
                world::stopwatch::MULTIPLIER,
                Value::Float(n.configuration.multiplier),
            ),
            (
                world::stopwatch::TRIGGER_TIME,
                Value::Float(n.configuration.trigger_time),
            ),
        ]);
        assign(
            &mut behaviors,
            id,
            Behavior::Stopwatch {
                time_bound: ports.has_output(id, world::stopwatch::TIME),
            },
        )?;
    }
    for n in &sources.synced {
        let id = index.lookup(&n.identity)?;
        assign(&mut behaviors, id, Behavior::Synced(n.kind))?;
    }
    for award in &sources.awards {
        let score = index.node(&award.identity, "ScoreEntityData")?;
        let context = index.node(&award.context.identity, "ScoreContextEntityData")?;
        let provider = index.node(
            &award.context.participant_provider,
            "ParticipantLogicParticipantProviderEntityData",
        )?;
        let keepers = award
            .context
            .keepers
            .iter()
            .map(|k| {
                Ok(Keeper {
                    node: index.node(&k.identity, "LinkedSchematicChannelReferenceEntityData")?,
                    field: k.field,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        assign(
            &mut behaviors,
            score,
            Behavior::Score {
                default: award.default,
                context,
            },
        )?;
        let expected = Behavior::ScoreContext {
            participant_provider: provider,
            keepers,
        };
        match &behaviors[usize::from(context)] {
            None => behaviors[usize::from(context)] = Some(expected),
            Some(existing) if *existing == expected => {}
            Some(_) => return Err(bad("score context shared with different keepers")),
        }
    }
    for n in &sources.roads {
        let id = index.lookup(&n.identity)?;
        assign(
            &mut behaviors,
            id,
            Behavior::Road {
                configuration: n.configuration,
                limit_bound: ports.has_output(id, world::road::SPEED_LIMIT),
            },
        )?;
    }
    let mut drafts = BTreeMap::new();
    for n in &sources.drafts {
        drafts.insert(index.lookup(&n.identity)?, n.configuration);
    }
    let mut donuts = BTreeMap::new();
    for n in &sources.donuts {
        donuts.insert(index.lookup(&n.identity)?, n.configuration);
    }
    let mut proximities = BTreeMap::new();
    for n in &sources.proximities {
        proximities.insert(index.lookup(&n.identity)?, n.radius);
    }
    let eligible: BTreeMap<&Identity, bool> = sources
        .creation
        .iter()
        .map(|n| (&n.identity, n.realm.includes_server()))
        .collect();
    let mut node_list = Vec::with_capacity(count);
    for (i, n) in composition.nodes.iter().enumerate() {
        let id = i as u16;
        let created = eligible.get(&n.identity).copied().unwrap_or(false);
        let behavior = match behaviors[i].take() {
            Some(behavior) => behavior,
            None if !created => Behavior::Inert,
            None => match n.class.as_str() {
                "RaceVehiclePhysicsQueryEntityData"
                | "RaceVehicleDriftQueryEntityData"
                | "RaceVehicleDraftQueryEntityData" => {
                    let query = match n.class.as_str() {
                        "RaceVehiclePhysicsQueryEntityData" => Query::Physics,
                        "RaceVehicleDriftQueryEntityData" => Query::Drift,
                        _ => Query::Draft,
                    };
                    if query == Query::Draft {
                        let config = drafts
                            .get(&id)
                            .ok_or_else(|| bad("draft configuration missing"))?;
                        let values = [
                            config.min_other_speed,
                            config.max_delta_speed,
                            config.cone_scale,
                            config.min_draft_time,
                            config.max_draft_time,
                        ];
                        defaults[i].extend(
                            world::draft::INPUTS
                                .into_iter()
                                .zip(values)
                                .map(|(port, v)| (port, Value::Float(v))),
                        );
                    }
                    // Equal-value suppression on every bound output is the
                    // observed native condition for these scalar and vector
                    // property descriptors (E907/E908 evidence).
                    let outputs: Vec<_> = ports.outputs[i].iter().map(|f| (*f, true)).collect();
                    Behavior::VehicleQuery(QueryBindings::new(query, &outputs).map_err(lift)?)
                }
                "RaceVehicleCollisionQueryEntityData" => {
                    Behavior::Collision(world::collision::Outputs {
                        querying: ports.has_output(id, world::collision::QUERYING),
                        value: ports.has_output(id, world::collision::VALUE),
                        suppress_equal_querying: true,
                    })
                }
                "RaceVehicleNearMissQueryEntityData" => Behavior::NearMiss,
                "VehicleProximitySetFilterEntityData" => {
                    let radius = proximities
                        .get(&id)
                        .ok_or_else(|| bad("proximity radius missing"))?;
                    defaults[i].push((world::proximity::RADIUS, Value::Float(*radius)));
                    Behavior::Proximity
                }
                "RaceVehicleDonutQueryEntityData" => Behavior::Donut(
                    *donuts
                        .get(&id)
                        .ok_or_else(|| bad("donut configuration missing"))?,
                ),
                "VehicleStatsProviderEntityData" => Behavior::Stats,
                "RaceVehicleNosQueryEntityData" => Behavior::Nos,
                "RaceVehicleAccumulatingCollisionsQueryEntityData" => {
                    Behavior::AccumulatedCollisions
                }
                class if INERT.contains(&class) => Behavior::Inert,
                class if UNSUPPORTED.contains(&class) => Behavior::Unsupported,
                class => return Err(bad(&format!("unsupported scoring class {class}"))),
            },
        };
        let listeners = n.listeners.unwrap_or(realms::Listeners {
            events: realms::Realm::Neither,
            properties: realms::Realm::Neither,
        });
        node_list.push(Node {
            identity: identity(&n.identity),
            class: n.class.clone(),
            created,
            events: created && listeners.events.includes_server(),
            properties: created && listeners.properties.includes_server(),
            forward: n.forward,
            defaults: std::mem::take(&mut defaults[i]),
            behavior,
        });
    }
    let layout = &typed.layout;
    let mut endpoints: Vec<Vec<Port>> = vec![Vec::new(); layout.cells.len()];
    for (port, &cell) in layout.outputs.iter().chain(&layout.inputs) {
        endpoints
            .get_mut(cell)
            .ok_or_else(|| bad("property cell index"))?
            .push(*port);
    }
    let mut cell_map = vec![None; layout.cells.len()];
    let mut cells = Vec::new();
    let mut unmodeled = Vec::new();
    for (cell, kind) in typed.kinds.iter().enumerate() {
        let Some(kind) = kind else {
            for port in &endpoints[cell] {
                let node = &node_list[usize::from(port.node)];
                if !matches!(node.behavior, Behavior::Inert | Behavior::Unsupported) {
                    return Err(bad("untyped scoring cell reaches a modeled node"));
                }
                unmodeled.push(Unmodeled {
                    port: *port,
                    class: node.class.clone(),
                });
            }
            continue;
        };
        let mut seen = BTreeSet::new();
        cell_map[cell] = Some(cells.len());
        cells.push(Definition {
            kind: *kind,
            initial: None,
            ports: endpoints[cell]
                .iter()
                .copied()
                .filter(|p| seen.insert(*p))
                .collect(),
            listeners: Vec::new(),
        });
    }
    // A synchronized node owns its Out cell even when nothing consumes it:
    // the value is replicated regardless of local consumers (E901).
    for (i, node) in node_list.iter().enumerate() {
        if let Behavior::Synced(kind) = node.behavior
            && node.created
            && !ports.has_output(i as u16, world::synced::OUT)
        {
            cells.push(Definition {
                kind: kind.property_kind(),
                initial: None,
                ports: vec![Port {
                    node: i as u16,
                    field: world::synced::OUT,
                }],
                listeners: Vec::new(),
            });
        }
    }
    let mut all_ports = BTreeSet::new();
    for cell in &cells {
        for port in &cell.ports {
            if !all_ports.insert(*port) {
                return Err(bad("distinct input/output cells share one store port"));
            }
        }
    }
    let kept = |port: &Port| -> bool {
        layout
            .outputs
            .get(port)
            .or_else(|| layout.inputs.get(port))
            .is_some_and(|c| cell_map[*c].is_some())
    };
    let startup = startup(
        composition,
        &sources.interfaces,
        layout,
        &node_list,
        &kept,
        &typed.kinds,
    )?;
    let program = Program {
        nodes: node_list,
        cells,
        connections: composition.connections.clone(),
        startup,
        unmodeled,
    };
    program.validate().map_err(lift)?;
    Ok(program)
}

/// Native order: a bus is constructed when its blueprint object is reached
/// (merge forwarders and interface defaults), each created entity then binds
/// and registers its sorted input listeners, and activation follows creation
/// in insertion order once every object exists.
fn startup(
    composition: &Composition,
    interfaces: &[Interface],
    layout: &bindings::Layout,
    nodes: &[Node],
    kept: &dyn Fn(&Port) -> bool,
    kinds: &[Option<Kind>],
) -> Result<Vec<Step>, Error> {
    let mut steps = Vec::new();
    let bus_of = |n: &super::events::Node| {
        composition
            .buses
            .iter()
            .position(|b| b.asset == n.identity.asset && b.instances == n.identity.instances)
    };
    let is_bus = |n: &super::events::Node| composition.buses.contains(&n.identity);
    let mut constructed = BTreeSet::new();
    for (i, n) in composition.nodes.iter().enumerate() {
        let id = i as u16;
        if is_bus(n) {
            let bus = composition
                .buses
                .iter()
                .position(|b| *b == n.identity)
                .ok_or_else(|| bad("scoring bus identity"))?;
            if !constructed.insert(bus) {
                return Err(bad("scoring bus constructed twice"));
            }
            let plan = layout
                .plan
                .buses
                .get(bus)
                .ok_or_else(|| bad("scoring bus plan missing"))?;
            for input in &plan.inputs {
                if input.producers.len() < 2 || !kept(&input.target) {
                    continue;
                }
                for producer in &input.producers {
                    if !kept(producer) {
                        return Err(bad("merge producer without a typed cell"));
                    }
                    steps.push(Step::Register(Registration::Forward {
                        source: *producer,
                        target: input.target,
                    }));
                    steps.push(Step::CopyReady {
                        source: *producer,
                        target: input.target,
                    });
                }
            }
            if let Some(interface) = plan.interface {
                let descriptor = interfaces
                    .iter()
                    .find(|d| composition.nodes[usize::from(interface)].identity == d.identity)
                    .ok_or_else(|| bad("interface declaration missing"))?;
                for field in &descriptor.fields {
                    let (Some(default), Access::Target) = (field.default, field.access) else {
                        continue;
                    };
                    let port = Port {
                        node: interface,
                        field: field.id,
                    };
                    let Some(&cell) = layout.outputs.get(&port) else {
                        continue;
                    };
                    if !kept(&port) {
                        continue;
                    }
                    let value = match (kinds[cell], default) {
                        (Some(Kind::Boolean), v @ Value::Boolean(_))
                        | (Some(Kind::Float), v @ Value::Float(_))
                        | (Some(Kind::Integer), v @ Value::Integer(_)) => v,
                        _ => return Err(bad("interface default type mismatch")),
                    };
                    steps.push(Step::Default {
                        port,
                        value,
                        mode: DefaultMode::IfUnready,
                    });
                }
            }
            continue;
        }
        let node = &nodes[i];
        if !node.created {
            continue;
        }
        steps.push(Step::Bind(id));
        // A prefab reference has no property callback of its own: its inputs
        // are aliased by the child bus, whose entities register listeners.
        if !node.properties || node.forward.is_some() {
            continue;
        }
        let bus = bus_of(n).ok_or_else(|| bad("scoring node outside every bus"))?;
        let mut inputs: Vec<&bindings::Input> = layout.plan.buses[bus]
            .inputs
            .iter()
            .filter(|input| input.target.node == id)
            .collect();
        inputs.sort_by_key(|input| input.target.field as i32);
        for input in inputs {
            // Mode bit 0 (authored Flags 0x10) binds the input without a listener.
            if input.mode & 1 != 0 || !kept(&input.target) {
                continue;
            }
            steps.push(Step::Register(Registration::Notify(input.target)));
        }
    }
    for (i, node) in nodes.iter().enumerate() {
        if node.created && !is_bus(&composition.nodes[i]) {
            steps.push(Step::Activate(i as u16));
        }
    }
    if steps.len() > program::MAX_STEPS {
        return Err(bad("scoring startup bound"));
    }
    Ok(steps)
}

impl AssetIndex {
    /// Read every installed input of one composition without compiling it.
    pub fn scoring_sources(&mut self, blueprint: &str) -> Result<Sources, Error> {
        Ok(Sources {
            composition: self.scoring_events(blueprint)?,
            interfaces: self.scoring_interfaces(blueprint)?,
            declarations: self.scoring_property_declarations(blueprint)?,
            creation: self.scoring_creation_realms(blueprint)?,
            scalars: self.scoring_nodes(blueprint)?,
            calculations: self.scoring_calculations(blueprint)?,
            arithmetic: self.scoring_arithmetic(blueprint)?,
            folds: self.scoring_boolean_folds(blueprint)?,
            gates: self.scoring_property_gates(blueprint)?,
            delays: self.scoring_delays(blueprint)?,
            stopwatches: self.scoring_stopwatches(blueprint)?,
            synced: self.scoring_synchronized(blueprint)?,
            awards: self.scoring_awards(blueprint)?,
            roads: self.scoring_roads(blueprint)?,
            drafts: self.scoring_drafts(blueprint)?,
            donuts: self.scoring_donuts(blueprint)?,
            proximities: self.scoring_proximities(blueprint)?,
        })
    }

    /// Compile the server realm of one composition. See [`compile`] for the
    /// contract; this adds no behavior.
    pub fn scoring_program(&mut self, blueprint: &str) -> Result<Program, Error> {
        compile(&self.scoring_sources(blueprint)?)
    }
}

#[cfg(test)]
pub(crate) mod tests;
