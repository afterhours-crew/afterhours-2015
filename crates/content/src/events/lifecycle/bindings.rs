// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored lifecycle observers and participant-logic spawners. Identities
//! include prefab occurrences; these objects have no replication serializer.
//! Do not infer their lifetime from a blueprint name or a global race phase.
use super::*;
use crate::scoring::{events::Identity, participant_properties::identifier};
use nfs_world::participant_states::properties::Definition;

const LIMIT: usize = 512;
pub mod ai;
pub mod distance;
pub mod mirrors;
pub mod results;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateProperty {
    pub identity: Identity,
    pub definition: Definition<Endpoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Link {
    pub identity: Identity,
    pub port: u32,
    pub class: String,
    pub channel: Option<Property>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Controller {
    Boolean(Link),
    Set(Link),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spawner {
    pub identity: Identity,
    pub blueprint: String,
    /// Occurrence among spawners of this blueprint, matching Layout.occurrence.
    pub occurrence: usize,
    /// Native activation tolerates an unconnected controller and creates none.
    pub controller: Option<Controller>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateContext {
    pub identity: Identity,
    pub states: Vec<Endpoint>,
    pub manager: Property,
    pub categories: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateReporter {
    pub identity: Identity,
    pub compound: bool,
    /// Distinct sources in authored order. Counts sum memberships, not the
    /// union of participants (native 046aaea0/046abf10).
    pub states: Vec<Endpoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum IntegerSource {
    Local(Identity),
    Channel(Property),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegerSorter {
    pub identity: Identity,
    pub source: IntegerSource,
    pub order: nfs_world::scoring::rank_sorter::Order,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SetSource {
    Local(Identity),
    Channel(Property),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetKind {
    Owned,
    States(Vec<Endpoint>),
    Boolean(Link),
    Players {
        manager: Property,
    },
    Predicate {
        operation: nfs_world::scoring::participant_sets::Operation,
        operands: Vec<SetSource>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParticipantSet {
    pub identity: Identity,
    pub kind: SetKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetCounter {
    pub identity: Identity,
    pub source: Option<SetSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RankQuery {
    pub identity: Identity,
    pub sorter: Identity,
    pub index: i32,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Bindings {
    pub channels: Vec<super::channels::Channel>,
    pub set_counters: Vec<SetCounter>,
    pub rank_queries: Vec<RankQuery>,
    pub ai_controls: Vec<ai::Control>,
    pub ai_commands: BTreeMap<Identity, ai::Command>,
    /// Owned game-clock channel used by countdown states and DNF transitions;
    /// distinct from the process receipt clock used by finish arbitration.
    pub clocks: BTreeMap<usize, Property>,
    pub update_managers: Vec<UpdateManager>,
    pub predicates: Vec<Predicate>,
    /// Activation order in the composed entity list. Indices address the
    /// corresponding vectors; transitions use their serializer identity.
    pub order: Vec<Observer>,
    pub properties: Vec<StateProperty>,
    pub contexts: Vec<StateContext>,
    pub reporters: Vec<StateReporter>,
    pub integers: Vec<Identity>,
    pub mirror_managers: Vec<Identity>,
    pub mirrors: Vec<mirrors::Mirror>,
    pub mirror_providers: Vec<mirrors::Provider>,
    pub distances: Vec<distance::Definition>,
    pub sorters: Vec<IntegerSorter>,
    pub results: Vec<results::Definition>,
    pub spawners: Vec<Spawner>,
    pub sets: Vec<ParticipantSet>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateManager {
    pub identity: Identity,
    pub sets: Vec<Identity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Observer {
    SetCounter(usize),
    Ai(usize),
    Distance(usize),
    Mirror(usize),
    MirrorProvider(usize),
    Predicate(usize),
    Property(usize),
    Context(usize),
    Reporter(usize),
    Sorter(usize),
    Set(usize),
    Spawner(usize),
    Transition(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BooleanOperation {
    And,
    Or,
    Not,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredicateKind {
    Boolean {
        operation: BooleanOperation,
        operands: Vec<Link>,
    },
    Triggered,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Predicate {
    pub identity: Identity,
    pub kind: PredicateKind,
}

fn set_class(class: &str) -> bool {
    matches!(
        class,
        "ParticipantSetEntityData"
            | "ParticipantSetStateAdapterEntityData"
            | "ParticipantSetBoolPropertyAdapterEntityData"
            | "ParticipantSetPlayerAdapterEntityData"
            | "ParticipantSetPredicateEntityData"
    )
}

pub(super) fn set_source<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
) -> Result<SetSource, Error> {
    let link =
        controller(graph, site, port)?.ok_or_else(|| bad("lifecycle set operand missing"))?;
    if link.port == 0 && set_class(&link.class) {
        Ok(SetSource::Local(link.identity))
    } else if link.port != 0 && link.class == "LinkedSchematicChannelReferenceEntityData" {
        Ok(SetSource::Channel(identifier(graph, site, port)?))
    } else {
        Err(bad("lifecycle set operand class"))
    }
}

pub(super) fn controller<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
) -> Result<Option<Link>, Error> {
    let links = graph.links(site, port)?;
    match links.as_slice() {
        [] => Ok(None),
        [link] => {
            let class = graph.object(&link.site)?.class;
            let channel = if link.port != 0 && class == "LinkedSchematicChannelReferenceEntityData"
            {
                Some(identifier(graph, site, port)?)
            } else {
                None
            };
            Ok(Some(Link {
                identity: Identity::from(&link.site),
                port: link.port,
                class,
                channel,
            }))
        }
        _ => Err(bad("participant logic controller count")),
    }
}

pub(super) fn integer_source<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
) -> Result<Option<IntegerSource>, Error> {
    let Some(link) = controller(graph, site, port)? else {
        return Ok(None);
    };
    if let Some(channel) = link.channel {
        Ok(Some(IntegerSource::Channel(channel)))
    } else if link.port == 0
        && matches!(
            link.class.as_str(),
            "IntParticipantPropertyEntityData" | "IntParticipantPropertyMirrorEntityData"
        )
    {
        Ok(Some(IntegerSource::Local(link.identity)))
    } else {
        Err(bad("integer property source class"))
    }
}

pub(super) fn extract<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    fields: &[FieldSite],
) -> Result<Bindings, Error> {
    let index: BTreeMap<_, _> = fields
        .iter()
        .enumerate()
        .map(|(i, (s, _, _))| (s.clone(), i))
        .collect();
    let mut result = Bindings::default();
    let mut occurrences = BTreeMap::<String, usize>::new();
    for node in nodes {
        let identity = Identity::from(&node.site);
        match node.object.class.as_str() {
            "ParticipantSetCounterEntityData" => {
                let source = if graph.links(&node.site, 0x1224_af09)?.is_empty() {
                    None
                } else {
                    Some(set_source(graph, &node.site, 0x1224_af09)?)
                };
                result
                    .order
                    .push(Observer::SetCounter(result.set_counters.len()));
                result.set_counters.push(SetCounter { identity, source });
            }
            "IntParticipantPropertySorterQueryByIndexEntityData" => {
                let links = graph.links(&node.site, 0x970a_d7f8)?;
                if let [link] = links.as_slice()
                    && link.port == 0
                    && graph.object(&link.site)?.class == "IntParticipantPropertySorterEntityData"
                {
                    result.rank_queries.push(RankQuery {
                        identity,
                        sorter: Identity::from(&link.site),
                        index: crate::scoring::rank_query::configuration(&node.object)?,
                    });
                }
            }
            "ControlAiInParticipantStateEntityData" | "ControlAiInGameModeStateEntityData" => {
                if result.ai_controls.len() >= LIMIT {
                    return Err(bad("event AI control bound"));
                }
                result.order.push(Observer::Ai(result.ai_controls.len()));
                result
                    .ai_controls
                    .push(ai::control(graph, node, &index, fields)?);
            }
            "WaypointChannelGotoWaypointCommandEntityData"
            | "WaypointSeriesPacingGroupCommandEntityData"
            | "WanderCommandEntityData" => {
                if result.ai_commands.len() >= LIMIT {
                    return Err(bad("event AI command bound"));
                }
                result
                    .ai_commands
                    .insert(identity, ai::command(graph, node, &index, fields)?);
            }
            "DistanceReporterEntityData" => {
                result
                    .order
                    .push(Observer::Distance(result.distances.len()));
                result
                    .distances
                    .push(distance::read(graph, node, &index, fields)?);
            }
            "MirroringManagerEntityData" => {
                result.mirror_managers.push(identity);
            }
            "IntParticipantPropertyMirrorProviderEntityData" => {
                if result.mirror_providers.len() >= LIMIT {
                    return Err(bad("integer mirror provider bound"));
                }
                result
                    .order
                    .push(Observer::MirrorProvider(result.mirror_providers.len()));
                result
                    .mirror_providers
                    .push(mirrors::provider(graph, node)?);
            }
            "ResultEntityData" => {
                if result.results.len() >= 64 {
                    return Err(bad("result entity bound"));
                }
                result.results.push(results::read(graph, node)?);
            }
            "IntParticipantPropertyEntityData" | "IntParticipantPropertyMirrorEntityData" => {
                if result.integers.len() >= LIMIT {
                    return Err(bad("integer property bound"));
                }
                result.integers.push(identity);
                if node.object.class == "IntParticipantPropertyMirrorEntityData" {
                    result.order.push(Observer::Mirror(result.mirrors.len()));
                    result.mirrors.push(mirrors::read(graph, node)?);
                }
            }
            "IntParticipantPropertySorterEntityData" => {
                use nfs_world::scoring::rank_sorter::Order;
                let order = match node.object.fields.get("SortOrder") {
                    Some(Value::Enum {
                        value: 0,
                        name: Some(n),
                    }) if n == "Descending" => Order::Descending,
                    Some(Value::Enum {
                        value: 1,
                        name: Some(n),
                    }) if n == "Ascending" => Order::Ascending,
                    _ => return Err(bad("integer sorter order")),
                };
                let source = integer_source(graph, &node.site, 0x0df2_30d5)?
                    .ok_or_else(|| bad("integer sorter source missing"))?;
                result.order.push(Observer::Sorter(result.sorters.len()));
                result.sorters.push(IntegerSorter {
                    identity,
                    source,
                    order,
                });
            }
            "StateReporterEntityData" | "CompoundStatesReporterEntityData" => {
                let compound = node.object.class == "CompoundStatesReporterEntityData";
                let mut states = endpoints(
                    graph,
                    &node.site,
                    if compound { 0xc5df_0f41 } else { 0x0dc0_f0f2 },
                    &index,
                    fields,
                )?;
                if states.len() > if compound { MAX_STATES } else { 1 } {
                    return Err(bad("state reporter source count"));
                }
                let mut seen = BTreeSet::new();
                states.retain(|s| seen.insert(*s));
                result
                    .order
                    .push(Observer::Reporter(result.reporters.len()));
                result.reporters.push(StateReporter {
                    identity,
                    compound,
                    states,
                });
            }
            "BooleanOperationPredicateEntityData" | "TriggeredPredicateEntityData" => {
                let kind = if node.object.class == "TriggeredPredicateEntityData" {
                    PredicateKind::Triggered
                } else {
                    let Some(Value::Int32(count)) = node.object.fields.get("NumberOfOperands")
                    else {
                        return Err(bad("Boolean predicate operand count"));
                    };
                    if !(0..=32).contains(count) {
                        return Err(bad("Boolean predicate operand bound"));
                    }
                    let operation = match node.object.fields.get("Operation") {
                        Some(Value::Enum {
                            value: 0,
                            name: Some(n),
                        }) if n == "AND" => BooleanOperation::And,
                        Some(Value::Enum {
                            value: 1,
                            name: Some(n),
                        }) if n == "OR" => BooleanOperation::Or,
                        Some(Value::Enum {
                            value: 2,
                            name: Some(n),
                        }) if n == "NOT" => BooleanOperation::Not,
                        _ => return Err(bad("Boolean predicate operation")),
                    };
                    let mut operands = Vec::new();
                    for i in 0..*count {
                        operands.push(
                            controller(
                                graph,
                                &node.site,
                                nfs_world::scoring::integers::port("", i as u16),
                            )?
                            .ok_or_else(|| bad("Boolean predicate operand missing"))?,
                        );
                    }
                    PredicateKind::Boolean {
                        operation,
                        operands,
                    }
                };
                result
                    .order
                    .push(Observer::Predicate(result.predicates.len()));
                result.predicates.push(Predicate { identity, kind });
            }
            "UpdateableManagerEntityData" => {
                let links = graph.links(&node.site, 0x7644_d2cd)?;
                if links.len() > LIMIT {
                    return Err(bad("update manager set bound"));
                }
                let mut sets = Vec::new();
                for link in links {
                    if link.port != 0 || graph.object(&link.site)?.class != "UpdateSetEntityData" {
                        return Err(bad("update manager set class"));
                    }
                    sets.push(Identity::from(&link.site));
                }
                if result.update_managers.len() >= LIMIT {
                    return Err(bad("update manager bound"));
                }
                result
                    .update_managers
                    .push(UpdateManager { identity, sets });
            }
            class if transition_kind(class).is_some() => {
                let serializer = *index
                    .get(&node.site)
                    .ok_or_else(|| bad("transition observer serializer"))?;
                result.order.push(Observer::Transition(serializer));
            }
            class if set_class(class) => {
                let kind = match class {
                    "ParticipantSetEntityData" => SetKind::Owned,
                    "ParticipantSetStateAdapterEntityData" => {
                        let states = endpoints(graph, &node.site, 0x9e22_81be, &index, fields)?;
                        if states.len() > MAX_STATES {
                            return Err(bad("state set list bound"));
                        }
                        SetKind::States(states)
                    }
                    "ParticipantSetBoolPropertyAdapterEntityData" => SetKind::Boolean(
                        controller(graph, &node.site, 0x969a_1637)?
                            .ok_or_else(|| bad("Boolean set property missing"))?,
                    ),
                    "ParticipantSetPlayerAdapterEntityData" => SetKind::Players {
                        manager: identifier(graph, &node.site, 0x965e_d189)?,
                    },
                    "ParticipantSetPredicateEntityData" => {
                        let (operation, count) = crate::scoring::sets::predicate(&node.object)?;
                        let mut operands = Vec::new();
                        for i in 0..count {
                            operands.push(set_source(
                                graph,
                                &node.site,
                                nfs_world::scoring::integers::port("", i as u16),
                            )?);
                        }
                        SetKind::Predicate {
                            operation,
                            operands,
                        }
                    }
                    _ => unreachable!(),
                };
                result.order.push(Observer::Set(result.sets.len()));
                result.sets.push(ParticipantSet { identity, kind });
            }
            "ParticipantStateListBoolParticipantPropertyEntityData" => {
                let states = endpoints(graph, &node.site, 0x9e22_81be, &index, fields)?;
                if states.len() > MAX_STATES {
                    return Err(bad("state property list bound"));
                }
                result
                    .order
                    .push(Observer::Property(result.properties.len()));
                result.properties.push(StateProperty {
                    identity,
                    definition: Definition::List(states),
                });
            }
            "ParticipantStateRangeBoolParticipantPropertyEntityData" => {
                let begins = endpoints(graph, &node.site, 0xff47_6cc8, &index, fields)?;
                let ends = endpoints(graph, &node.site, 0x68d2_51b8, &index, fields)?;
                let ([begin], [end]) = (begins.as_slice(), ends.as_slice()) else {
                    return Err(bad("state property range endpoints"));
                };
                result
                    .order
                    .push(Observer::Property(result.properties.len()));
                result.properties.push(StateProperty {
                    identity,
                    definition: Definition::Range {
                        begin: *begin,
                        end: *end,
                    },
                });
            }
            "ParticipantStateParticipantContextEntityData" => {
                let states = endpoints(graph, &node.site, 0xc5df_0f41, &index, fields)?;
                if states.len() > MAX_STATES {
                    return Err(bad("state context list bound"));
                }
                let rows = node
                    .object
                    .fields
                    .get("Categories")
                    .and_then(Value::as_array)
                    .filter(|r| r.len() <= 128)
                    .ok_or_else(|| bad("state context categories"))?;
                let mut categories = Vec::new();
                for row in rows {
                    let key = graph
                        .source
                        .target(
                            &node.site.key.0,
                            row.as_pointer()
                                .ok_or_else(|| bad("state context category pointer"))?,
                        )?
                        .ok_or_else(|| bad("state context null category"))?;
                    let category = graph.source.object(&key)?;
                    if category.class != "GameModeCategory" {
                        return Err(bad("state context category class"));
                    }
                    categories.push(bits32(
                        category
                            .fields
                            .get("CategoryHash")
                            .ok_or_else(|| bad("state context category hash"))?,
                    )?);
                }
                result.order.push(Observer::Context(result.contexts.len()));
                result.contexts.push(StateContext {
                    identity,
                    states,
                    manager: identifier(graph, &node.site, 0xb611_c196)?,
                    categories,
                });
            }
            class @ ("ParticipantLogicSpawnerEntityData"
            | "ParticipantLogicSpawnerSetControlledEntityData") => {
                let key = graph.target_field(&node.site, "ParticipantLogicBlueprint")?;
                if graph.source.object(&key)?.class != "ParticipantLogicBlueprint" {
                    return Err(bad("participant logic spawner blueprint class"));
                }
                let occurrence = occurrences.entry(key.0.clone()).or_default();
                let source = if class == "ParticipantLogicSpawnerEntityData" {
                    controller(graph, &node.site, 0xa707_ccbf)?.map(Controller::Boolean)
                } else {
                    controller(graph, &node.site, 0xb075_bd0b)?.map(Controller::Set)
                };
                result.order.push(Observer::Spawner(result.spawners.len()));
                result.spawners.push(Spawner {
                    identity,
                    blueprint: key.0,
                    occurrence: *occurrence,
                    controller: source,
                });
                *occurrence += 1;
            }
            _ => {}
        }
        if result.properties.len()
            + result.set_counters.len()
            + result.rank_queries.len()
            + result.ai_controls.len()
            + result.predicates.len()
            + result.contexts.len()
            + result.reporters.len()
            + result.sorters.len()
            + result.mirrors.len()
            + result.mirror_providers.len()
            + result.distances.len()
            + result.spawners.len()
            + result.sets.len()
            > LIMIT
        {
            return Err(bad("lifecycle observer bound"));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
