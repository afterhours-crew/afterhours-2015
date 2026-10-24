// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;
use nfs_world::scoring::nodes::{FLOAT_VALUE, IN, OUTPUT_VALUE};

fn links(ids: &[u32]) -> Value {
    Value::Array(
        ids.iter()
            .map(|&id| Value::Struct(Fields(vec![("Id".into(), Value::Int32(id as i32))])))
            .collect(),
    )
}

/// Root: an integer (4) writes the accessor's (1) Value; the accessor names
/// port 77 of channel `c` through a channel reference (2); a synchronized
/// integer (3) mirrors the accessor's output.
fn memory(identifiers: Vec<Value>, channel: &str, declared: &[u32]) -> Memory {
    let mut memory = Memory::default();
    memory.put(
        "r",
        0,
        "ParticipantLogicBlueprint",
        &[
            (
                "Objects",
                Value::Array(vec![local(1), local(2), local(3), local(4)]),
            ),
            ("LinkConnections", Value::Array(identifiers)),
            (
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(1), OUTPUT_VALUE, local(3), IN),
                    edge(local(4), FLOAT_VALUE, local(1), FLOAT_VALUE),
                ]),
            ),
        ],
    );
    memory.put("r", 1, ACCESSOR, &[("Value", Value::Int32(0))]);
    memory.put(
        "r",
        2,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    memory.put("r", 3, "SyncedIntEntityData", &[]);
    memory.put("r", 4, "IntEntityData", &[]);
    memory.put("c", 0, channel, &[("Links", links(declared))]);
    memory
}
fn identifier() -> Value {
    edge(local(1), IDENTIFIER, local(2), 77)
}

#[test]
fn set_accessor_resolves_shared_channel_port_and_own_provider_separately() {
    let mut source = memory(
        vec![
            edge(local(1), 772_610_520, local(2), 77),
            edge(local(1), PARTICIPANT_PROVIDER, local(5), 0),
        ],
        "SchematicChannelAsset",
        &[77],
    );
    source.put("r", 1, "ParticipantSetAccessorEntityData", &[]);
    source.put("r", 5, "ParticipantLogicParticipantProviderEntityData", &[]);
    let keys = sets(&mut source, "r").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0].1,
        Some(Property {
            channel: "c".into(),
            port: 77
        })
    );
    let providers = event_providers(&mut source, "r").unwrap();
    assert_eq!(providers[0].1.as_ref().unwrap().index, 5);
    source.put("c", 0, "SchematicChannelAsset", &[("Links", links(&[]))]);
    assert!(sets(&mut source, "r").is_err());
}

#[test]
fn each_scalar_accessor_resolves_its_own_identifier_field() {
    for class in [
        ACCESSOR,
        "FloatParticipantPropertyAccessorEntityData",
        "BoolParticipantPropertyAccessorEntityData",
        "SortAndRankIntParticipantPropertyEntityData",
    ] {
        let identifier = accessor(class).map_or(233_976_021, |a| a.0);
        let mut source = memory(
            vec![
                edge(local(1), identifier, local(2), 77),
                edge(local(1), PARTICIPANT_PROVIDER, local(5), 0),
            ],
            "SchematicChannelAsset",
            &[77],
        );
        source.put("r", 1, class, &[]);
        source.put("r", 5, "ParticipantLogicParticipantProviderEntityData", &[]);
        let result = read_kinds(&mut source, "r", true, true).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].1,
            Property {
                channel: "c".into(),
                port: 77
            }
        );
        assert_eq!(
            read(&mut source, "r", true).unwrap().len(),
            usize::from(class == ACCESSOR)
        );
    }
}
fn site(memory: &mut Memory, index: usize) -> Site {
    Graph::new(memory)
        .read(Site::root("r"))
        .unwrap()
        .into_iter()
        .find(|n| n.site.key.1 == index)
        .unwrap()
        .site
}

#[test]
fn an_accessor_names_its_declared_channel_port_and_a_synced_integer_mirrors_it() {
    let mut source = memory(vec![identifier()], "SchematicChannelAsset", &[76, 77]);
    let expected = Property {
        channel: "c".into(),
        port: 77,
    };
    let accessor = site(&mut source, 1);
    assert_eq!(
        property(&mut Graph::new(&mut source), &accessor).unwrap(),
        expected
    );
    let synced = site(&mut source, 3);
    assert_eq!(
        mirrored(&mut Graph::new(&mut source), &synced).unwrap(),
        Some(expected)
    );
    // Other classes mirror nothing.
    let integer = site(&mut source, 4);
    assert_eq!(
        mirrored(&mut Graph::new(&mut source), &integer).unwrap(),
        None
    );
    assert!(property(&mut Graph::new(&mut source), &integer).is_err());
}

#[test]
fn undeclared_ambiguous_or_foreign_identifiers_fail() {
    for mut source in [
        // The channel does not declare the port.
        memory(vec![identifier()], "SchematicChannelAsset", &[76]),
        // Two identifier links.
        memory(
            vec![identifier(), edge(local(1), IDENTIFIER, local(2), 76)],
            "SchematicChannelAsset",
            &[76, 77],
        ),
        // No identifier link.
        memory(vec![], "SchematicChannelAsset", &[77]),
        // The reference names another asset class.
        memory(vec![identifier()], "LogicPrefabBlueprint", &[77]),
        // The identifier links to an object port of the reference.
        memory(
            vec![edge(local(1), IDENTIFIER, local(2), 0)],
            "SchematicChannelAsset",
            &[77],
        ),
    ] {
        let accessor = site(&mut source, 1);
        assert!(property(&mut Graph::new(&mut source), &accessor).is_err());
    }
}

#[test]
fn owned_accessors_require_one_enclosing_participant_provider() {
    for (providers, class, valid) in [
        (
            vec![edge(local(1), PARTICIPANT_PROVIDER, local(5), 0)],
            "ParticipantLogicParticipantProviderEntityData",
            true,
        ),
        (
            vec![],
            "ParticipantLogicParticipantProviderEntityData",
            false,
        ),
        (
            vec![edge(local(1), PARTICIPANT_PROVIDER, local(5), 0); 2],
            "ParticipantLogicParticipantProviderEntityData",
            false,
        ),
        (
            vec![edge(local(1), PARTICIPANT_PROVIDER, local(5), 1)],
            "ParticipantLogicParticipantProviderEntityData",
            false,
        ),
        (
            vec![edge(local(1), PARTICIPANT_PROVIDER, local(5), 0)],
            "ParticipantProviderEntityData",
            false,
        ),
    ] {
        let mut identifiers = vec![identifier()];
        identifiers.extend(providers);
        let mut source = memory(identifiers, "SchematicChannelAsset", &[77]);
        source.put("r", 5, class, &[]);
        let result = read(&mut source, "r", true);
        assert_eq!(result.is_ok(), valid, "{class}: {result:?}");
        if valid {
            assert_eq!(
                result.unwrap()[0].1,
                Property {
                    channel: "c".into(),
                    port: 77
                }
            );
        }
        // The descriptive API does not claim provider ownership.
        assert_eq!(read(&mut source, "r", false).unwrap().len(), 1);
    }
}
