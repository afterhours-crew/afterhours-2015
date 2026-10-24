// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

fn channel(m: &mut Memory, ports: &[u32]) {
    m.put(
        "r",
        5,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[(
            "Links",
            Value::Array(
                ports
                    .iter()
                    .map(|&p| Value::Struct(Fields(vec![("Id".into(), Value::UInt32(p))])))
                    .collect(),
            ),
        )],
    );
}
fn id(i: usize) -> Identity {
    Identity {
        asset: "r".into(),
        index: i,
        instances: vec![],
    }
}

#[test]
fn dynamic_boolean_accessor_keeps_provider_source_and_authored_fallback() {
    let mut m = Memory::default();
    m.put(
        "r",
        0,
        "GameModeBlueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(2), local(5)])),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 0x969a_1637, local(5), 11),
                    edge(local(1), 0xb68b_0dbb, local(2), 0),
                ]),
            ),
        ],
    );
    m.put(
        "r",
        1,
        "BoolParticipantPropertyAccessorEntityData",
        &[("Value", Value::Bool(true))],
    );
    m.put(
        "r",
        2,
        "ParticipantEventToParticipantProviderEntityData",
        &[],
    );
    channel(&mut m, &[11]);
    let services = read(&mut m, "r").unwrap();
    assert!(
        matches!(&services[&id(1)], Service::BooleanAccessor { source, provider, default: true } if source.port==11 && provider==&id(2))
    );
    m.put("r", 2, "GameModeEntityData", &[]);
    assert!(read(&mut m, "r").is_err());
}

#[test]
fn shared_mode_context_and_spawn_retain_distinct_channels_and_exact_local_entities() {
    let mut m = Memory::default();
    m.put(
        "r",
        0,
        "GameModeBlueprint",
        &[
            ("Objects", Value::Array((1..=5).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(3), 0x1e5f_3d28, local(1), 0),
                    edge(local(3), 0xb611_c196, local(5), 11),
                    edge(local(3), 0xef9b_60e7, local(5), 12),
                    edge(local(4), 0x7835_cd73, local(1), 0),
                    edge(local(4), 0xf0d7_8ff6, local(2), 0),
                    edge(local(4), 0x153b_e35a, local(5), 13),
                    edge(local(4), 0x6ecb_e2bd, local(5), 14),
                    edge(local(4), 0xe2de_1081, local(5), 15),
                ]),
            ),
        ],
    );
    m.put("r", 1, "GameModeEntityData", &[]);
    m.put("r", 2, "WaitForAllAiToSpawnStateEntityData", &[]);
    m.put("r", 3, "GameModeParticipantContextEntityData", &[]);
    m.put("r", 4, "SpawnAiForGameModeStateEntityData", &[]);
    channel(&mut m, &[11, 12, 13, 14, 15]);
    let services = read(&mut m, "r").unwrap();
    assert_eq!(
        services[&id(3)],
        Service::Context {
            game_mode: id(1),
            manager: Property {
                channel: "c".into(),
                port: 11
            },
            categories: Property {
                channel: "c".into(),
                port: 12
            }
        }
    );
    assert!(
        matches!(&services[&id(4)], Service::SpawnAi { game_mode, state, roster, release, manager } if *game_mode==id(1) && *state==id(2) && roster.port==13 && release.port==14 && manager.port==15)
    );
    m.put("r", 2, "SimpleGameModeStateEntityData", &[]);
    assert!(read(&mut m, "r").is_err());
    m.put("r", 2, "WaitForAllAiToSpawnStateEntityData", &[]);
    channel(&mut m, &[11, 12, 13, 14]);
    assert!(read(&mut m, "r").is_err());
}

#[test]
fn shared_outro_and_notification_preserve_set_order_and_integer_source() {
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
                    edge(local(6), 0x1fc5_82cb, local(3), 0),
                    edge(local(6), 0x9408_53a5, local(1), 0),
                    edge(local(6), 0xfe2f_53e3, local(2), 0),
                    edge(local(6), 0xe79f_2f44, local(4), 0),
                    edge(local(6), 0xe2de_1081, local(5), 15),
                    edge(local(7), 0xfd32_724e, local(4), 0),
                ]),
            ),
        ],
    );
    for i in 1..=3 {
        m.put("r", i, "ParticipantSetEntityData", &[]);
    }
    m.put("r", 4, "IntParticipantPropertyEntityData", &[]);
    channel(&mut m, &[15]);
    m.put("r", 6, "EventOutroPlayerProviderEntityData", &[]);
    m.put(
        "r",
        7,
        "IntParticipantPropertyClientNotificationEntityData",
        &[],
    );
    let services = read(&mut m, "r").unwrap();
    assert!(
        matches!(&services[&id(6)], Service::Outro { sets, score, .. } if *sets==[SetSource::Local(id(3)), SetSource::Local(id(1)), SetSource::Local(id(2))] && *score==IntegerSource::Local(id(4)))
    );
    assert_eq!(
        services[&id(7)],
        Service::IntegerNotification {
            source: IntegerSource::Local(id(4))
        }
    );
    m.put("r", 4, "FloatParticipantPropertyEntityData", &[]);
    assert!(read(&mut m, "r").is_err());
}
