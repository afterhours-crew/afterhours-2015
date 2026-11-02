// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::{Fields, Value};

fn memory(provider: &str, omit: Option<u32>, duplicate: bool) -> Memory {
    let mut m = Memory::default();
    let services = [0x7eef_cb0c, 0x2824_4ad5, 0xb57a_a3e6, 0xa663_9088];
    let mut edges = vec![edge(local(1), 0xb68b_0dbb, local(2), 0)];
    for (i, field) in services.iter().enumerate() {
        if omit != Some(*field) {
            edges.push(edge(local(1), *field, local(3), 10 + i as u32));
        }
    }
    if duplicate {
        edges.push(edge(local(1), services[0], local(3), 10));
    }
    m.put(
        "r",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(2), local(3)])),
            ("LinkConnections", Value::Array(edges)),
        ],
    );
    m.put("r", 1, CLASS, &[]);
    m.put("r", 2, provider, &[]);
    m.put(
        "r",
        3,
        "LinkedSchematicChannelReferenceEntityData",
        &[("ChannelAsset", ptr("s", 0))],
    );
    m.put(
        "s",
        0,
        "SchematicChannelAsset",
        &[(
            "Links",
            Value::Array(
                (10..14)
                    .map(|v| Value::Struct(Fields(vec![("Id".into(), Value::UInt32(v))])))
                    .collect(),
            ),
        )],
    );
    m
}

#[test]
fn accessor_requires_own_provider_and_each_independent_declared_service() {
    let own = "ParticipantLogicParticipantProviderEntityData";
    let mut m = memory(own, None, false);
    let nodes = read(&mut m, "r").unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].provider.index, 2);
    assert_eq!(
        [
            &nodes[0].sources.pacing,
            &nodes[0].sources.actors,
            &nodes[0].sources.personalities,
            &nodes[0].sources.curves
        ]
        .map(|v| (v.channel.as_str(), v.port)),
        [("s", 10), ("s", 11), ("s", 12), ("s", 13)]
    );
    for missing in [0x7eef_cb0c, 0x2824_4ad5, 0xb57a_a3e6, 0xa663_9088] {
        assert!(read(&mut memory(own, Some(missing), false), "r").is_err());
    }
    assert!(read(&mut memory(own, None, true), "r").is_err());
    assert!(
        read(
            &mut memory("ParticipantProviderEntityData", None, false),
            "r"
        )
        .is_err()
    );
    m.put(
        "s",
        0,
        "SchematicChannelAsset",
        &[("Links", Value::Array(vec![]))],
    );
    assert!(read(&mut m, "r").is_err());
    let object = Object {
        class: CLASS.into(),
        guid: None,
        fields: Fields(vec![]),
    };
    assert_eq!(
        crate::scoring::creation::compile(&object).unwrap(),
        Some(crate::scoring::realms::Realm::Server)
    );
    let declaration = crate::scoring::property_types::compile(&object)
        .unwrap()
        .unwrap();
    assert_eq!(
        declaration.outputs,
        vec![(
            nfs_world::scoring::participant_pacing::NOS_REBUILD,
            nfs_world::scoring::properties::Kind::Float
        )]
    );
}

fn object(class: &str, rows: Value) -> Object {
    Object {
        class: class.into(),
        guid: None,
        fields: Fields(vec![("NOSRebuildPacingCurve".into(), rows)]),
    }
}
fn curve(top: f32) -> Value {
    Value::Array(
        [[0., 0., 1.], [1., top, 2.], [0., 0., 0.], [1., 1., 0.]]
            .into_iter()
            .map(|v| {
                Value::Struct(Fields(vec![
                    ("x".into(), Value::Float32(v[0])),
                    ("y".into(), Value::Float32(v[1])),
                    ("z".into(), Value::Float32(v[2])),
                ]))
            })
            .collect(),
    )
}
#[test]
fn curve_libraries_preserve_empty_slots_and_reject_malformed_nonempty_data() {
    let personalities = [
        object("PersonalityEntityData", curve(4.)),
        object("PersonalityEntityData", Value::Array(vec![])),
    ];
    let fallback = object("PacingLibraryEntityData", curve(12.));
    let compiled = library(&personalities, Some(&fallback)).unwrap();
    assert_eq!(compiled.evaluate(Some(0), 0.5).unwrap(), 2.);
    assert_eq!(compiled.evaluate(Some(1), 0.5).unwrap(), 6.);
    assert_eq!(compiled.evaluate(None, 0.5).unwrap(), 0.);
    assert_eq!(
        library(&personalities, None)
            .unwrap()
            .evaluate(Some(1), 0.5)
            .unwrap(),
        0.
    );
    for malformed in [
        Value::Bool(false),
        Value::Array(vec![Value::Float32(0.)]),
        curve(f32::NAN),
    ] {
        assert!(
            library(
                &[object("PersonalityEntityData", malformed)],
                Some(&fallback)
            )
            .is_err()
        );
    }
    assert!(library(std::slice::from_ref(&fallback), None).is_err());
    assert!(library(&personalities, Some(&personalities[0])).is_err());
    assert!(library(&vec![personalities[1].clone(); MAX_PERSONALITIES + 1], None).is_err());
}
