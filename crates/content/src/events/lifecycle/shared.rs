// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared-mode services owned by the event host. Reading these bindings does
//! not implement their native callbacks or establish playable mode coverage.
use super::*;
use crate::scoring::{events::Identity, participant_properties::identifier};
use bindings::{IntegerSource, Link, SetSource, controller, integer_source, set_source};

#[derive(Clone, Debug, PartialEq)]
pub enum Service {
    BooleanAccessor {
        source: Property,
        provider: Identity,
        default: bool,
    },
    UpdateSet,
    ParticipantEventProvider,
    BooleanNotification {
        source: Identity,
    },
    VolumeFilter {
        participants: Property,
        volumes: Property,
    },
    FloatProperty,
    BooleanProperty,
    ParticipantEventChannel,
    Waypoints {
        reference: Identity,
    },
    Pacing {
        schedule: (String, usize),
        player_relative: bool,
        drift_train: bool,
    },
    Context {
        game_mode: Identity,
        manager: Property,
        categories: Property,
    },
    SpawnAi {
        game_mode: Identity,
        state: Identity,
        roster: Property,
        release: Property,
        manager: Property,
    },
    Outro {
        sets: [SetSource; 3],
        score: IntegerSource,
        manager: Property,
    },
    EventInfo {
        active: Link,
        position: IntegerSource,
    },
    IntegerNotification {
        source: IntegerSource,
    },
    RecentPlayers {
        manager: Property,
    },
    ResetRoute {
        state: Identity,
        managers: [Property; 3],
        configuration: nfs_world::crash::route::Config,
    },
    WaypointTelemetry {
        state: Identity,
        properties: [IntegerSource; 3],
    },
    PlatformSession {
        player_session_on_init: bool,
    },
}

fn local<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    class: &str,
) -> Result<Identity, Error> {
    let links = graph.links(site, port)?;
    let [link] = links.as_slice() else {
        return Err(bad("shared event local link count"));
    };
    if link.port != 0 || graph.object(&link.site)?.class != class {
        return Err(bad("shared event local link class"));
    }
    Ok(Identity::from(&link.site))
}
fn integer<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
) -> Result<IntegerSource, Error> {
    integer_source(graph, site, port)?.ok_or_else(|| bad("shared event integer link missing"))
}

pub(crate) fn read<S: Source>(
    source: &mut S,
    blueprint: &str,
) -> Result<BTreeMap<Identity, Service>, Error> {
    let mut graph = Graph::new(source);
    let nodes = graph.read(Site::root(blueprint))?;
    let mut result = BTreeMap::new();
    for node in &nodes {
        let site = &node.site;
        let service = match node.object.class.as_str() {
            "BoolParticipantPropertyAccessorEntityData" => Service::BooleanAccessor {
                source: identifier(&mut graph, site, 0x969a_1637)?,
                provider: local(
                    &mut graph,
                    site,
                    0xb68b_0dbb,
                    "ParticipantEventToParticipantProviderEntityData",
                )?,
                default: boolean(&node.object, "Value")?,
            },
            "UpdateSetEntityData" => Service::UpdateSet,
            "ParticipantEventToParticipantProviderEntityData" => Service::ParticipantEventProvider,
            "BoolPropertyClientNotificationEntityData" => Service::BooleanNotification {
                source: local(
                    &mut graph,
                    site,
                    0x969a_1637,
                    "BoolParticipantPropertyEntityData",
                )?,
            },
            "VolumeParticipantSetFilterEntityData" => Service::VolumeFilter {
                participants: identifier(&mut graph, site, 0x5fd7_137a)?,
                volumes: identifier(&mut graph, site, 0x0dd0_46ea)?,
            },
            "FloatParticipantPropertyEntityData" => Service::FloatProperty,
            "BoolParticipantPropertyEntityData" => Service::BooleanProperty,
            "ParticipantEventChannelEntityData" => Service::ParticipantEventChannel,
            "ChannelWaypointListProviderEntityData" => Service::Waypoints {
                reference: local(
                    &mut graph,
                    site,
                    0x87b3_1017,
                    "WaypointListChannelReferenceEntityData",
                )?,
            },
            "PacingScheduleProviderEntityData" => {
                let schedule = graph.target_field(site, "Schedule")?;
                if !matches!(
                    graph.source.object(&schedule)?.class.as_str(),
                    "RacePacingScheduleGroup" | "TimeAttackPacingScheduleGroup"
                ) {
                    return Err(bad("shared event pacing schedule class"));
                }
                Service::Pacing {
                    schedule,
                    player_relative: boolean(&node.object, "PacingRelativeToPlayer")?,
                    drift_train: boolean(&node.object, "IsDriftTrain")?,
                }
            }
            "GameModeParticipantContextEntityData" => Service::Context {
                game_mode: local(&mut graph, site, 0x1e5f_3d28, "GameModeEntityData")?,
                manager: identifier(&mut graph, site, 0xb611_c196)?,
                categories: identifier(&mut graph, site, 0xef9b_60e7)?,
            },
            "SpawnAiForGameModeStateEntityData" => Service::SpawnAi {
                game_mode: local(&mut graph, site, 0x7835_cd73, "GameModeEntityData")?,
                state: local(
                    &mut graph,
                    site,
                    0xf0d7_8ff6,
                    "WaitForAllAiToSpawnStateEntityData",
                )?,
                roster: identifier(&mut graph, site, 0x153b_e35a)?,
                release: identifier(&mut graph, site, 0x6ecb_e2bd)?,
                manager: identifier(&mut graph, site, 0xe2de_1081)?,
            },
            "EventOutroPlayerProviderEntityData" => Service::Outro {
                sets: [0x1fc5_82cb, 0x9408_53a5, 0xfe2f_53e3]
                    .map(|port| set_source(&mut graph, site, port))
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| bad("outro set count"))?,
                score: integer(&mut graph, site, 0xe79f_2f44)?,
                manager: identifier(&mut graph, site, 0xe2de_1081)?,
            },
            "EventInfoPresentationDataCollectorEntityData" => {
                let active = controller(&mut graph, site, 0x456a_1331)?
                    .ok_or_else(|| bad("event info active property"))?;
                if active.channel.is_none()
                    && (active.port != 0
                        || !matches!(
                            active.class.as_str(),
                            "BoolParticipantPropertyEntityData"
                                | "ParticipantStateListBoolParticipantPropertyEntityData"
                                | "ParticipantStateRangeBoolParticipantPropertyEntityData"
                        ))
                {
                    return Err(bad("event info active property class"));
                }
                Service::EventInfo {
                    active,
                    position: integer(&mut graph, site, 0xf26a_8424)?,
                }
            }
            "IntParticipantPropertyClientNotificationEntityData" => Service::IntegerNotification {
                source: integer(&mut graph, site, 0xfd32_724e)?,
            },
            "ServerUpdateRecentPlayersEntityData" => Service::RecentPlayers {
                manager: identifier(&mut graph, site, 0x965e_d189)?,
            },
            "VehicleResetRouteEntityData" => Service::ResetRoute {
                state: local(
                    &mut graph,
                    site,
                    0x3886_36cf,
                    "WaypointsGameModeStateEntityData",
                )?,
                managers: [
                    identifier(&mut graph, site, 0x2e5f_409a)?,
                    identifier(&mut graph, site, 0x6f08_ee50)?,
                    identifier(&mut graph, site, 0x9a82_e7df)?,
                ],
                configuration: super::super::recovery::extract(std::slice::from_ref(node))?
                    .ok_or_else(|| bad("reset route configuration"))?,
            },
            "WaypointTelemetryEntityData" => Service::WaypointTelemetry {
                state: local(
                    &mut graph,
                    site,
                    0x2e87_9742,
                    "WaypointsGameModeStateEntityData",
                )?,
                properties: [
                    integer(&mut graph, site, 0x6e29_1b16)?,
                    integer(&mut graph, site, 0xb2df_32c8)?,
                    integer(&mut graph, site, 0xe4c3_ae41)?,
                ],
            },
            "DurangoEventListenerEntityData" => Service::PlatformSession {
                player_session_on_init: boolean(&node.object, "PlayerSessionOnInit")?,
            },
            _ => continue,
        };
        if result.len() >= 512 || result.insert(Identity::from(site), service).is_some() {
            return Err(bad("shared event service bound/identity"));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
