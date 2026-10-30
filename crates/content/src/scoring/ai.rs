// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! AI utility blueprints: strict local state/command and declared manager links.
use super::{
    boolean,
    events::Identity,
    field,
    participant_properties::{channel_endpoint, identifier},
    scalar,
};
use crate::{
    Error,
    graph::{Graph, Site, Source, bad, bits32},
};
use nfs_frostbite::ebx::Value;
use nfs_world::scoring::{
    ai_commands::{Control, Definition, Enable, Goto, MAX_COMMANDS, Scripted},
    ai_control::Inclusion,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub definition: Definition<Identity>,
}

impl crate::assets::AssetIndex {
    /// Inclusion filters can reject a participant before resolving commands or
    /// manager links owned by a different runtime (for example AI race members).
    pub fn scoring_ai_inclusions(
        &mut self,
        blueprint: &str,
    ) -> Result<Vec<(Identity, Inclusion)>, Error> {
        inclusions(&mut self.assets, blueprint)
    }
}

fn inclusion(f: &nfs_frostbite::ebx::Fields) -> Result<Inclusion, Error> {
    match field(f, "PlayerInclusionMode")? {
        Value::Enum {
            value: 0,
            name: Some(n),
        } if n == "PlayerInclusion_OnlyHuman" => Ok(Inclusion::Human),
        Value::Enum {
            value: 1,
            name: Some(n),
        } if n == "PlayerInclusion_OnlyAi" => Ok(Inclusion::Ai),
        Value::Enum {
            value: 2,
            name: Some(n),
        } if n == "PlayerInclusion_All" => Ok(Inclusion::All),
        _ => Err(bad("AI player inclusion enum")),
    }
}
fn inclusions<S: Source>(
    source: &mut S,
    blueprint: &str,
) -> Result<Vec<(Identity, Inclusion)>, Error> {
    Graph::new(source)
        .read(Site::root(blueprint))?
        .into_iter()
        .filter(|n| n.object.class == "EnableAiOnParticipantEntityData")
        .map(|n| Ok((Identity::from(&n.site), inclusion(&n.object.fields)?)))
        .collect()
}

fn local<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    classes: &[&str],
) -> Result<Vec<Identity>, Error> {
    let links = graph.links(site, port)?;
    if links.len() > MAX_COMMANDS {
        return Err(bad("AI link count"));
    }
    links
        .into_iter()
        .map(|e| {
            if e.port != 0 || !classes.contains(&graph.object(&e.site)?.class.as_str()) {
                return Err(bad("AI local entity link"));
            }
            Ok(Identity::from(&e.site))
        })
        .collect()
}
pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = vec![];
    for n in graph.read(Site::root(blueprint))? {
        let f = &n.object.fields;
        let definition = match n.object.class.as_str() {
            "ParticipantStateEntityData" => Definition::State {
                handles_marked_for_delete: boolean(f, "HandlesMarkedForDelete")?,
                send_rpc: boolean(f, "SendRpcToClientSide")?,
            },
            "EnableAiOnParticipantEntityData" => {
                let inclusion = inclusion(f)?;
                let states = local(
                    &mut graph,
                    &n.site,
                    0xe5c2_fc2d,
                    &["ParticipantStateEntityData"],
                )?;
                if states.len() > 1 {
                    return Err(bad("AI enable state count"));
                }
                Definition::Enable(Enable {
                    inclusion,
                    add_on_enable: boolean(f, "AddParticipantToStateOnEnableAi")?,
                    remove_on_disable: boolean(f, "RemoveParticipantFromStateOnDisableAi")?,
                    state: states.first().cloned(),
                    actor_manager: identifier(&mut graph, &n.site, 0x2824_4ad5)?,
                    ai_manager: identifier(&mut graph, &n.site, 0xe2de_1081)?,
                })
            }
            "ControlAiInGameModeStateEntityData" => {
                let key = graph.target_field(&n.site, "ContextId")?;
                let context = graph.source.object(&key)?;
                if context.class != "AiContextId" {
                    return Err(bad("AI context asset class"));
                }
                Definition::Control(Control {
                    context_hash: bits32(field(&context.fields, "ContextHash")?)?,
                    one_by_one: boolean(f, "ApplyCommandsOneByOne")?,
                    apply_existing: boolean(f, "ApplyCommandsToParticipantsAlreadyInState")?,
                    states: local(
                        &mut graph,
                        &n.site,
                        0xf0d7_8ff6,
                        &["ParticipantStateEntityData"],
                    )?,
                    commands: local(
                        &mut graph,
                        &n.site,
                        0x9135_1a19,
                        &[
                            "GotoWaypointCommandEntityData",
                            "ScriptedInputCommandEntityData",
                        ],
                    )?,
                    command_manager: identifier(&mut graph, &n.site, 0x7073_de02)?,
                    actor_manager: identifier(&mut graph, &n.site, 0x775e_e84e)?,
                    context_manager: identifier(&mut graph, &n.site, 0x7835_cd73)?,
                })
            }
            "GotoWaypointCommandEntityData" => {
                let links = graph.links(&n.site, 0x75c6_5578)?;
                let location = match links.as_slice() {
                    [] => None,
                    [link] => Some(channel_endpoint(&mut graph, link)?),
                    _ => return Err(bad("AI goto location count")),
                };
                let precision = graph.links(&n.site, 0x5f93_70e3)?;
                let (radius, stop) = match precision.as_slice() {
                    [] => (150., false),
                    [link] if link.port == 0 => {
                        let data = graph.object(&link.site)?;
                        if data.class != "GotoWaypointCommandPrecisionEntityData" {
                            return Err(bad("AI precision class"));
                        }
                        (
                            scalar(&data.fields, "LocationReachedRadius")?,
                            boolean(&data.fields, "StopAtWaypoint")?,
                        )
                    }
                    _ => return Err(bad("AI precision count")),
                };
                let fields = field(f, "LocationTransform")?
                    .as_fields()
                    .ok_or_else(|| bad("AI transform"))?;
                let mut transform = [[0.; 3]; 4];
                for (v, name) in transform
                    .iter_mut()
                    .zip(["right", "up", "forward", "trans"])
                {
                    let vector = field(fields, name)?
                        .as_fields()
                        .ok_or_else(|| bad("AI transform vector"))?;
                    for (x, name) in v.iter_mut().zip(["x", "y", "z"]) {
                        *x = scalar(vector, name)?;
                    }
                }
                Definition::Goto(Goto {
                    location,
                    transform,
                    drift: boolean(f, "DoDrift")?,
                    stop,
                    radius,
                })
            }
            "ScriptedInputCommandEntityData" => {
                if !graph.links(&n.site, 0xbc0e_49ac)?.is_empty() {
                    return Err(bad("scripted AI input curves are not modeled"));
                }
                Definition::Scripted(Scripted {
                    throttle: scalar(f, "Throttle")?,
                    steering: scalar(f, "Steering")?,
                    brake: scalar(f, "Brake")?,
                    handbrake: scalar(f, "HandBrake")?,
                    use_nos: boolean(f, "UseNos")?,
                })
            }
            _ => continue,
        };
        result.push(Node {
            identity: Identity::from(&n.site),
            definition,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
