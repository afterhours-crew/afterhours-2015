// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored event rosters and membership dependencies, independently of playable
//! game modes. The compatibility roster remains normal-only; availability keeps
//! story variants and requires state supplied by their runtime owners.
use super::*;

pub mod channels;
pub mod integers;
pub mod membership;
pub mod requirements;

pub const MAX_EVENTS: usize = 512;

/// A scene serializer, resolved in its full prefab instance context.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct Endpoint {
    pub scene_key: u32,
    pub serializer: usize,
}

/// One start area. Narrative and Prestige variants can share its location.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub id: String,
    pub scene: String,
    pub game_mode: String,
    pub volume: Endpoint,
    /// Eligibility and mission-selection notifications required by the map POI.
    pub notifications: Vec<Endpoint>,
}

impl AssetIndex {
    /// Resolve normal authored start areas in loaded scenes. This does not imply
    /// support for starting or completing each of their game modes.
    pub fn event_roster(&mut self) -> Result<Vec<Entry>, Error> {
        self.roster(false, false).map(|r| r.entries)
    }

    /// Include notification-backed start requirements, preserving their own
    /// participant-state and Boolean-property dependencies.
    pub fn event_roster_with_requirements(
        &mut self,
    ) -> Result<(Vec<Entry>, Vec<requirements::Binding>), Error> {
        self.roster(true, false)
            .map(|r| (r.entries, r.requirements))
    }

    /// All authored variants with exact map-membership expressions. Unlike
    /// the normal-only compatibility roster, no story selection is pre-applied.
    pub fn event_availability(&mut self) -> Result<Availability, Error> {
        self.roster(false, true).map(|r| Availability {
            entries: r.entries,
            memberships: r.memberships.into_values().collect(),
            integers: r.integers,
            visibility: r.visibility,
            copies: r.copies,
            channels: r.channels,
        })
    }

    fn roster(
        &mut self,
        include_requirements: bool,
        include_memberships: bool,
    ) -> Result<Roster, Error> {
        let rules = self.identity.scenes.clone();
        let startup = self.startup_entries(&rules.level)?;
        let mut graph = Graph::new(&mut self.assets);
        let mut scenes = SceneLookup {
            roots: scene_roots(&mut graph, &rules.level, &startup)?,
            rules: &rules,
            endpoints: BTreeMap::new(),
            loaded: BTreeSet::new(),
        };
        let roots: Vec<_> = scenes
            .roots
            .iter()
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect();
        let identified = if include_requirements || include_memberships {
            requirements::identified_bools(&mut graph, &roots)?
        } else {
            BTreeMap::new()
        };
        let mut entries = BTreeMap::new();
        let mut requirements = Vec::new();
        let mut memberships = BTreeMap::new();
        let mut visibility_rules = BTreeMap::new();
        for (scene, root) in &roots {
            let nodes = graph.read(root.clone())?;
            for launcher in nodes.iter().filter(|n| n.object.class == LAUNCHER) {
                let mode = graph.target_field(&launcher.site, "GameMode")?.0;
                // Automatic speed-run/drift-corner activities have their own
                // map/trigger lifecycle; this roster is for authored races.
                if mode.starts_with("gameplay/activities/") {
                    continue;
                }
                // Invitation launchers use a group-invite identity, not an event.
                let Ok(channel) = sibling(&nodes, DETAILS_CHANNEL, &launcher.site) else {
                    continue;
                };
                let port = one_port(&mut graph, &channel.site)?;
                let targets = graph.links(&channel.site, port)?;
                let details: Vec<_> = targets
                    .into_iter()
                    .filter_map(|t| match graph.object(&t.site) {
                        Ok(o) if o.class == DETAILS => Some(Ok(t.site)),
                        Ok(_) => None,
                        Err(e) => Some(Err(e)),
                    })
                    .collect::<Result<_, _>>()?;
                let [details] = details.as_slice() else {
                    continue;
                };
                let definition = graph.object(details)?;
                let categories = definition
                    .fields
                    .get("Categories")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() <= 32)
                    .ok_or_else(|| bad("event categories"))?;
                let mut authored = false;
                for category in categories {
                    let target = graph.source.target(
                        &details.key.0,
                        category
                            .as_pointer()
                            .ok_or_else(|| bad("event category pointer"))?,
                    )?;
                    authored |= target.is_some_and(|(asset, _)| {
                        asset == "gameplay/gamemodes/categories/authoredevent"
                    });
                }
                if !authored {
                    continue;
                }
                let id = graph.target_field(details, "Id")?.0;
                if !id.starts_with("gameplay/gamemodes/")
                    || (!include_memberships && (id.ends_with("_nar") || id.ends_with("_pres")))
                {
                    continue;
                }
                let Some(frame) = launcher.site.parents.last() else {
                    continue;
                };
                let instance = &frame.reference;
                let pois: Vec<_> = nodes
                    .iter()
                    .filter(|n| {
                        n.object.class == "MapGenericPoiEntityData" && within(&n.site, instance)
                    })
                    .collect();
                let [poi] = pois.as_slice() else { continue };
                let volume = unique_in(&nodes, START_VOLUME, instance)?.site.clone();
                let mut notifications = BTreeMap::new();
                visibility(
                    &mut graph,
                    &poi.site,
                    name("Enabled"),
                    true,
                    &mut notifications,
                    &mut BTreeSet::new(),
                )
                .map_err(|e| bad(&format!("event {id} visibility: {e}")))?;
                let mut resolved = Vec::new();
                let mut conditions = BTreeMap::new();
                for (site, enabled) in notifications {
                    if !enabled && !include_memberships {
                        continue;
                    }
                    let (scene_key, serializer) = scenes.endpoint(&mut graph, &site)?;
                    let endpoint = Endpoint {
                        scene_key,
                        serializer,
                    };
                    if include_memberships
                        && conditions
                            .insert(endpoint, enabled)
                            .is_some_and(|old| old != enabled)
                    {
                        return Err(bad("conflicting resolved visibility notification"));
                    }
                    if include_memberships && !memberships.contains_key(&endpoint) {
                        let program =
                            membership::read(&mut graph, &site, &identified, &mut |g, s| {
                                let (scene_key, serializer) = scenes.endpoint(g, s)?;
                                Ok(Endpoint {
                                    scene_key,
                                    serializer,
                                })
                            })
                            .map_err(|e| bad(&format!("event {id} membership: {e}")))?;
                        memberships.insert(
                            endpoint,
                            membership::Binding {
                                notification: endpoint,
                                program,
                            },
                        );
                    }
                    if enabled {
                        resolved.push(endpoint);
                    }
                }
                resolved.sort();
                resolved.dedup();
                if resolved.is_empty() || resolved.len() > 16 {
                    return Err(bad("event map notification count"));
                }
                if include_memberships {
                    let rule = Visibility { conditions };
                    rule.validate()?;
                    if visibility_rules.insert(id.clone(), rule).is_some() {
                        return Err(bad("duplicate event visibility identity"));
                    }
                }
                let (scene_key, serializer) = scenes.endpoint(&mut graph, &volume)?;
                let entry = Entry {
                    id: id.clone(),
                    scene: scene.clone(),
                    game_mode: mode,
                    volume: Endpoint {
                        scene_key,
                        serializer,
                    },
                    notifications: resolved,
                };
                if include_requirements {
                    let mut start_area = launcher.site.clone();
                    let frame = start_area
                        .parents
                        .pop()
                        .ok_or_else(|| bad("start area parent"))?;
                    start_area.key = frame.reference;
                    start_area.bus = frame.bus;
                    requirements.extend(requirements::read(
                        &mut graph,
                        &mut scenes,
                        &start_area,
                        &entry,
                        &identified,
                    )?);
                }
                if entries.insert(id, entry).is_some() || entries.len() > MAX_EVENTS {
                    return Err(bad("duplicate event identity or roster bound"));
                }
            }
        }
        let selected = memberships
            .values()
            .flat_map(|b| b.program.ranges().values())
            .map(|r| r.input.clone())
            .collect();
        let integers = integers::read(
            &mut graph,
            &roots.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
            &selected,
        )?;
        let selected_copies = memberships
            .values()
            .flat_map(|b| b.program.facts())
            .filter_map(|f| match f {
                membership::Fact::Copy(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let copies = membership::copies::read(
            &mut graph,
            &roots.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
            &selected_copies,
            &identified,
            &mut |g, s| {
                let (scene_key, serializer) = scenes.endpoint(g, s)?;
                Ok(Endpoint {
                    scene_key,
                    serializer,
                })
            },
        )?;
        let channels = channels::read(
            &mut graph,
            &roots.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
            &copies.values().map(|c| c.channel.clone()).collect(),
        )?;
        Ok(Roster {
            entries: entries.into_values().collect(),
            requirements,
            memberships,
            integers,
            visibility: visibility_rules,
            copies,
            channels,
        })
    }
}

struct Roster {
    entries: Vec<Entry>,
    requirements: Vec<requirements::Binding>,
    memberships: BTreeMap<Endpoint, membership::Binding>,
    integers: BTreeMap<crate::scoring::events::Identity, integers::Store>,
    visibility: BTreeMap<String, Visibility>,
    copies: BTreeMap<crate::scoring::events::Identity, membership::copies::Control>,
    channels: BTreeMap<Property, Vec<channels::Publisher>>,
}

#[derive(Clone, Debug)]
pub struct Availability {
    pub entries: Vec<Entry>,
    pub memberships: Vec<membership::Binding>,
    pub integers: BTreeMap<crate::scoring::events::Identity, integers::Store>,
    /// Full positive and negative membership conditions for each variant.
    /// Client-owned presentation channels remain separate from this rule.
    pub visibility: BTreeMap<String, Visibility>,
    pub copies: BTreeMap<crate::scoring::events::Identity, membership::copies::Control>,
    pub channels: BTreeMap<Property, Vec<channels::Publisher>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Visibility {
    pub conditions: BTreeMap<Endpoint, bool>,
}
impl Visibility {
    fn validate(&self) -> Result<(), Error> {
        if self.conditions.is_empty()
            || self.conditions.len() > 16
            || self.conditions.keys().any(|s| s.scene_key == 0)
        {
            return Err(bad("event visibility condition bound or identity"));
        }
        Ok(())
    }
    /// Require all memberships, including a negative or currently unselected
    /// branch. Missing state is not evidence that a variant is unavailable.
    pub fn evaluate(
        &self,
        mut member: impl FnMut(Endpoint) -> Option<bool>,
    ) -> Result<bool, Error> {
        self.validate()?;
        let mut eligible = true;
        for (&site, &required) in &self.conditions {
            eligible &=
                member(site).ok_or_else(|| bad("event visibility membership missing"))? == required;
        }
        Ok(eligible)
    }
}

/// Follow the authored conjunction/negation gates to the membership notifications.
/// Channel inputs stay under the client's existing free-roam/event state. They are
/// not overridden to force a POI on during races or Prestige mode.
fn visibility<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    wanted: bool,
    notifications: &mut BTreeMap<Site, bool>,
    active: &mut BTreeSet<(Site, u32)>,
) -> Result<(), Error> {
    if active.len() >= 64 || !active.insert((site.clone(), port)) {
        return Err(bad("visibility cycle or depth"));
    }
    let sources = graph.inputs(site, port)?;
    if sources.len() > 8 {
        return Err(bad("visibility input bound"));
    }
    for source in sources {
        let o = graph.object(&source.site)?;
        match o.class.as_str() {
            NOTIFICATION if source.port == 513_251_846 => {
                if notifications
                    .insert(source.site.clone(), wanted)
                    .is_some_and(|old| old != wanted)
                {
                    return Err(bad("conflicting visibility notification"));
                }
            }
            "NotEntityData" if source.port == name("Out") => visibility(
                graph,
                &source.site,
                name("In"),
                !wanted,
                notifications,
                active,
            )?,
            "AndEntityData" if source.port == name("Out") && wanted => {
                let count = o
                    .fields
                    .get("InputCount")
                    .and_then(Value::bits32)
                    .filter(|v| (1..=8).contains(v))
                    .ok_or_else(|| bad("visibility conjunction count"))?;
                for i in 1..=count {
                    visibility(
                        graph,
                        &source.site,
                        name(&format!("In{i}")),
                        true,
                        notifications,
                        active,
                    )?;
                }
            }
            "MathEntityData" if source.port == name("Out") => {
                if !wanted {
                    return Err(bad("negative visibility conjunction"));
                }
                for (port, wanted) in conjunction(&o)? {
                    visibility(graph, &source.site, port, wanted, notifications, active)?;
                }
            }
            "SchematicChannelEntityData"
            | "LinkedSchematicChannelRelayEntityData"
            | "LinkedSchematicChannelEntityData" => {}
            "BoolPropertyClientNotificationEntityData"
                if source.port == name("Value") && !wanted => {}
            _ => {
                return Err(bad(&format!(
                    "unsupported visibility {}:{} {} port {}",
                    source.site.key.0, source.site.key.1, o.class, source.port
                )));
            }
        }
    }
    active.remove(&(site.clone(), port));
    Ok(())
}

/// The authored mission switch is A AND B, or A AND NOT B. Validate the complete
/// instruction/register/return contract before treating it as a conjunction.
fn conjunction(object: &Object) -> Result<[(u32, bool); 2], Error> {
    if object
        .fields
        .get("EvaluateOnEvent")
        .and_then(Value::as_bool)
        != Some(false)
    {
        return Err(bad("event-driven visibility math"));
    }
    let assembly = object
        .fields
        .get("Assembly")
        .and_then(Value::as_fields)
        .ok_or_else(|| bad("visibility math assembly"))?;
    if assembly
        .get("FunctionCalls")
        .and_then(Value::as_array)
        .is_none_or(|a| !a.is_empty())
    {
        return Err(bad("visibility math functions"));
    }
    let instructions = assembly
        .get("Instructions")
        .and_then(Value::as_array)
        .filter(|a| (4..=5).contains(&a.len()))
        .ok_or_else(|| bad("visibility math instruction count"))?;
    let mut rows = Vec::new();
    for instruction in instructions {
        let f = instruction
            .as_fields()
            .ok_or_else(|| bad("visibility math instruction"))?;
        let code = f
            .get("Code")
            .and_then(Value::as_enum_name)
            .ok_or_else(|| bad("visibility math opcode"))?;
        let n = |k| {
            f.get(k)
                .and_then(Value::bits32)
                .ok_or_else(|| bad("visibility math operand"))
        };
        rows.push((code, n("Result")?, n("Param1")?, n("Param2")?));
    }
    let negative = rows.len() == 5;
    if rows[0].0 != "MathOpCode_InputB"
        || rows[0].1 != 0
        || rows[0].3 != 0
        || rows[1].0 != "MathOpCode_InputB"
        || rows[1].1 != 1
        || rows[1].3 != 0
        || (negative && rows[2] != ("MathOpCode_NotB", 1, 1, 0))
        || rows[rows.len() - 2] != ("MathOpCode_AndB", 0, 0, 1)
        || rows[rows.len() - 1] != ("MathOpCode_Return", 0, 1, 0)
    {
        return Err(bad("unsupported visibility math conjunction"));
    }
    Ok([(rows[0].2, true), (rows[1].2, !negative)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local};
    use nfs_frostbite::ebx::Fields;

    #[test]
    fn shared_location_variants_retain_both_sides_of_selection_and_require_all_state() {
        let unlocked = Endpoint {
            scene_key: 3,
            serializer: 1,
        };
        let selected = Endpoint {
            scene_key: 3,
            serializer: 2,
        };
        let normal = Visibility {
            conditions: BTreeMap::from([(unlocked, true), (selected, false)]),
        };
        let story = Visibility {
            conditions: BTreeMap::from([(unlocked, true), (selected, true)]),
        };
        for (open, selection, normal_expected, story_expected) in [
            (false, false, false, false),
            (true, false, true, false),
            (true, true, false, true),
            (false, true, false, false),
        ] {
            let state = |s| Some(if s == unlocked { open } else { selection });
            assert_eq!(normal.evaluate(state).unwrap(), normal_expected);
            assert_eq!(story.evaluate(state).unwrap(), story_expected);
        }
        assert!(
            normal
                .evaluate(|s| (s == unlocked).then_some(false))
                .is_err()
        );
        assert!(
            Visibility {
                conditions: BTreeMap::new()
            }
            .evaluate(|_| Some(true))
            .is_err()
        );
    }

    fn fixture() -> Memory {
        let mut m = Memory::default();
        m.put(
            "a",
            0,
            "Blueprint",
            &[(
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(2), name("Out"), local(1), name("Enabled")),
                    edge(local(3), 513_251_846, local(2), name("In1")),
                    edge(local(4), name("Out"), local(2), name("In2")),
                    edge(local(5), 513_251_846, local(4), name("In")),
                ]),
            )],
        );
        m.put("a", 1, "MapGenericPoiEntityData", &[]);
        m.put("a", 2, "AndEntityData", &[("InputCount", Value::Int32(2))]);
        m.put("a", 3, NOTIFICATION, &[]);
        m.put("a", 4, "NotEntityData", &[]);
        m.put("a", 5, NOTIFICATION, &[]);
        m
    }
    fn read(m: &mut Memory) -> Result<BTreeMap<Site, bool>, Error> {
        let mut out = BTreeMap::new();
        let mut poi = Site::root("a");
        poi.key.1 = 1;
        visibility(
            &mut Graph::new(m),
            &poi,
            name("Enabled"),
            true,
            &mut out,
            &mut BTreeSet::new(),
        )?;
        Ok(out)
    }
    #[test]
    fn map_gates_preserve_positive_and_negative_membership_requirements() {
        let found = read(&mut fixture()).unwrap();
        assert_eq!(
            found
                .into_iter()
                .map(|(s, v)| (s.key.1, v))
                .collect::<Vec<_>>(),
            [(3, true), (5, false)]
        );
    }
    #[test]
    fn unknown_gates_cycles_and_conflicting_membership_are_rejected() {
        let mut m = fixture();
        m.put("a", 3, "UnknownGate", &[]);
        assert!(read(&mut m).is_err());
        let mut m = fixture();
        m.put(
            "a",
            0,
            "Blueprint",
            &[(
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(2), name("Out"), local(1), name("Enabled")),
                    edge(local(3), 513_251_846, local(2), name("In1")),
                    edge(local(4), name("Out"), local(2), name("In2")),
                    edge(local(3), 513_251_846, local(4), name("In")),
                ]),
            )],
        );
        assert!(read(&mut m).is_err());
        m.put(
            "a",
            0,
            "Blueprint",
            &[(
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(4), name("Out"), local(1), name("Enabled")),
                    edge(local(4), name("Out"), local(4), name("In")),
                ]),
            )],
        );
        assert!(read(&mut m).is_err());
    }
    fn fields(rows: Vec<(&str, Value)>) -> Value {
        Value::Struct(Fields(
            rows.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        ))
    }
    fn instruction(code: &str, result: u32, a: u32, b: u32) -> Value {
        fields(vec![
            (
                "Code",
                Value::Enum {
                    value: 0,
                    name: Some(format!("MathOpCode_{code}")),
                },
            ),
            ("Result", Value::UInt32(result)),
            ("Param1", Value::UInt32(a)),
            ("Param2", Value::UInt32(b)),
        ])
    }
    #[test]
    fn mission_conjunction_validates_registers_and_complement() {
        for negative in [false, true] {
            let mut rows = vec![
                instruction("InputB", 0, 900, 0),
                instruction("InputB", 1, 901, 0),
            ];
            if negative {
                rows.push(instruction("NotB", 1, 1, 0));
            }
            rows.extend([instruction("AndB", 0, 0, 1), instruction("Return", 0, 1, 0)]);
            let mut m = Memory::default();
            m.put(
                "a",
                0,
                "MathEntityData",
                &[
                    ("EvaluateOnEvent", Value::Bool(false)),
                    (
                        "Assembly",
                        fields(vec![
                            ("FunctionCalls", Value::Array(vec![])),
                            ("Instructions", Value::Array(rows.clone())),
                        ]),
                    ),
                ],
            );
            assert_eq!(
                conjunction(&m.0[&("a".into(), 0)]).unwrap(),
                [(900, true), (901, !negative)]
            );
            rows.pop();
            rows.push(instruction("Return", 0, 2, 0));
            m.put(
                "a",
                0,
                "MathEntityData",
                &[
                    ("EvaluateOnEvent", Value::Bool(false)),
                    (
                        "Assembly",
                        fields(vec![
                            ("FunctionCalls", Value::Array(vec![])),
                            ("Instructions", Value::Array(rows)),
                        ]),
                    ),
                ],
            );
            assert!(conjunction(&m.0[&("a".into(), 0)]).is_err());
        }
    }
}
