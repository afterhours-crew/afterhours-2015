// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

fn site(asset: &str, index: usize) -> Site {
    Site {
        key: (asset.into(), index),
        bus: (asset.into(), 0),
        parents: vec![],
    }
}
fn schema(m: &mut Memory, ports: &[u32]) {
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[(
            "Links",
            Value::Array(
                ports
                    .iter()
                    .map(|p| Value::Struct(Fields(vec![("Id".into(), Value::UInt32(*p))])))
                    .collect(),
            ),
        )],
    );
}

#[test]
fn indexed_channel_properties_keep_native_order_repeats_and_filter_client_realm() {
    let mut m = Memory::default();
    let rows = [
        (11, 1, "Realm_Server"),
        (12, 0, "Realm_Client"),
        (13, 2, "Realm_ClientAndServer"),
    ]
    .into_iter()
    .map(|(id, value, name)| {
        Value::Struct(Fields(vec![
            ("Id".into(), Value::UInt32(id)),
            (
                "Realm".into(),
                Value::Enum {
                    value,
                    name: Some(name.into()),
                },
            ),
        ]))
    })
    .collect();
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[
            ("Properties", Value::Array(rows)),
            ("Links", Value::Array(vec![])),
        ],
    );
    m.put(
        "r",
        0,
        "Blueprint",
        &[("Objects", Value::Array(vec![local(1), local(2), local(3)]))],
    );
    m.put(
        "r",
        1,
        "LinkedSchematicChannelEntityData",
        &[
            ("ChannelAsset", ptr("c", 0)),
            (
                "InputProperties",
                Value::Array(vec![
                    Value::UInt32(2),
                    Value::UInt32(0),
                    Value::UInt32(2),
                    Value::UInt32(1),
                ]),
            ),
            ("InputRefProperties", Value::Array(vec![])),
        ],
    );
    m.put(
        "r",
        2,
        "LinkedSchematicChannelReferenceEntityData",
        &[
            ("ChannelAsset", ptr("c", 0)),
            ("OutputProperties", Value::Array(vec![Value::UInt32(0)])),
            ("OutputRefProperties", Value::Array(vec![])),
        ],
    );
    m.put(
        "r",
        3,
        "LinkedSchematicChannelRelayEntityData",
        &[
            ("ChannelAsset", ptr("c", 0)),
            (
                "OutputProperties",
                Value::Array(vec![Value::UInt32(2), Value::UInt32(0)]),
            ),
            ("OutputRefProperties", Value::Array(vec![])),
        ],
    );
    let defs = definitions(&mut m, "r").unwrap();
    assert_eq!(
        defs[&Identity::from(&site("r", 1))].properties,
        [13, 11, 13]
    );
    assert!(defs[&Identity::from(&site("r", 2))].reference);
    assert_eq!(defs[&Identity::from(&site("r", 2))].properties, [11]);
    let relay = &defs[&Identity::from(&site("r", 3))];
    assert!(relay.relay && relay.reference);
    assert_eq!(relay.properties, [13, 11]);
    m.put(
        "r",
        2,
        "LinkedSchematicChannelReferenceEntityData",
        &[
            ("ChannelAsset", ptr("c", 0)),
            ("OutputProperties", Value::Array(vec![Value::Int32(-1)])),
            ("OutputRefProperties", Value::Array(vec![])),
        ],
    );
    assert!(definitions(&mut m, "r").is_err());
}

#[test]
fn bound_links_keep_owner_identity_empty_ports_repeats_and_source_order() {
    let mut m = Memory::default();
    m.put(
        "r",
        1,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put(
        "s",
        1,
        "LinkedSchematicChannelEntityData",
        &[("ChannelAsset", ptr("c", 0))],
    );
    m.put("s", 2, "IntParticipantPropertyEntityData", &[]);
    m.put("s", 3, "ParticipantStateEntityData", &[]);
    m.put(
        "s",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(1), 11, local(3), 0),
                edge(local(1), 11, local(2), 0),
                edge(local(1), 11, local(3), 0),
            ]),
        )],
    );
    schema(&mut m, &[11, 12]);
    let bindings = BTreeMap::from([(
        site("r", 1),
        (
            site("s", 1),
            Binding::Scene {
                content_key: 91,
                serializer: 7,
            },
        ),
    )]);
    let channels = read(&mut Graph::new(&mut m), &bindings).unwrap();
    assert_eq!(channels.len(), 1);
    let c = &channels[0];
    assert_eq!(
        c.owner,
        Binding::Scene {
            content_key: 91,
            serializer: 7
        }
    );
    assert_eq!(c.reference.asset, "r");
    assert_eq!(c.definition.asset, "s");
    assert_eq!(
        c.links[&11]
            .iter()
            .map(|t| t.identity.index)
            .collect::<Vec<_>>(),
        [3, 2, 3]
    );
    assert_eq!(c.links[&11][1].class, "IntParticipantPropertyEntityData");
    assert!(c.links[&12].is_empty());
    schema(&mut m, &[11, 11]);
    assert!(read(&mut Graph::new(&mut m), &bindings).is_err());
    schema(&mut m, &[0]);
    assert!(read(&mut Graph::new(&mut m), &bindings).is_err());
    schema(&mut m, &[11]);
    m.put(
        "s",
        1,
        "LinkedSchematicChannelEntityData",
        &[("ChannelAsset", ptr("d", 0))],
    );
    assert!(read(&mut Graph::new(&mut m), &bindings).is_err());
}
