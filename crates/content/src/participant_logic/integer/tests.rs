// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Fields;

fn site(i: usize) -> Site {
    Site {
        key: ("a".into(), i),
        ..Site::root("a")
    }
}
fn array(v: Vec<Value>) -> Value {
    Value::Array(v)
}
fn set(m: &mut Memory, i: usize, k: &str, v: Value) {
    let f = &mut m.0.get_mut(&("a".into(), i)).unwrap().fields.0;
    f.retain(|(n, _)| n != k);
    f.push((k.into(), v));
}
fn fields() -> Vec<(Site, Kind)> {
    vec![
        (site(1), Kind::Root),
        (site(2), Kind::RpcReference),
        (site(4), Kind::I32Property),
        (site(8), Kind::RpcReference),
    ]
}
fn base() -> (Memory, Rules) {
    let r = crate::BuildIdentity::supported().participant_logic;
    let p = &r.integer_ports;
    let b = &r.boolean_ports;
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            (
                "PropertyConnections",
                array(vec![edge(
                    local(5),
                    p["accessor_output"],
                    local(4),
                    p["synced_input"],
                )]),
            ),
            (
                "LinkConnections",
                array(vec![
                    edge(local(5), b["participant"], local(3), 0),
                    edge(local(5), p["property_link"], local(8), 991),
                    edge(local(3), b["provider_root"], local(1), 0),
                ]),
            ),
        ],
    );
    m.put("a", 1, "ParticipantLogicEntityData", &[]);
    for n in [2, 8] {
        m.put(
            "a",
            n,
            "LinkedSchematicChannelReferenceEntityData",
            &[("ChannelAsset", ptr("c", 0))],
        );
    }
    m.put("a", 3, "ParticipantLogicParticipantProviderEntityData", &[]);
    m.put("a", 4, "SyncedIntEntityData", &[]);
    m.put("a", 5, "IntParticipantPropertyAccessorEntityData", &[]);
    m.put(
        "c",
        0,
        "SchematicChannelAsset",
        &[(
            "Links",
            array(vec![Value::Struct(Fields(vec![
                ("Id".into(), Value::UInt32(991)),
                (
                    "LinkTypeHash".into(),
                    Value::UInt32(p["participant_property_type"]),
                ),
            ]))]),
        )],
    );
    (m, r)
}
fn program(m: &mut Memory, r: &Rules) -> Result<Program, Error> {
    Ok(compile(&mut Graph::new(m), &fields(), r)?
        .remove(0)
        .initial
        .unwrap())
}
#[test]
fn exact_channel_occurrence_and_current_participant_state() {
    let (mut m, r) = base();
    let p = program(&mut m, &r).unwrap();
    for value in [i32::MIN, -1, 0, 40, i32::MAX] {
        assert_eq!(
            p.evaluate(|port| {
                assert_eq!(
                    *port,
                    Port {
                        serializer: 3,
                        field: 991
                    }
                );
                Some(value)
            })
            .unwrap(),
            value
        );
    }
    assert!(p.evaluate(|_| None).is_err());
    let b = &r.boolean_ports;
    let i = &r.integer_ports;
    set(
        &mut m,
        0,
        "LinkConnections",
        array(vec![
            edge(local(5), b["participant"], local(3), 0),
            edge(local(5), i["property_link"], local(8), 991),
            edge(local(3), b["provider_root"], local(2), 0),
        ]),
    );
    assert!(program(&mut m, &r).is_err());
}
#[test]
fn disconnected_is_distinct_from_unknown_ambiguous_and_event_gated_sources() {
    let (mut m, r) = base();
    let p = &r.integer_ports;
    set(&mut m, 0, "PropertyConnections", array(vec![]));
    assert!(
        compile(&mut Graph::new(&mut m), &fields(), &r).unwrap()[0]
            .initial
            .is_none()
    );
    let e = edge(local(5), p["accessor_output"], local(4), p["synced_input"]);
    set(
        &mut m,
        0,
        "PropertyConnections",
        array(vec![e.clone(), e.clone()]),
    );
    assert!(program(&mut m, &r).is_err());
    set(&mut m, 0, "PropertyConnections", array(vec![e]));
    for class in ["UnsupportedData", "PropertyGateEntityData"] {
        m.put("a", 5, class, &[]);
        assert!(program(&mut m, &r).is_err());
    }
}
#[test]
fn channel_declarations_must_be_unique_and_typed() {
    for (count, wrong) in [(0, false), (2, false), (1, true)] {
        let (mut m, r) = base();
        let row = Value::Struct(Fields(vec![
            ("Id".into(), Value::UInt32(991)),
            (
                "LinkTypeHash".into(),
                Value::UInt32(if wrong {
                    1
                } else {
                    r.integer_ports["participant_property_type"]
                }),
            ),
        ]));
        m.put(
            "c",
            0,
            "SchematicChannelAsset",
            &[("Links", array(vec![row; count]))],
        );
        assert!(program(&mut m, &r).is_err());
    }
}
fn calculator(m: &mut Memory, r: &Rules, output: &str, values: Vec<Value>) {
    let p = &r.integer_ports;
    m.put(
        "a",
        6,
        "IntScoreToIntLevelEntityData",
        &[("LevelThresholdsAsset", ptr("t", 0))],
    );
    m.put(
        "t",
        0,
        "IntScoreToIntLevelAsset",
        &[("LevelThresholds", array(values))],
    );
    set(
        m,
        0,
        "PropertyConnections",
        array(vec![
            edge(local(5), p["accessor_output"], local(6), p["score_input"]),
            edge(local(6), p[output], local(4), p["synced_input"]),
        ]),
    );
}
#[test]
fn installed_thresholds_drive_signed_scores_boundaries_and_cap() {
    let (mut m, r) = base();
    for (port, index) in [
        ("level_output", 1),
        ("current_threshold_output", 2),
        ("next_threshold_output", 3),
    ] {
        calculator(
            &mut m,
            &r,
            port,
            vec![Value::Int32(0), Value::Int32(10), Value::Int32(25)],
        );
        let p = program(&mut m, &r).unwrap();
        for row in [
            [-1, 0, 0, 0],
            [0, 1, 0, 10],
            [9, 1, 0, 10],
            [10, 2, 10, 25],
            [25, 3, 25, 25],
            [i32::MAX, 3, 25, 25],
        ] {
            assert_eq!(p.evaluate(|_| Some(row[0])).unwrap(), row[index]);
        }
    }
}
#[test]
fn invalid_threshold_type_order_count_class_and_output_are_rejected() {
    for values in [
        vec![],
        vec![Value::UInt32(0)],
        vec![Value::Int32(1)],
        vec![Value::Int32(0), Value::Int32(0)],
        vec![Value::Int32(0); 257],
    ] {
        let (mut m, r) = base();
        calculator(&mut m, &r, "level_output", values);
        assert!(program(&mut m, &r).is_err());
    }
    let (mut m, r) = base();
    calculator(&mut m, &r, "level_output", vec![Value::Int32(0)]);
    m.0.get_mut(&("t".into(), 0)).unwrap().class = "WrongAsset".into();
    assert!(program(&mut m, &r).is_err());
    calculator(&mut m, &r, "int_value", vec![Value::Int32(0)]);
    assert!(program(&mut m, &r).is_err());
}
#[test]
fn constants_preserve_sign_and_forwarding_rejects_cycles_and_depth() {
    let (mut m, r) = base();
    let p = &r.integer_ports;
    m.put(
        "a",
        5,
        "IntEntityData",
        &[("DefaultValue", Value::Int32(-37))],
    );
    set(
        &mut m,
        0,
        "PropertyConnections",
        array(vec![edge(
            local(5),
            p["int_value"],
            local(4),
            p["synced_input"],
        )]),
    );
    assert_eq!(
        program(&mut m, &r).unwrap().evaluate(|_| None).unwrap(),
        -37
    );
    set(&mut m, 5, "DefaultValue", Value::UInt32(37));
    assert!(program(&mut m, &r).is_err());
    set(
        &mut m,
        0,
        "PropertyConnections",
        array(vec![edge(
            local(4),
            p["synced_output"],
            local(4),
            p["synced_input"],
        )]),
    );
    assert!(program(&mut m, &r).is_err());
    let mut edges = Vec::new();
    let mut layout = fields();
    for i in 10..40 {
        m.put("a", i, "SyncedIntEntityData", &[]);
        layout.push((site(i), Kind::I32Property));
        edges.push(edge(
            local(i + 1),
            p["synced_output"],
            local(i),
            p["synced_input"],
        ));
    }
    edges.push(edge(
        local(10),
        p["synced_output"],
        local(4),
        p["synced_input"],
    ));
    set(&mut m, 0, "PropertyConnections", array(edges));
    assert!(compile(&mut Graph::new(&mut m), &layout, &r).is_err());
}
