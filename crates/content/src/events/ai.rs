// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored event roster: start-area spawner, filtered persona provider and
//! transformed grid locators. Selection, availability and lifecycle are owned
//! by the runtime. Never select the first matching channel elsewhere in a scene.
use super::spatial::location;
use crate::{
    Error,
    graph::{Graph, Node, Site, Source, bad},
    rivals::{
        Links, Persona,
        extract::{persona, tags},
    },
};
use nfs_frostbite::ebx::{Object, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_SPAWNS: usize = 32;
pub const MAX_PERSONAS: usize = 1024;
// Installed link fields, validated against the normal event's authored graph.
const EVENT_SPAWNER: u32 = 356_246_362;
const SPAWNS: u32 = (-2_102_271_509i32) as u32;
// The spawner processes these authored entries before its ordinary fill list.
// Story variants can have only this list, or mix it with ordinary opponents.
const PRIORITY_SPAWNS: u32 = (-1_118_065_650i32) as u32;
const SPAWN_LOCATION: u32 = (-1_050_389_661i32) as u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Completion {
    UnspawnAllAtShutdown,
    GiveAllToDirectorAtShutdown,
    UnspawnIndividualAtCompletion,
    GiveIndividualToDirectorAtCompletion,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Spawn {
    pub personas: Vec<usize>,
    pub position: [f32; 3],
    pub basis: [[f32; 3]; 3],
    pub team: u8,
    pub once: bool,
    pub store_participant: bool,
    pub ignore_availability: bool,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Roster {
    pub managers: Option<nfs_world::replication::players::ai::Managers>,
    pub completion: Option<Completion>,
    pub personas: Vec<Persona>,
    /// Persona-aligned authored pacing skill. None means no linked personality;
    /// callers must explicitly support that case before using the persona.
    pub pacing_skills: Vec<Option<f32>>,
    pub spawns: Vec<Spawn>,
}

fn boolean(o: &Object, field: &str) -> Result<bool, Error> {
    o.fields
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| bad("AI spawn Boolean"))
}
fn one<S: Source>(g: &mut Graph<S>, site: &Site, port: u32, class: &str) -> Result<Site, Error> {
    let rows = g.links(site, port)?;
    if rows.len() != 1 || rows[0].port != 0 || g.object(&rows[0].site)?.class != class {
        return Err(bad(&format!("AI spawn expects one {class}")));
    }
    Ok(rows[0].site.clone())
}
fn provider_set<S: Source>(
    g: &mut Graph<S>,
    provider: &Site,
    manager: &[Node],
    ports: Links,
) -> Result<Site, Error> {
    let links = g.links(provider, ports.provider)?;
    let [relay] = links.as_slice() else {
        return Err(bad("AI union provider source count"));
    };
    let class = g.object(&relay.site)?.class;
    // Story providers reach the narrative set directly through the scene's
    // interface. Preserve that occurrence and its original persona ordinals.
    if class == "PersonaSetEntityData" && relay.port == 0 {
        return Ok(relay.site.clone());
    }
    if class != "LinkedSchematicChannelRelayEntityData" {
        return Err(bad(
            "AI union provider requires a persona set or channel relay",
        ));
    }
    let asset = g.target_field(&relay.site, "ChannelAsset")?;
    let mut found = Vec::new();
    for node in manager
        .iter()
        .filter(|n| n.object.class == "LinkedSchematicChannelEntityData")
    {
        if g.target_field(&node.site, "ChannelAsset")? == asset {
            found.push(node.site.clone());
        }
    }
    let [channel] = found.as_slice() else {
        return Err(bad("AI persona channel missing or ambiguous"));
    };
    one(g, channel, relay.port, "PersonaSetEntityData")
}
pub(crate) fn extract<S: Source>(
    g: &mut Graph<S>,
    area: &Site,
    manager: &[Node],
    ports: Links,
) -> Result<Roster, Error> {
    let linked = g.links(area, EVENT_SPAWNER)?;
    if linked.is_empty() {
        return Ok(Roster::default());
    }
    let spawner = one(g, area, EVENT_SPAWNER, "SpawnAiForEventEntityData")?;
    extract_spawner(g, &spawner, manager, ports)
}

/// A story launcher may bind its spawner through the progression scene rather
/// than a free-roam start area. The roster semantics are identical.
pub(crate) fn extract_spawner<S: Source>(
    g: &mut Graph<S>,
    spawner: &Site,
    manager: &[Node],
    ports: Links,
) -> Result<Roster, Error> {
    let object = g.object(spawner)?;
    if object.class != "SpawnAiForEventEntityData" {
        return Err(bad("AI event spawner class"));
    }
    let completion = match object
        .fields
        .get("AiActionOnGameModeCompletion")
        .and_then(Value::as_enum_name)
    {
        Some("AiAction_UnspawnAllAiOnGameModeShutdown") => Completion::UnspawnAllAtShutdown,
        Some("AiAction_GiveAllAiToDirectorOnGameModeShutdown") => {
            Completion::GiveAllToDirectorAtShutdown
        }
        Some("AiAction_UnspawnIndividualAiOnGameModeCompletion") => {
            Completion::UnspawnIndividualAtCompletion
        }
        Some("AiAction_GiveIndividualAiToDirectorOnGameModeCompletion") => {
            Completion::GiveIndividualToDirectorAtCompletion
        }
        _ => return Err(bad("AI completion action")),
    };
    let libraries: Vec<_> = manager
        .iter()
        .filter(|n| n.object.class == "PersonalityLibraryEntityData")
        .collect();
    let [library] = libraries.as_slice() else {
        return Err(bad("AI personality library missing or ambiguous"));
    };
    let personalities = g.links(&library.site, ports.personalities)?;
    if personalities.len() > crate::rivals::MAX_PERSONALITIES {
        return Err(bad("AI personality bound"));
    }
    let mut personality_ids = BTreeMap::new();
    for (index, e) in personalities.iter().enumerate() {
        if e.port != 0
            || g.object(&e.site)?.class != "PersonalityEntityData"
            || personality_ids
                .insert(e.site.key.clone(), index as u16)
                .is_some()
        {
            return Err(bad("AI personality library identity"));
        }
    }
    let mut spawns = g.links(spawner, PRIORITY_SPAWNS)?;
    spawns.extend(g.links(spawner, SPAWNS)?);
    if spawns.len() > MAX_SPAWNS {
        return Err(bad("AI spawn count"));
    }
    let mut out = Roster {
        completion: Some(completion),
        ..Default::default()
    };
    let mut persona_ids = BTreeMap::new();
    let mut spawn_ids = BTreeSet::new();
    for spawn in spawns {
        let o = g.object(&spawn.site)?;
        if spawn.port != 0
            || o.class != "AiSpawnEntityData"
            || !spawn_ids.insert(spawn.site.clone())
        {
            return Err(bad("AI spawn identity"));
        }
        let team = match o.fields.get("Team") {
            Some(Value::Enum { value, .. }) => u8::try_from(*value).map_err(|_| bad("AI team"))?,
            _ => return Err(bad("AI team")),
        };
        let provider = one(
            g,
            &spawn.site,
            ports.provider,
            "UnionVehicleProviderEntityData",
        )?;
        let p = g.object(&provider)?;
        let required = tags(g, &provider.key, &p)?;
        let parent = provider_set(g, &provider, manager, ports)?;
        let members = g.links(&parent, ports.personas)?;
        if members.len() > MAX_PERSONAS {
            return Err(bad("AI persona count"));
        }
        let mut candidates = Vec::new();
        for (ordinal, member) in members.iter().enumerate() {
            let p = g.object(&member.site)?;
            if member.port != 0 || p.class != "PersonaEntityData" {
                return Err(bad("AI persona member"));
            }
            if !required.is_subset(&tags(g, &member.site.key, &p)?) {
                continue;
            }
            let identity = (parent.clone(), member.site.key.clone(), ordinal);
            let id = if let Some(&id) = persona_ids.get(&identity) {
                id
            } else {
                if out.personas.len() >= MAX_PERSONAS {
                    return Err(bad("AI roster persona bound"));
                }
                let mut parsed = persona(g, &member.site.key, ordinal)?;
                let personality = g.links(&member.site, ports.personality)?;
                if personality.len() > 1 {
                    return Err(bad("AI persona personality count"));
                }
                if let Some(e) = personality.first() {
                    parsed.personality_index = *personality_ids
                        .get(&e.site.key)
                        .ok_or_else(|| bad("AI persona personality missing"))?;
                }
                let skill = personality
                    .first()
                    .map(|e| {
                        let object = g.object(&e.site)?;
                        super::finite(
                            object.fields.get("PacingSkill"),
                            "AI personality pacing skill",
                        )
                    })
                    .transpose()?;
                let id = out.personas.len();
                out.personas.push(parsed);
                out.pacing_skills.push(skill);
                persona_ids.insert(identity, id);
                id
            };
            candidates.push(id);
        }
        if candidates.is_empty() {
            return Err(bad("AI provider has no matching personas"));
        }
        let locator = one(g, &spawn.site, SPAWN_LOCATION, "LocatorEntityData")?;
        let transform = location(g, &locator)?;
        out.spawns.push(Spawn {
            personas: candidates,
            position: transform.position,
            basis: transform.basis,
            team,
            once: boolean(&o, "CanOnlySpawnOnce")?,
            store_participant: boolean(&o, "StoreParticipantPointer")?,
            ignore_availability: boolean(&o, "IgnorePersonaAvailability")?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
