// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The world volumes the official host puts the player's participant into
//! during the prologue (career start, E1194), resolved from the installation.
//! Every one is a `VolumeParticipantSetFilterEntityData`: the event start areas'
//! start volumes (the normal-event roster), the activity volumes and the
//! collectible and spot volumes. The start areas' map visibility follows the
//! mission-switching notification. This catalog covers that eligibility
//! subset; other authored requirement notifications have their own predicate
//! state and cannot be inferred from POI visibility alone.
//! The families are named by the authored prefab that defines each filter,
//! as observed in the official capture; their lifecycle belongs to the host.
use nfs_content::{
    assets::{AssetIndex, scene_key},
    events::discovery::{Endpoint, Entry},
    world::StartupKind,
};
use std::collections::{BTreeMap, BTreeSet};

const VOLUME_FILTER: &str = "VolumeParticipantSetFilterEntityData";
/// Subscribed while the participant drives, outside the story's races.
const ACTIVITIES: [&str; 4] = [
    "gameplay/activities/driftcornerprefab",
    "gameplay/activities/speedrunprefab",
    "gameplay/progression/narrativelocation",
    "gameplay/progression/missionvolumes",
];
/// Added and removed again when the participant first drives.
const SPOTS: [&str; 5] = [
    "gameplay/activities/activitynotificationarea",
    "gameplay/collectibles/donutprefab",
    "gameplay/collectibles/vistaprefab",
    "gameplay/collectibles/donorcarprefab",
    "gameplay/collectibles/urbexprefab",
];
const MISSION_SWITCHING: [&str; 2] = [
    "gameplay/gamemodes/missionswitchingprefab",
    "gameplay/gamemodes/missionswitchingprefabprestige",
];
/// Bound on the endpoints of one family.
const MAX_ENDPOINTS: usize = 1024;

/// One installed scene serializer: its definition's class and prefab asset.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Binding {
    pub endpoint: Endpoint,
    pub class: String,
    pub asset: String,
}

/// An event's eligibility notification precedes its participant subscription.
/// Keep that relationship: independently sorting the two endpoint sets loses it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EventVolume {
    pub volume: Endpoint,
    pub notifications: Vec<Endpoint>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorldVolumes {
    /// Event start volumes (participant memberships).
    pub events: BTreeSet<Endpoint>,
    /// Activity volumes (participant memberships).
    pub activities: BTreeSet<Endpoint>,
    /// Collectible and spot volumes (a membership pulse on the first drive).
    pub spots: BTreeSet<Endpoint>,
    /// Start-area map notifications (scene switches).
    pub notifications: BTreeSet<Endpoint>,
    /// Ungated events first, then notification declaration order in the loaded
    /// scenes. Endpoint hashes and event names are identities, not order keys.
    pub event_order: Vec<EventVolume>,
    /// Driving subscriptions in activation-family and scene declaration order.
    pub driving_order: Vec<Endpoint>,
    /// `CopSpawnPatrolEntityData` serializers: the client's patrol requests
    /// (m0 `[scene, player]`, no arguments) get no host answer (official
    /// E1026, E912: no reply and no cop goal in the following 3 s).
    pub cop_patrols: BTreeSet<Endpoint>,
}

impl WorldVolumes {
    /// Read the level's scenes and the normal-event roster.
    pub fn prepare(index: &mut AssetIndex) -> Result<Self, nfs_content::Error> {
        let level = nfs_content::Options::default().identity.scenes.level;
        let roster = index.event_roster()?;
        let mut bindings = Vec::new();
        let startup = index.startup_entries(&level)?;
        for entry in startup.iter().filter(|e| e.kind == StartupKind::SubLevel) {
            let key = scene_key(&entry.name);
            for binding in index.scene_bindings(&entry.name)? {
                bindings.push(Binding {
                    endpoint: Endpoint {
                        scene_key: key,
                        serializer: binding.serializer,
                    },
                    class: binding.class,
                    asset: binding.asset,
                });
            }
        }
        Self::classify(&roster, &bindings)
    }

    pub fn classify(roster: &[Entry], bindings: &[Binding]) -> Result<Self, nfs_content::Error> {
        let mut volumes = Self {
            events: roster.iter().map(|entry| entry.volume).collect(),
            ..Self::default()
        };
        let assets: BTreeMap<Endpoint, &str> = bindings
            .iter()
            .map(|b| (b.endpoint, b.asset.as_str()))
            .collect();
        volumes.cop_patrols = bindings
            .iter()
            .filter(|b| b.class == "CopSpawnPatrolEntityData")
            .map(|b| b.endpoint)
            .collect();
        for binding in bindings.iter().filter(|b| b.class == VOLUME_FILTER) {
            if ACTIVITIES.contains(&binding.asset.as_str()) {
                volumes.activities.insert(binding.endpoint);
            } else if SPOTS.contains(&binding.asset.as_str()) {
                volumes.spots.insert(binding.endpoint);
            }
        }
        volumes.notifications = roster
            .iter()
            .flat_map(|entry| entry.notifications.iter().copied())
            .filter(|n| assets.get(n).is_some_and(|a| MISSION_SWITCHING.contains(a)))
            .collect();
        let order: BTreeMap<_, _> = bindings
            .iter()
            .enumerate()
            .map(|(ordinal, binding)| (binding.endpoint, ordinal))
            .collect();
        for entry in roster {
            let mut notifications: Vec<_> = entry
                .notifications
                .iter()
                .copied()
                .filter(|n| volumes.notifications.contains(n))
                .collect();
            notifications.sort_by_key(|n| order.get(n));
            if !order.contains_key(&entry.volume)
                || notifications.iter().any(|n| !order.contains_key(n))
            {
                return Err(nfs_content::Error::Content(
                    "world volume declaration is missing".into(),
                ));
            }
            volumes.event_order.push(EventVolume {
                volume: entry.volume,
                notifications,
            });
        }
        volumes.event_order.sort_by_key(|entry| {
            (
                !entry.notifications.is_empty(),
                order[entry.notifications.first().unwrap_or(&entry.volume)],
            )
        });
        // These activation families have distinct owned lifetimes. In
        // particular the first-drive spot pulse lies between the narrative
        // subscriptions and the automatic driving activities.
        for asset in [
            "gameplay/progression/narrativelocation",
            "gameplay/progression/missionvolumes",
            "gameplay/collectibles/donorcarprefab",
            "gameplay/collectibles/vistaprefab",
            "gameplay/collectibles/donutprefab",
            "gameplay/collectibles/urbexprefab",
            "gameplay/activities/activitynotificationarea",
            "gameplay/activities/speedrunprefab",
            "gameplay/activities/driftcornerprefab",
        ] {
            volumes.driving_order.extend(
                bindings
                    .iter()
                    .filter(|b| b.class == VOLUME_FILTER && b.asset == asset)
                    .map(|b| b.endpoint),
            );
        }
        if [
            &volumes.events,
            &volumes.activities,
            &volumes.spots,
            &volumes.notifications,
        ]
        .iter()
        .any(|family| family.len() > MAX_ENDPOINTS)
        {
            return Err(nfs_content::Error::Content("world volume bound".into()));
        }
        Ok(volumes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(scene_key: u32, serializer: usize) -> Endpoint {
        Endpoint {
            scene_key,
            serializer,
        }
    }
    fn binding(endpoint: Endpoint, class: &str, asset: &str) -> Binding {
        Binding {
            endpoint,
            class: class.into(),
            asset: asset.into(),
        }
    }
    fn entry(volume: Endpoint, notifications: Vec<Endpoint>) -> Entry {
        Entry {
            id: format!("gameplay/gamemodes/sprint_{}", volume.serializer),
            scene: "levels/genesis01/district01_sprintraces".into(),
            game_mode: "racegamemode".into(),
            volume,
            notifications,
        }
    }

    #[test]
    fn families_follow_the_authored_prefab_and_the_roster() {
        let roster = vec![
            // Mission switching present: only it switches the map.
            entry(at(7, 97), vec![at(7, 103), at(7, 151)]),
            // Only the event-logic notification (drag races): none switches.
            entry(at(7, 300), vec![at(7, 306)]),
        ];
        let bindings = vec![
            binding(at(7, 97), VOLUME_FILTER, "eventlogic(set)"),
            binding(at(7, 300), VOLUME_FILTER, "eventlogic(set)"),
            binding(
                at(7, 103),
                "SetClientNotificationEntityData",
                "eventlogic(set)",
            ),
            binding(
                at(7, 151),
                "SetClientNotificationEntityData",
                "gameplay/gamemodes/missionswitchingprefab",
            ),
            binding(
                at(7, 306),
                "SetClientNotificationEntityData",
                "eventlogic(set)",
            ),
            binding(
                at(8, 4),
                VOLUME_FILTER,
                "gameplay/activities/speedrunprefab",
            ),
            binding(at(8, 5), VOLUME_FILTER, "gameplay/collectibles/vistaprefab"),
            // Another filter (garage transition) and another class stay out.
            binding(at(8, 6), VOLUME_FILTER, "gameplay/garagetransitionarea"),
            binding(
                at(8, 7),
                "LinkedSchematicChannelEntityData",
                "gameplay/activities/speedrunprefab",
            ),
        ];
        let volumes = WorldVolumes::classify(&roster, &bindings).unwrap();
        assert_eq!(volumes.events, BTreeSet::from([at(7, 97), at(7, 300)]));
        assert_eq!(volumes.activities, BTreeSet::from([at(8, 4)]));
        assert_eq!(volumes.spots, BTreeSet::from([at(8, 5)]));
        assert_eq!(volumes.notifications, BTreeSet::from([at(7, 151)]));
        assert_eq!(volumes.event_order[0].volume, at(7, 300));
        assert_eq!(volumes.event_order[1].volume, at(7, 97));
        assert_eq!(volumes.event_order[1].notifications, [at(7, 151)]);
        assert_eq!(volumes.driving_order, [at(8, 5), at(8, 4)]);
    }

    #[test]
    fn event_publication_follows_notification_declarations_not_hashes_names_or_volumes() {
        let roster = vec![
            entry(at(7, 10), vec![at(7, 40)]),
            entry(at(90, 30), vec![at(90, 20)]),
            entry(at(90, 2), vec![at(90, 50)]),
        ];
        let bindings = vec![
            binding(at(90, 2), VOLUME_FILTER, "eventlogic(set)"),
            binding(
                at(90, 20),
                "SetClientNotificationEntityData",
                MISSION_SWITCHING[0],
            ),
            binding(at(90, 30), VOLUME_FILTER, "eventlogic(set)"),
            binding(
                at(90, 50),
                "SetClientNotificationEntityData",
                MISSION_SWITCHING[0],
            ),
            binding(at(7, 10), VOLUME_FILTER, "eventlogic(set)"),
            binding(
                at(7, 40),
                "SetClientNotificationEntityData",
                MISSION_SWITCHING[0],
            ),
        ];
        let volumes = WorldVolumes::classify(&roster, &bindings).unwrap();
        assert_eq!(
            volumes
                .event_order
                .iter()
                .map(|e| e.volume)
                .collect::<Vec<_>>(),
            [at(90, 30), at(90, 2), at(7, 10)]
        );
        let mut missing = bindings;
        missing.retain(|b| b.endpoint != at(90, 30));
        assert!(WorldVolumes::classify(&roster, &missing).is_err());
    }
}
