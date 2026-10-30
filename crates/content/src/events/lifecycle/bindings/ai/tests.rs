// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "r",
        0,
        "GameModeBlueprint",
        &[
            ("Objects", Value::Array((1..=7).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 0xf0d7_8ff6, local(2), 0),
                    edge(local(1), 0x9135_1a19, local(3), 0),
                    edge(local(1), 0x7073_de02, local(4), 10),
                    edge(local(1), 0x775e_e84e, local(4), 11),
                    edge(local(1), 0x7835_cd73, local(5), 0),
                    edge(local(3), 0xbdb1_8947, local(2), 0),
                    edge(local(3), 0xfd69_801e, local(6), 0),
                    edge(local(3), 0x5ea8_be53, local(7), 0),
                ]),
            ),
        ],
    );
    m.put(
        "r",
        1,
        "ControlAiInGameModeStateEntityData",
        &[
            ("ContextId", ptr("a", 0)),
            ("ApplyCommandsOneByOne", Value::Bool(false)),
            (
                "ApplyCommandsToParticipantsAlreadyInState",
                Value::Bool(true),
            ),
        ],
    );
    m.put("r", 2, "WaypointsGameModeStateEntityData", &[]);
    m.put("r", 3, "WaypointSeriesPacingGroupCommandEntityData", &[]);
    m.put(
        "r",
        4,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put("r", 5, "GameModeEntityData", &[]);
    m.put("r", 6, "ChannelWaypointListProviderEntityData", &[]);
    m.put("r", 7, "PacingScheduleProviderEntityData", &[]);
    m.put(
        "a",
        0,
        "AiContextId",
        &[("ContextHash", Value::UInt32(0xfedc_ba98))],
    );
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[(
            "Links",
            Value::Array(
                (10..=11)
                    .map(|id| Value::Struct(Fields(vec![("Id".into(), Value::Int32(id))])))
                    .collect(),
            ),
        )],
    );
    m
}

#[test]
fn shared_controller_retains_real_state_and_route_schedule_provider_identity() {
    let mut m = fixture();
    let mut g = Graph::new(&mut m);
    let nodes = g.read(Site::root("r")).unwrap();
    let state = nodes.iter().find(|n| n.site.key.1 == 2).unwrap();
    let fields = vec![(
        state.site.clone(),
        state.object.class.clone(),
        nfs_world::replication::entity::Kind::WaypointsState,
    )];
    let index = BTreeMap::from([(state.site.clone(), 0)]);
    let c = control(
        &mut g,
        nodes.iter().find(|n| n.site.key.1 == 1).unwrap(),
        &index,
        &fields,
    )
    .unwrap();
    assert_eq!(c.context_hash, 0xfedc_ba98);
    assert_eq!(
        c.states,
        [Endpoint {
            serializer: 0,
            port: 0
        }]
    );
    assert_eq!(
        c.command_manager,
        Property {
            channel: "c".into(),
            port: 10
        }
    );
    assert_eq!(c.game_mode.unwrap().identity.index, 5);
    let command = command(
        &mut g,
        nodes.iter().find(|n| n.site.key.1 == 3).unwrap(),
        &index,
        &fields,
    )
    .unwrap();
    assert!(matches!(command, Command::Pacing { state, route, schedule }
        if state.serializer == 0 && route.identity.index == 6 && schedule.identity.index == 7));
}

#[test]
fn shared_controller_rejects_untyped_context_and_manager_substitutions() {
    for (asset, object, replacement) in [
        ("a", 0, "StringId"),
        ("c", 0, "ParticipantStateEntityData"),
        ("r", 5, "ParticipantSetEntityData"),
    ] {
        let mut m = fixture();
        m.0.get_mut(&(asset.into(), object)).unwrap().class = replacement.into();
        let mut g = Graph::new(&mut m);
        let nodes = g.read(Site::root("r")).unwrap();
        let state = nodes.iter().find(|n| n.site.key.1 == 2).unwrap();
        let fields = vec![(
            state.site.clone(),
            state.object.class.clone(),
            nfs_world::replication::entity::Kind::WaypointsState,
        )];
        let index = BTreeMap::from([(state.site.clone(), 0)]);
        assert!(
            control(
                &mut g,
                nodes.iter().find(|n| n.site.key.1 == 1).unwrap(),
                &index,
                &fields
            )
            .is_err()
        );
    }
}
