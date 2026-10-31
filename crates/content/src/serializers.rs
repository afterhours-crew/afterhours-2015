// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Serializer profile of one installed blueprint under its family's class
//! rules, independently of the scene, owner or service that spawns it. A
//! decoder of whole sessions needs the bit layout of every created entity;
//! spawning still goes through the family layout builders, which also bind
//! channels. Channel binding does not change bit boundaries.
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
    participant_logic::Rules,
};
use nfs_world::replication::entity::{Kind, Profile};

impl AssetIndex {
    /// Families: participant logic, participant and vehicle state sequences,
    /// game modes. Any class outside the family rules fails; nothing is guessed.
    pub fn serializer_profile(&mut self, blueprint: &str) -> Result<Profile, Error> {
        let name = blueprint.to_lowercase();
        if name.is_empty() || name.len() > 512 || name.contains("..") {
            return Err(bad("blueprint asset name bound"));
        }
        let participant = Rules::from_json(&self.identity.participant_logic.to_json())?;
        let sequences = self.identity.sequences.clone();
        let nodes = Graph::new(&mut self.assets).read(Site::root(&name))?;
        let kinds: Vec<Kind> = match nodes.first().map(|n| n.object.class.as_str()) {
            Some("ParticipantLogicBlueprint") => {
                crate::participant_logic::shape(&nodes, &participant)?
                    .1
                    .into_iter()
                    .map(|(_, kind)| kind)
                    .collect()
            }
            Some("ParticipantStateSequenceBlueprint" | "VehicleStateSequenceBlueprint") => {
                crate::sequences::authored::shape(&nodes, &sequences)?
                    .1
                    .into_iter()
                    .map(|(_, _, kind)| kind)
                    .collect()
            }
            Some("GameModeBlueprint") => {
                crate::events::fields(&nodes, &participant, "GameModeEntityData")?
                    .1
                    .into_iter()
                    .map(|(_, _, kind)| kind)
                    .collect()
            }
            _ => return Err(bad("blueprint has no serializer family")),
        };
        Profile::new(&kinds).map_err(|_| bad("serializer profile"))
    }
}

#[cfg(test)]
mod tests {
    use crate::{assets::AssetIndex, synthetic};
    use nfs_frostbite::synthetic::{Asset as Fixture, Field as F, PartitionWriter};
    use nfs_world::replication::entity::Kind;

    fn blueprint(seed: u8, name: &str, class: &str, objects: Vec<&str>) -> Fixture {
        Fixture {
            name: name.into(),
            partition: PartitionWriter::default().build_many(
                [seed; 16],
                class,
                [seed + 100; 16],
                vec![
                    ("Flags", F::UInt(1)),
                    (
                        "Objects",
                        F::RawPointers((2..objects.len() as u32 + 2).collect()),
                    ),
                ],
                objects
                    .into_iter()
                    .map(|c| {
                        let fields = if c == "UIWidgetBlueprint" {
                            vec![("Flags", F::UInt(0))]
                        } else {
                            vec![]
                        };
                        (c, fields)
                    })
                    .collect(),
            ),
        }
    }
    struct Installation(std::path::PathBuf);
    impl Drop for Installation {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn each_family_uses_its_own_rules_and_unknown_compositions_fail() {
        let assets = vec![
            blueprint(
                1,
                "test/logic",
                "ParticipantLogicBlueprint",
                vec![
                    "ParticipantLogicEntityData",
                    "LinkedSchematicChannelReferenceEntityData",
                ],
            ),
            blueprint(
                2,
                "test/sequence",
                "ParticipantStateSequenceBlueprint",
                vec![
                    "ParticipantStateSequenceEntityData",
                    "LinkedSchematicChannelReferenceEntityData",
                    "StateSequenceParticipantProviderEntityData",
                ],
            ),
            blueprint(
                3,
                "test/mode",
                "GameModeBlueprint",
                vec![
                    "GameModeEntityData",
                    "CountdownGameModeStateEntityData",
                    "SimpleGameModeStateEntityData",
                ],
            ),
            blueprint(
                4,
                "test/unknown",
                "ParticipantLogicBlueprint",
                vec!["ParticipantLogicEntityData", "NotARuledClassEntityData"],
            ),
            blueprint(5, "test/other", "SpatialPrefabBlueprint", vec![]),
            blueprint(
                6,
                "test/inspection",
                "GameModeBlueprint",
                vec![
                    "GameModeEntityData",
                    "ControlCopRacerAiInGameModeStateEntityData",
                    "EscapeGameModeStateEntityData",
                    "CopAudioInterfaceEntityData",
                    "ChaseInterfaceEntityData",
                    "CopFineTrackerEntityData",
                    "DisplayCopsOnMapEntityData",
                    "TargetVisibilityEntityData",
                    "CooldownGameModeStateEntityData",
                    "SyncedTimerEntityData",
                    "SurrenderPromptSenderEntityData",
                ],
            ),
            blueprint(
                7,
                "test/local",
                "ParticipantLogicBlueprint",
                vec![
                    "ParticipantLogicEntityData",
                    "RaceVehicleRoadblockCollisionEntityData",
                    "IllegalActivityInterfaceEntityData",
                    "Vec3PresentationDataCollectorEntityData",
                    "ConditionalVec3EntityData",
                    "UIWidgetBlueprint",
                    "StateSequenceSelectorEntityData",
                    "ParticipantEventEntityData",
                    "SyncedBoolEntityData",
                ],
            ),
        ];
        let temp = Installation(std::env::temp_dir().join(format!(
            "nfs-serializer-profiles-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )));
        let _ = std::fs::remove_dir_all(&temp.0);
        let (options, _) = synthetic::write(&temp.0, &assets).unwrap();
        let mut index = AssetIndex::open(&temp.0, &options).unwrap();
        let kinds = |index: &mut AssetIndex, name: &str| {
            index.serializer_profile(name).map(|p| p.kinds().to_vec())
        };
        assert_eq!(
            kinds(&mut index, "Test/Logic").unwrap(),
            [Kind::Root, Kind::RpcReference]
        );
        assert_eq!(
            kinds(&mut index, "test/sequence").unwrap(),
            [
                Kind::SequenceRoot,
                Kind::RpcReference,
                Kind::RpcOptionalReference
            ]
        );
        assert_eq!(
            kinds(&mut index, "test/mode").unwrap(),
            [
                Kind::GameModeRoot,
                Kind::CountdownState,
                Kind::RpcReferences
            ]
        );
        assert!(kinds(&mut index, "test/unknown").is_err());
        assert!(kinds(&mut index, "test/other").is_err());
        assert!(kinds(&mut index, "test/missing").is_err());
        assert!(kinds(&mut index, "").is_err());
        assert_eq!(
            kinds(&mut index, "test/inspection").unwrap(),
            [
                Kind::GameModeRoot,
                Kind::RpcReferences,
                Kind::RpcBoolUpdate,
                Kind::RpcChaseUpdate,
                Kind::RpcCopFineTracker,
                Kind::RpcCopMapUpdate,
                Kind::RpcTargetVisibilityUpdate,
                Kind::RpcReferences,
                Kind::Rpc,
            ]
        );
        assert_eq!(
            kinds(&mut index, "test/local").unwrap(),
            [Kind::Root, Kind::BoolProperty]
        );
    }
}
