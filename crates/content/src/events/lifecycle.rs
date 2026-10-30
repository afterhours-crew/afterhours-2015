// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored participant-state topology. Endpoints retain serializer identity and
//! channel ports; a channel is not mistaken for a state in the same prefab.
//! Native state configuration and transition wiring. Execution and external
//! manager lifetimes belong to the event host.
use super::*;

pub mod bindings;
pub mod channels;
pub mod program;
pub mod scenes;
pub mod shared;

const INITIAL: u32 = 0x1852_1aed;
const FINAL: u32 = 0x7b7a_ffc2;
const ROOT_STATE: u32 = 0x9759_e5bd;
const STATES: u32 = 0x8fe7_512c;
const TRANSITIONS: u32 = 0xbb14_0b43;
const SOURCE: u32 = 0xf02f_c8dc;
const TARGET: u32 = 0x856e_2e03;
pub const MAX_STATES: usize = 256;
pub const MAX_TRANSITIONS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StateKind {
    Participant,
    Updateable,
    GameMode,
    Simple,
    Countdown,
    Waypoints,
    WaitForAi,
    Sequence,
    /// This scene owns membership, while its service supplies entry/exit hooks
    /// and completion. Registering it must not run a game-mode default hook.
    HostOwned,
    /// Membership alias for a state whose behavior belongs to another scene.
    External,
}
fn state_kind(class: &str) -> Option<StateKind> {
    Some(match class {
        "ParticipantStateEntityData" => StateKind::Participant,
        "UpdateableParticipantStateEntityData" => StateKind::Updateable,
        "GameModeStateEntityData" => StateKind::GameMode,
        "SimpleGameModeStateEntityData" => StateKind::Simple,
        "CountdownGameModeStateEntityData" => StateKind::Countdown,
        "WaypointsGameModeStateEntityData" => StateKind::Waypoints,
        "WaitForAllAiToSpawnStateEntityData" => StateKind::WaitForAi,
        "ParticipantSequenceSpawnerGameModeStateEntityData" => StateKind::Sequence,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TransitionKind {
    Completed,
    Triggered,
    FinishLine,
    Dnf,
    DnfTriggered,
    AiFilter,
}
fn transition_kind(class: &str) -> Option<TransitionKind> {
    Some(match class {
        "ParticipantTransitionEntityData" => TransitionKind::Completed,
        "TriggeredParticipantTransitionEntityData" => TransitionKind::Triggered,
        "FinishLineGameModeStateTransitionEntityData" => TransitionKind::FinishLine,
        "DNFGameModeStateTransitionEntityData" => TransitionKind::Dnf,
        "DNFTriggeredGameModeStateTransitionEntityData" => TransitionKind::DnfTriggered,
        "AiParticipantFilterTransitionEntityData" => TransitionKind::AiFilter,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StateConfiguration {
    Default,
    Updateable {
        update_set: crate::scoring::events::Identity,
    },
    Countdown {
        sequence: (String, usize),
        seconds: f32,
    },
    Sequence {
        sequence: (String, usize),
    },
    Waypoints {
        allowed_wrong_direction_distance: f32,
    },
    WaitForAi {
        manager: Property,
    },
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub serializer: usize,
    pub kind: StateKind,
    pub configuration: StateConfiguration,
    pub handles_marked_for_delete: bool,
    pub send_rpc_to_client: bool,
}
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct Endpoint {
    pub serializer: usize,
    /// Zero for a state, or the authored port of a linked schematic channel.
    pub port: u32,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TransitionConfiguration {
    Default,
    FinishLine {
        laps: u8,
    },
    Dnf {
        timeout_seconds: f32,
    },
    /// Native Target is used for AI. The separate HumanTarget link is selected
    /// for a participant absent from the authored AI manager (046e9a40).
    AiFilter {
        manager: Property,
        human_target: Option<Endpoint>,
    },
}

pub(super) fn resolve_configurations<S: Source>(
    lifecycle: &mut Lifecycle,
    graph: &mut Graph<S>,
    fields: &[FieldSite],
    channels: &BTreeMap<Site, (Site, Binding)>,
) -> Result<(), Error> {
    use super::values::{Evaluator, Number, name};
    let values = channels
        .iter()
        .map(|(k, (v, _))| (k.clone(), v.clone()))
        .collect();
    for transition in &mut lifecycle.transitions {
        let site = &fields[transition.serializer].0;
        if transition.kind == TransitionKind::Dnf {
            lifecycle.bindings.clocks.insert(
                transition.serializer,
                crate::scoring::participant_properties::identifier(graph, site, 0x1e6b_affe)?,
            );
            let value = Evaluator::with_channels(graph, &values).field_or_input(
                site,
                name("TimeOutDuration"),
                "TimeOutDuration",
            )?;
            let Number::Float(timeout_seconds) = value else {
                return Err(bad("DNF timeout type"));
            };
            if !timeout_seconds.is_finite() || !(0.0..=3600.0).contains(&timeout_seconds) {
                return Err(bad("DNF timeout bound"));
            }
            transition.configuration = TransitionConfiguration::Dnf { timeout_seconds };
        }
        if transition.kind != TransitionKind::FinishLine {
            continue;
        }
        let links = graph.links(site, 0xfd69_801e)?;
        let laps = match links.as_slice() {
            [] => 1,
            [link] => {
                let mut current = link.clone();
                let mut visited = BTreeSet::new();
                let mut provider = false;
                loop {
                    if !visited.insert((current.site.clone(), current.port)) || visited.len() > 64 {
                        return Err(bad("finish-line waypoint link cycle"));
                    }
                    if let Some((bound, _)) = channels.get(&current.site) {
                        current.site = bound.clone();
                    }
                    if current.port == 0 {
                        if graph.object(&current.site)?.class
                            != "ChannelWaypointListProviderEntityData"
                        {
                            break;
                        }
                        // Native provider +c0 reads its bound waypoint channel.
                        // The finish transition itself does not link the channel.
                        provider = true;
                        current.port = 0x87b3_1017;
                    }
                    let links = graph.links(&current.site, current.port)?;
                    let [link] = links.as_slice() else {
                        return Err(bad("finish-line waypoint target count"));
                    };
                    current = link.clone();
                }
                let class = graph.object(&current.site)?.class;
                if !provider || class != "WaypointListChannelEntityData" {
                    return Err(bad(&format!("finish-line waypoint target class: {class}")));
                }
                super::route::laps(graph, &current.site)?
            }
            _ => return Err(bad("finish-line waypoint source count")),
        };
        transition.configuration = TransitionConfiguration::FinishLine { laps };
    }
    for state in &mut lifecycle.states {
        if let StateConfiguration::Countdown { seconds, .. } = &mut state.configuration {
            lifecycle.bindings.clocks.insert(
                state.serializer,
                crate::scoring::participant_properties::identifier(
                    graph,
                    &fields[state.serializer].0,
                    0x1e6b_affe,
                )?,
            );
            let value = Evaluator::with_channels(graph, &values).field_or_input(
                &fields[state.serializer].0,
                name("CountdownTime"),
                "CountdownTime",
            )?;
            let Number::Float(value) = value else {
                return Err(bad("countdown time type"));
            };
            if !value.is_finite() || !(0.0..=3600.0).contains(&value) {
                return Err(bad("countdown time bound"));
            }
            *seconds = value;
        }
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transition {
    pub serializer: usize,
    pub kind: TransitionKind,
    pub sources: Vec<Endpoint>,
    /// None is a native removal without entering another state.
    pub target: Option<Endpoint>,
    pub configuration: TransitionConfiguration,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Lifecycle {
    /// Participant states owned by an enclosing scene, reached through bound
    /// channel references (for example the teleport manager's restart states).
    #[serde(with = "crate::prepared_cache::ordered_map")]
    pub imports: BTreeMap<Endpoint, Vec<ImportedState>>,
    pub bindings: bindings::Bindings,
    pub initial: usize,
    pub final_state: usize,
    /// Optional secondary state observed by the game-mode root. It is not an
    /// implicit parent of every state, and time trial leaves this unconnected.
    pub root_state: Option<usize>,
    pub states: Vec<State>,
    pub transitions: Vec<Transition>,
    /// Root registration lists in authored order, distinct from all constructed
    /// states/transitions (nested logic may own additional ones).
    pub registered_states: Vec<usize>,
    pub registered_transitions: Vec<usize>,
}

/// State/observer content common to a loaded scene and a game-mode instance.
/// A scene has no invented Initial/Final state or game-mode root membership.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Scope {
    #[serde(with = "crate::prepared_cache::ordered_map")]
    pub imports: BTreeMap<Endpoint, Vec<ImportedState>>,
    pub bindings: bindings::Bindings,
    pub states: Vec<State>,
    pub transitions: Vec<Transition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Root {
    pub initial: usize,
    pub final_state: usize,
    pub root_state: Option<usize>,
    pub registered_states: Vec<usize>,
    pub registered_transitions: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ImportedState {
    pub identity: crate::scoring::events::Identity,
    pub class: String,
    pub scene_content_key: u32,
    /// Serializer in the enclosing scene, not in the game-mode profile.
    pub state: State,
}

impl Lifecycle {
    pub fn scope(&self) -> Scope {
        Scope {
            imports: self.imports.clone(),
            bindings: self.bindings.clone(),
            states: self.states.clone(),
            transitions: self.transitions.clone(),
        }
    }
    pub fn root(&self) -> Root {
        Root {
            initial: self.initial,
            final_state: self.final_state,
            root_state: self.root_state,
            registered_states: self.registered_states.clone(),
            registered_transitions: self.registered_transitions.clone(),
        }
    }
    /// Construct isolated membership storage using authored state identities.
    /// Resolving channel endpoints and specialized completion conditions is
    /// still the event host's responsibility.
    pub fn machine(&self) -> Result<nfs_world::participant_states::Machine, Error> {
        state_machine(&self.states)
    }
}
impl Scope {
    pub fn machine(&self) -> Result<nfs_world::participant_states::Machine, Error> {
        if self.states.is_empty() {
            Ok(nfs_world::participant_states::Machine::empty())
        } else {
            state_machine(&self.states)
        }
    }
}
fn state_machine(states: &[State]) -> Result<nfs_world::participant_states::Machine, Error> {
    use nfs_world::participant_states::{Definition, Machine};
    let definitions = states
        .iter()
        .map(|s| {
            Ok(Definition {
                id: u16::try_from(s.serializer).map_err(|_| bad("state identity bound"))?,
                handles_marked_for_delete: s.handles_marked_for_delete,
                send_rpc_to_client: s.send_rpc_to_client,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Machine::new(&definitions).map_err(|_| bad("participant state definitions"))
}

fn boolean(object: &Object, field: &str) -> Result<bool, Error> {
    match object.fields.get(field) {
        Some(Value::Bool(v)) => Ok(*v),
        _ => Err(bad("participant-state Boolean")),
    }
}

fn sequence_id<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<(String, usize), Error> {
    let id = graph.target_field(site, "SequenceId")?;
    if graph.source.object(&id)?.class != "StateSequenceId" {
        return Err(bad("state sequence identifier class"));
    }
    Ok(id)
}
fn ai_manager<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<Property, Error> {
    crate::scoring::participant_properties::identifier(graph, site, 0x1527_581a)
}
fn state_configuration<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    kind: StateKind,
    data: &Object,
) -> Result<StateConfiguration, Error> {
    use StateConfiguration as C;
    Ok(match kind {
        StateKind::Updateable => {
            let links = graph.links(site, 0x5ba0_872d)?;
            let [link] = links.as_slice() else {
                return Err(bad("updateable state update-set count"));
            };
            if link.port != 0 || graph.object(&link.site)?.class != "UpdateSetEntityData" {
                return Err(bad("updateable state update-set class"));
            }
            C::Updateable {
                update_set: crate::scoring::events::Identity::from(&link.site),
            }
        }
        StateKind::Countdown => C::Countdown {
            sequence: sequence_id(graph, site)?,
            seconds: finite(data.fields.get("CountdownTime"), "countdown time")?,
        },
        StateKind::Sequence => C::Sequence {
            sequence: sequence_id(graph, site)?,
        },
        StateKind::Waypoints => C::Waypoints {
            allowed_wrong_direction_distance: finite(
                data.fields.get("AllowedWrongDirectionDistance"),
                "wrong direction distance",
            )?,
        },
        StateKind::WaitForAi => C::WaitForAi {
            manager: ai_manager(graph, site)?,
        },
        _ => C::Default,
    })
}

pub(super) fn extract<S: Source>(
    graph: &mut Graph<S>,
    fields: &[FieldSite],
) -> Result<Lifecycle, Error> {
    let (root, class, _) = fields
        .first()
        .ok_or_else(|| bad("lifecycle root missing"))?;
    if class != "GameModeEntityData" {
        return Err(bad("lifecycle root class"));
    }
    let index: BTreeMap<_, _> = fields
        .iter()
        .enumerate()
        .map(|(i, (s, _, _))| (s.clone(), i))
        .collect();
    if index.len() != fields.len() {
        return Err(bad("duplicate lifecycle serializer site"));
    }
    let mut states = Vec::new();
    let mut transitions = Vec::new();
    for (serializer, (site, class, _)) in fields.iter().enumerate() {
        if let Some(kind) = state_kind(class) {
            let data = graph.object(site)?;
            states.push(State {
                serializer,
                kind,
                configuration: state_configuration(graph, site, kind, &data)?,
                handles_marked_for_delete: boolean(&data, "HandlesMarkedForDelete")?,
                send_rpc_to_client: boolean(&data, "SendRpcToClientSide")?,
            });
        }
        if let Some(kind) = transition_kind(class) {
            let sources = endpoints(graph, site, SOURCE, &index, fields)?;
            let targets = endpoints(graph, site, TARGET, &index, fields)?;
            if sources.is_empty() || sources.len() > MAX_STATES || targets.len() > 1 {
                return Err(bad("transition endpoint count"));
            }
            let configuration = match kind {
                TransitionKind::Dnf => TransitionConfiguration::Dnf {
                    timeout_seconds: finite(
                        graph.object(site)?.fields.get("TimeOutDuration"),
                        "DNF timeout",
                    )?,
                },
                TransitionKind::AiFilter => {
                    let targets = endpoints(graph, site, 0x5e32_a388, &index, fields)?;
                    if targets.len() > 1 {
                        return Err(bad("AI transition human target count"));
                    }
                    TransitionConfiguration::AiFilter {
                        manager: ai_manager(graph, site)?,
                        human_target: targets.first().copied(),
                    }
                }
                _ => TransitionConfiguration::Default,
            };
            transitions.push(Transition {
                serializer,
                kind,
                sources,
                target: targets.first().copied(),
                configuration,
            });
        }
    }
    if states.is_empty() || states.len() > MAX_STATES || transitions.len() > MAX_TRANSITIONS {
        return Err(bad("lifecycle bound"));
    }
    let state_ids: BTreeSet<_> = states.iter().map(|s| s.serializer).collect();
    let transition_ids: BTreeSet<_> = transitions.iter().map(|t| t.serializer).collect();
    let mut list = |port, allowed: &BTreeSet<usize>| -> Result<Vec<usize>, Error> {
        let mut result = Vec::new();
        for e in graph.links(root, port)? {
            let id = *index
                .get(&e.site)
                .ok_or_else(|| bad("unregistered lifecycle link"))?;
            if e.port != 0 || !allowed.contains(&id) || result.len() >= MAX_TRANSITIONS {
                return Err(bad("invalid lifecycle registration"));
            }
            result.push(id);
        }
        Ok(result)
    };
    let initial = list(INITIAL, &state_ids)?;
    let final_state = list(FINAL, &state_ids)?;
    let root_state = list(ROOT_STATE, &state_ids)?;
    let registered_states = list(STATES, &state_ids)?;
    let registered_transitions = list(TRANSITIONS, &transition_ids)?;
    let ([initial], [final_state]) = (initial.as_slice(), final_state.as_slice()) else {
        return Err(bad("initial/final state count"));
    };
    // InitialState is an independent native link. An authored setup state can
    // belong to this mode's components without appearing in its States list.
    // Keep registration order intact; do not synthesize a States entry for it.
    if root_state.len() > 1 || !registered_states.contains(final_state) {
        return Err(bad("lifecycle root membership"));
    }
    Ok(Lifecycle {
        imports: BTreeMap::new(),
        bindings: {
            let nodes = graph.read(root.clone())?;
            bindings::extract(graph, &nodes, fields)?
        },
        initial: *initial,
        final_state: *final_state,
        root_state: root_state.first().copied(),
        states,
        transitions,
        registered_states,
        registered_transitions,
    })
}

pub(super) fn resolve_imports<S: Source>(
    lifecycle: &mut Lifecycle,
    graph: &mut Graph<S>,
    fields: &[FieldSite],
    channels: &BTreeMap<Site, (Site, Binding)>,
    mut scene_endpoint: impl FnMut(&mut Graph<S>, &Site) -> Result<(u32, usize), Error>,
) -> Result<(), Error> {
    let mut external = BTreeSet::new();
    for t in &lifecycle.transitions {
        external.extend(t.sources.iter().copied());
        external.extend(t.target);
        if let TransitionConfiguration::AiFilter { human_target, .. } = t.configuration {
            external.extend(human_target);
        }
    }
    for p in &lifecycle.bindings.properties {
        use nfs_world::participant_states::properties::Definition;
        match &p.definition {
            Definition::List(states) => external.extend(states),
            Definition::Range { begin, end } => {
                external.insert(*begin);
                external.insert(*end);
            }
        }
    }
    for c in &lifecycle.bindings.contexts {
        external.extend(&c.states);
    }
    for c in &lifecycle.bindings.ai_controls {
        external.extend(c.states.iter().copied());
    }
    for c in lifecycle.bindings.ai_commands.values() {
        if let bindings::ai::Command::Waypoints { state, .. }
        | bindings::ai::Command::Pacing { state, .. } = c
        {
            external.insert(*state);
        }
    }
    external.extend(lifecycle.bindings.distances.iter().map(|d| d.state));
    for r in &lifecycle.bindings.reporters {
        external.extend(&r.states);
    }
    for s in &lifecycle.bindings.sets {
        if let bindings::SetKind::States(states) = &s.kind {
            external.extend(states);
        }
    }
    let mut imports = BTreeMap::new();
    for endpoint in external.into_iter().filter(|e| e.port != 0) {
        let reference = &fields
            .get(endpoint.serializer)
            .ok_or_else(|| bad("external state reference"))?
            .0;
        let (channel, _) = channels
            .get(reference)
            .ok_or_else(|| bad("unbound external state channel"))?;
        let sites = imported_sites(
            graph,
            channel,
            endpoint.port,
            channels,
            &mut BTreeSet::new(),
        )?;
        let mut resolved = Vec::new();
        for site in sites {
            let object = graph.object(&site)?;
            if !is_scene_state(&object.class) {
                return Err(bad(&format!(
                    "external state target class {} at {site:?}, channel {channel:?}, port {:08x}",
                    object.class, endpoint.port
                )));
            }
            let (scene_content_key, serializer) = scene_endpoint(graph, &site)?;
            resolved.push(ImportedState {
                identity: crate::scoring::events::Identity::from(&site),
                class: object.class.clone(),
                scene_content_key,
                state: State {
                    serializer,
                    kind: StateKind::External,
                    configuration: StateConfiguration::Default,
                    handles_marked_for_delete: boolean(&object, "HandlesMarkedForDelete")?,
                    send_rpc_to_client: boolean(&object, "SendRpcToClientSide")?,
                },
            });
        }
        imports.insert(endpoint, resolved);
    }
    lifecycle.imports = imports;
    Ok(())
}

// These SDK classes derive from ParticipantStateEntityData. Import only their
// membership contract: sequence, teleport, garage and crash behavior remains
// with the enclosing scene, and must not be constructed a second time here.
fn is_scene_state(class: &str) -> bool {
    state_kind(class).is_some()
        || matches!(
            class,
            "BlockIfInExternalGameModeStateEntityData"
                | "BlockIfInPursuitGameModeStateEntityData"
                | "CooldownGameModeStateEntityData"
                | "DisconnectPlayerParticipantStateEntityData"
                | "EscapeGameModeStateEntityData"
                | "FMVParticipantStateEntityData"
                | "GarageParticipantStateEntityData"
                | "GetAiSpawnParametersParticipantStateEntityData"
                | "GetSpawnParametersParticipantStateEntityData"
                | "LoadInventoryParticipantStateEntityData"
                | "LoadPersistentDataParticipantStateEntityData"
                | "MoveToTransformParticipantStateEntityData"
                | "OrienteeringWaypointsGameModeStateEntityData"
                | "RecoverParticipantStateEntityData"
                | "SequenceGameModeStateEntityData"
                | "SetInitialTeleportLocationParticipantStateEntityData"
                | "SpawningParticipantStateEntityData"
                | "StateSequenceParticipantStateEntityData"
                | "TeleportingParticipantStateEntityData"
                | "UnloadPersistentDataParticipantStateEntityData"
                | "UnspawningAiParticipantStateEntityData"
                | "UnspawningParticipantStateEntityData"
                | "WaitForPendingItemUpdatesParticipantStateEntityData"
        )
}

/// A native multi-link query expands every channel target in authored order.
/// Cardinality is checked by the consuming single-state contract, not here.
fn imported_sites<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    channels: &BTreeMap<Site, (Site, Binding)>,
    active: &mut BTreeSet<(Site, u32)>,
) -> Result<Vec<Site>, Error> {
    if active.len() >= 64 || !active.insert((site.clone(), port)) {
        return Err(bad("external state link cycle"));
    }
    let mut result = Vec::new();
    let actual = channels.get(site).map_or(site, |(target, _)| target);
    for link in graph.links(actual, port)? {
        if link.port == 0 {
            result.push(link.site);
        } else {
            result.extend(imported_sites(
                graph, &link.site, link.port, channels, active,
            )?);
        }
        if result.len() > MAX_STATES {
            return Err(bad("external state list bound"));
        }
    }
    active.remove(&(site.clone(), port));
    Ok(result)
}

fn endpoints<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    index: &BTreeMap<Site, usize>,
    fields: &[FieldSite],
) -> Result<Vec<Endpoint>, Error> {
    graph
        .links(site, port)?
        .into_iter()
        .map(|e| {
            let serializer = *index
                .get(&e.site)
                .ok_or_else(|| bad("transition links an unknown serializer"))?;
            let class = &fields[serializer].1;
            if !(e.port == 0 && state_kind(class).is_some()
                || e.port != 0 && class == "LinkedSchematicChannelReferenceEntityData")
            {
                return Err(bad("transition endpoint is not a state or channel"));
            }
            Ok(Endpoint {
                serializer,
                port: e.port,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
