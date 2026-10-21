// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local};
use nfs_frostbite::ebx::Fields;
use nfs_world::race_pacing::Progress;

fn site(i: usize) -> Site {
    Site {
        key: ("a".into(), i),
        ..Site::root("a")
    }
}
fn rows(range: [f32; 2]) -> Value {
    Value::Array(
        [
            [0., range[0], 1.],
            [1., range[1], 2.],
            [0., 0., 0.],
            [1., 1., 0.],
        ]
        .into_iter()
        .map(|v| {
            Value::Struct(Fields(
                ["x", "y", "z"]
                    .into_iter()
                    .zip(v)
                    .map(|(k, v)| (k.into(), Value::Float32(v)))
                    .collect(),
            ))
        })
        .collect(),
    )
}
fn fixture() -> Memory {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            Value::Array(vec![
                edge(local(1), PROVIDER, local(2), 777),
                edge(local(3), 777, local(4), 0),
            ]),
        )],
    );
    m.put("a", 1, "WaypointSeriesPacingGroupCommandEntityData", &[]);
    m.put("a", 2, "LinkedSchematicChannelReferenceEntityData", &[]);
    m.put("a", 3, "LinkedSchematicChannelEntityData", &[]);
    m.put(
        "a",
        4,
        "PacingScheduleProviderEntityData",
        &[
            ("Schedule", local(5)),
            ("PacingRelativeToPlayer", Value::Bool(true)),
            ("IsDriftTrain", Value::Bool(false)),
        ],
    );
    m.put(
        "a",
        5,
        "RacePacingScheduleGroup",
        &[
            ("ExtraTargetOffset", Value::Float32(0.)),
            ("TargetOffsetScale", Value::Float32(1.)),
            (
                "AiOnlyPacingMode",
                Value::Enum {
                    value: 3,
                    name: Some("AiPacingMode_Average".into()),
                },
            ),
        ],
    );
    let o = m.0.get_mut(&("a".into(), 5)).unwrap();
    for count in 1..=MAX_OPPONENTS {
        for (rank, ordinal) in ORDINALS.iter().take(count).enumerate() {
            let suffix = if count == 1 {
                "1".into()
            } else {
                format!("{count}_{ordinal}")
            };
            o.fields
                .0
                .push((format!("TargetSchedule_{suffix}"), Value::Array(vec![])));
            o.fields.0.push((
                format!("SkillModifier_{suffix}"),
                rows([0., (count * 100 + rank + 1) as f32]),
            ));
        }
    }
    m
}
fn bindings() -> BTreeMap<Site, (Site, Binding)> {
    BTreeMap::from([(
        site(2),
        (
            site(3),
            Binding::Scene {
                content_key: 1,
                serializer: 2,
            },
        ),
    )])
}
fn read(
    m: &mut Memory,
    bindings: &BTreeMap<Site, (Site, Binding)>,
) -> Result<Option<Config>, Error> {
    let nodes = vec![Node {
        site: site(1),
        object: m.0[&("a".into(), 1)].clone(),
    }];
    extract(&mut Graph::new(m), &nodes, bindings)
}

#[test]
fn bound_provider_preserves_every_group_rank_and_empty_curves() {
    let c = read(&mut fixture(), &bindings()).unwrap().unwrap();
    assert_eq!(c.opponent_counts(), MAX_OPPONENTS);
    for count in 1..=MAX_OPPONENTS {
        for rank in 1..=count {
            let p = Progress {
                distance: 1.,
                total_distance: 10.,
                reference_distance: 0.,
                base_skill: 0.5,
            };
            assert_eq!(
                c.evaluate(count, rank, p).unwrap(),
                (count * 100 + rank) as f32 + 0.5
            );
        }
    }
}

#[test]
fn missing_binding_unsupported_mode_and_bad_curve_fail_explicitly() {
    let mut m = fixture();
    assert!(read(&mut m, &BTreeMap::new()).is_err());
    let o = m.0.get_mut(&("a".into(), 4)).unwrap();
    o.fields
        .0
        .iter_mut()
        .find(|(k, _)| k == "IsDriftTrain")
        .unwrap()
        .1 = Value::Bool(true);
    assert!(read(&mut m, &bindings()).is_err());
    let mut m = fixture();
    let o = m.0.get_mut(&("a".into(), 5)).unwrap();
    o.fields
        .0
        .iter_mut()
        .find(|(k, _)| k == "SkillModifier_5_Third")
        .unwrap()
        .1 = Value::Array(vec![Value::Float32(0.)]);
    assert!(read(&mut m, &bindings()).is_err());
    let mut m = fixture();
    let o = m.0.get_mut(&("a".into(), 0)).unwrap();
    let Value::Array(edges) = &mut o.fields.0[0].1 else {
        panic!()
    };
    edges.push(edge(local(3), 777, local(4), 0));
    assert!(read(&mut m, &bindings()).is_err());
}
