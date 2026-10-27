// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What sets the tutorial gates: the IdentifiedBool accessors of a mission
//! chapter (and the prefabs it references). Each accessor's value is a Bool
//! latched by an event chain through server Delays, a Bool that follows a
//! SyncedBool, a RaceObjective property, or an And of those. Installed intro
//! (`gameplay/progression/missionchapters/intro`): earningreputation at the
//! return prefab's event 1530399826, phonemessages and accessingphone 40 s and
//! 90 s after it, introfinish 5 s after the return objective completes,
//! remindertogarage while that objective is active after the event, and
//! remindertoclub (introdrivetoclub) while the drive objective is active after
//! the launcher started.
use super::*;

const ACCESSOR: &str = "IdentifiedBoolAccessorEntityData";
const ACCESSOR_VALUE: u32 = 225_375_086;
const BOOL_SET_TRUE: u32 = 2_753_900_881;
const BOOL_SET_FALSE: u32 = 668_205_626;
const SYNCED_ON_TRUE: u32 = 2_861_454_674;
const SYNCED_ON_FALSE: u32 = 4_213_085_657;
const SYNCED_IN: u32 = 5_862_146;
const LAUNCHER_STARTED: u32 = 3_655_010_165;
const MAX_HOPS: usize = 8;
const MAX_RULES: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    /// An interface event of a referenced prefab (its partition).
    PrefabEvent { prefab: String, event: u32 },
    /// An interface property of a referenced prefab, as a SyncedBool sees it.
    PrefabProperty { prefab: String, property: u32 },
    /// A RaceObjective's property inside `prefab`.
    Objective { prefab: String, property: u32 },
    /// The mode-started output of the launcher inside `prefab`.
    LauncherStarted { prefab: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    /// True from `seconds` after the trigger's first rise (never reset).
    Since { trigger: Trigger, seconds: f32 },
    /// True while the trigger's value is true.
    While(Trigger),
}

/// One gate: the IdentifiedBool asset and the terms that must all hold.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub gate: String,
    pub all: Vec<Term>,
}

fn realm_server(object: &nfs_frostbite::ebx::Object) -> bool {
    matches!(object.fields.get("Realm"),
        Some(Value::Enum { name: Some(n), .. }) if n == "Realm_Server")
}

/// The chain behind one event input: server Delays back to a root.
fn chain<S: Source>(
    graph: &mut Graph<S>,
    mut site: Site,
    mut port: u32,
) -> Result<(Trigger, f32), Error> {
    let mut seconds = 0f32;
    for _ in 0..MAX_HOPS {
        let object = graph.object(&site)?;
        match object.class.as_str() {
            "DelayEntityData" if port == nfs_world::scoring::nodes::OUT => {
                let world = matches!(object.fields.get("TimeDeltaType"),
                    Some(Value::Enum { name: Some(n), .. }) if n == "TimeDeltaType_World");
                let Some(Value::Float32(delay)) = object.fields.get("Delay") else {
                    return Err(bad("tutorial gate delay"));
                };
                if !realm_server(&object) || !world || !(0. ..=600.).contains(delay) {
                    return Err(bad("unsupported tutorial gate delay"));
                }
                seconds += delay;
                let sources = graph.event_sources(&site, nfs_world::scoring::nodes::IN)?;
                let [(next, next_port)] = sources.as_slice() else {
                    return Err(bad("tutorial gate delay input"));
                };
                (site, port) = (next.clone(), *next_port);
            }
            "LogicPrefabReferenceObjectData" => {
                let prefab = graph.target_field(&site, "Blueprint")?.0;
                return Ok((
                    Trigger::PrefabEvent {
                        prefab,
                        event: port,
                    },
                    seconds,
                ));
            }
            "SimpleGameModeLauncherEntityData" if port == LAUNCHER_STARTED => {
                let prefab = site.key.0.clone();
                return Ok((Trigger::LauncherStarted { prefab }, seconds));
            }
            "SyncedBoolEntityData" if port == SYNCED_ON_TRUE => {
                return Ok((synced(graph, &site)?, seconds));
            }
            _ => return Err(bad("unsupported tutorial gate event source")),
        }
    }
    Err(bad("tutorial gate chain bound"))
}

/// The prefab property a SyncedBool mirrors.
fn synced<S: Source>(graph: &mut Graph<S>, site: &Site) -> Result<Trigger, Error> {
    let sources = graph.property_sources(site, SYNCED_IN)?;
    let [source] = sources.as_slice() else {
        return Err(bad("tutorial gate synced input"));
    };
    if graph.object(&source.site)?.class != "LogicPrefabReferenceObjectData" {
        return Err(bad("tutorial gate synced source"));
    }
    Ok(Trigger::PrefabProperty {
        prefab: graph.target_field(&source.site, "Blueprint")?.0,
        property: source.port,
    })
}

/// The terms of one property source.
fn terms<S: Source>(
    graph: &mut Graph<S>,
    source: &Endpoint,
    depth: usize,
    out: &mut Vec<Term>,
) -> Result<(), Error> {
    if depth > MAX_DEPTH || out.len() >= MAX_DEPTH {
        return Err(bad("tutorial gate depth"));
    }
    let object = graph.object(&source.site)?;
    match object.class.as_str() {
        "AndEntityData" if realm_server(&object) => {
            let ports = graph.input_ports(&source.site)?;
            if ports.len() < 2 {
                return Err(bad("tutorial gate and inputs"));
            }
            for port in ports {
                let inputs = graph.property_sources(&source.site, port)?;
                let [input] = inputs.as_slice() else {
                    return Err(bad("tutorial gate and input"));
                };
                terms(graph, input, depth + 1, out)?;
            }
        }
        "BoolEntityData" if realm_server(&object) => {
            if matches!(object.fields.get("DefaultValue"), Some(Value::Bool(true))) {
                return Err(bad("tutorial gate default true"));
            }
            let sets = graph.event_sources(&source.site, BOOL_SET_TRUE)?;
            let resets = graph.event_sources(&source.site, BOOL_SET_FALSE)?;
            match (sets.as_slice(), resets.as_slice()) {
                ([(set, SYNCED_ON_TRUE)], [(reset, SYNCED_ON_FALSE)])
                    if set == reset && graph.object(set)?.class == "SyncedBoolEntityData" =>
                {
                    out.push(Term::While(synced(graph, set)?));
                }
                ([(set, port)], []) => {
                    let (trigger, seconds) = chain(graph, set.clone(), *port)?;
                    out.push(Term::Since { trigger, seconds });
                }
                _ => return Err(bad("unsupported tutorial gate bool")),
            }
        }
        "RaceObjectiveEntityData" => out.push(Term::While(Trigger::Objective {
            prefab: source.site.key.0.clone(),
            property: source.port,
        })),
        _ => return Err(bad("unsupported tutorial gate source")),
    }
    Ok(())
}

impl AssetIndex {
    /// The rules of every IdentifiedBool `chapter` sets (one writer each).
    pub fn tutorial_gates(&mut self, chapter: &str) -> Result<Vec<Rule>, Error> {
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(chapter))?;
        let mut rules: Vec<Rule> = vec![];
        for node in nodes.iter().filter(|n| n.object.class == ACCESSOR) {
            let sources = graph.property_sources(&node.site, ACCESSOR_VALUE)?;
            let [source] = sources.as_slice() else {
                // An accessor that only reads its bool sets nothing.
                if sources.is_empty() {
                    continue;
                }
                return Err(bad("tutorial gate writers"));
            };
            let gate = graph
                .target_field(&node.site, "BoolIdentifier")?
                .0
                .to_lowercase();
            if rules.len() >= MAX_RULES || rules.iter().any(|r| r.gate == gate) {
                return Err(bad("tutorial gate rule count"));
            }
            let mut all = vec![];
            terms(&mut graph, source, 0, &mut all)?;
            rules.push(Rule { gate, all });
        }
        Ok(rules)
    }
}
