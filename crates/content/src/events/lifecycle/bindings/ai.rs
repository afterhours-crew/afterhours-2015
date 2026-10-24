// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared-mode controllers use real lifecycle states, not utility-local states.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Control {
    pub identity: Identity,
    pub context_hash: u32,
    pub one_by_one: bool,
    pub apply_existing: bool,
    pub states: Vec<Endpoint>,
    pub commands: Vec<Identity>,
    pub command_manager: Property,
    pub actor_manager: Property,
    /// Only the derived game-mode controller observes this entity's lifetime.
    pub game_mode: Option<Link>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Waypoints {
        state: Endpoint,
        route: Link,
        drift: bool,
    },
    Pacing {
        state: Endpoint,
        route: Link,
        schedule: Link,
    },
    Wander {
        urgent: bool,
        prefer_straight: bool,
    },
}

pub(super) fn control<S: Source>(
    graph: &mut Graph<S>,
    node: &Node,
    index: &BTreeMap<Site, usize>,
    fields: &[FieldSite],
) -> Result<Control, Error> {
    let key = graph.target_field(&node.site, "ContextId")?;
    let context = graph.source.object(&key)?;
    if context.class != "AiContextId" {
        return Err(bad("event AI context class"));
    }
    let context_hash = crate::graph::bits32(
        context
            .fields
            .get("ContextHash")
            .ok_or_else(|| bad("event AI context hash"))?,
    )?;
    let states = endpoints(graph, &node.site, 0xf0d7_8ff6, index, fields)?;
    let links = graph.links(&node.site, 0x9135_1a19)?;
    if links.len() > nfs_world::scoring::ai_commands::MAX_COMMANDS || states.len() > MAX_STATES {
        return Err(bad("event AI controller bounds"));
    }
    let commands = links
        .into_iter()
        .map(|e| {
            if e.port != 0
                || !matches!(
                    graph.object(&e.site)?.class.as_str(),
                    "WaypointChannelGotoWaypointCommandEntityData"
                        | "WaypointSeriesPacingGroupCommandEntityData"
                        | "WanderCommandEntityData"
                )
            {
                return Err(bad("event AI command class"));
            }
            Ok(Identity::from(&e.site))
        })
        .collect::<Result<_, _>>()?;
    let game_mode = if node.object.class == "ControlAiInGameModeStateEntityData" {
        let link = controller(graph, &node.site, 0x7835_cd73)?;
        if link.as_ref().is_some_and(|l| {
            l.channel.is_none() && (l.port != 0 || l.class != "GameModeEntityData")
        }) {
            return Err(bad("event AI game-mode link"));
        }
        link
    } else {
        None
    };
    Ok(Control {
        identity: Identity::from(&node.site),
        context_hash,
        one_by_one: boolean(&node.object, "ApplyCommandsOneByOne")?,
        apply_existing: boolean(&node.object, "ApplyCommandsToParticipantsAlreadyInState")?,
        states,
        commands,
        command_manager: identifier(graph, &node.site, 0x7073_de02)?,
        actor_manager: identifier(graph, &node.site, 0x775e_e84e)?,
        game_mode,
    })
}

pub(super) fn command<S: Source>(
    graph: &mut Graph<S>,
    node: &Node,
    index: &BTreeMap<Site, usize>,
    fields: &[FieldSite],
) -> Result<Command, Error> {
    if node.object.class == "WanderCommandEntityData" {
        return Ok(Command::Wander {
            urgent: boolean(&node.object, "IsUrgent")?,
            prefer_straight: boolean(&node.object, "PreferDriveStraightAtIntersection")?,
        });
    }
    let states = endpoints(graph, &node.site, 0xbdb1_8947, index, fields)?;
    let [state] = states.as_slice() else {
        return Err(bad("event AI waypoint state"));
    };
    let route = controller(graph, &node.site, 0xfd69_801e)?.ok_or_else(|| bad("event AI route"))?;
    if route.channel.is_none()
        && (route.port != 0 || route.class != "ChannelWaypointListProviderEntityData")
    {
        return Err(bad("event AI route class"));
    }
    Ok(match node.object.class.as_str() {
        "WaypointChannelGotoWaypointCommandEntityData" => Command::Waypoints {
            state: *state,
            route,
            drift: boolean(&node.object, "DoDrift")?,
        },
        "WaypointSeriesPacingGroupCommandEntityData" => {
            let schedule = controller(graph, &node.site, 0x5ea8_be53)?
                .ok_or_else(|| bad("event AI pacing schedule"))?;
            if schedule.channel.is_none()
                && (schedule.port != 0 || schedule.class != "PacingScheduleProviderEntityData")
            {
                return Err(bad("event AI pacing schedule class"));
            }
            Command::Pacing {
                state: *state,
                route,
                schedule,
            }
        }
        _ => return Err(bad("event AI command class")),
    })
}

#[cfg(test)]
mod tests;
