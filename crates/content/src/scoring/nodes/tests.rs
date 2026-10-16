// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_frostbite::ebx::Fields;
fn object(class: &str, rows: &[(&str, Value)]) -> Object {
    Object {
        class: class.into(),
        guid: None,
        fields: Fields(
            rows.iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        ),
    }
}
#[test]
fn typed_defaults_flags_and_bound_cache_initialization() {
    let cache = compile(&object(
        "FloatCacheEntityData",
        &[("InValue", Value::Float32(9.0))],
    ))
    .unwrap();
    assert_eq!(cache.defaults, [(nodes::CACHE_INPUT, Scalar::Float(9.0))]);
    let state = nodes::Node::new(cache.definition).unwrap();
    assert_eq!(
        state
            .initial_outputs(|_| Some(Scalar::Float(27.0)))
            .unwrap(),
        [nodes::Effect::Publish(
            nodes::CACHE_OUTPUT,
            Scalar::Float(27.0)
        )]
    );
    let acc = compile(&object(
        "FloatAccumulatorEntityData",
        &[("Input", Value::Float32(2.0))],
    ))
    .unwrap();
    assert_eq!(acc.defaults, [(nodes::INPUT, Scalar::Float(2.0))]);
    for (class, field, definition) in [
        (
            "EventGateEntityData",
            "Default",
            Definition::Gate { open: true },
        ),
        (
            "EventSplitterEntityData",
            "RunOnce",
            Definition::Splitter { run_once: true },
        ),
    ] {
        assert_eq!(
            compile(&object(class, &[(field, Value::Bool(true))]))
                .unwrap()
                .definition,
            definition
        );
    }
    let float = compile(&object(
        "FloatEntityData",
        &[
            ("DefaultValue", Value::Float32(-2.0)),
            ("IncDecValue", Value::Float32(0.5)),
        ],
    ))
    .unwrap();
    assert_eq!(
        float.definition,
        Definition::Float {
            default: -2.0,
            step: 0.5
        }
    );
    for (class, a, b, kind, expected) in [
        (
            "CompareFloatEntityData",
            Value::Float32(-2.5),
            Value::Float32(1.0),
            Comparison::Float,
            Scalar::Float(-2.5),
        ),
        (
            "CompareIntEntityData",
            Value::Int32(-7),
            Value::Int32(i32::MAX),
            Comparison::Integer,
            Scalar::Integer(-7),
        ),
    ] {
        let config = compile(&object(
            class,
            &[
                ("A", a),
                ("B", b),
                ("TriggerOnStart", Value::Bool(true)),
                ("TriggerOnPropertyChange", Value::Bool(false)),
            ],
        ))
        .unwrap();
        assert_eq!(
            config.definition,
            Definition::Compare {
                kind,
                on_change: false,
                on_start: true
            }
        );
        assert_eq!(config.defaults[0], (nodes::A, expected));
    }
}
#[test]
fn reject_malformed_and_unsupported_nodes_instead_of_defaulting() {
    for obj in [
        object("FloatCacheEntityData", &[]),
        object(
            "FloatAccumulatorEntityData",
            &[("Input", Value::Float32(f32::INFINITY))],
        ),
        object(
            "FloatEntityData",
            &[
                ("DefaultValue", Value::Float32(0.0)),
                ("IncDecValue", Value::Float32(f32::NAN)),
            ],
        ),
        object("EventGateEntityData", &[("Default", Value::UInt32(1))]),
        object("EventSplitterEntityData", &[]),
        object(
            "CompareIntEntityData",
            &[("A", Value::UInt32(1)), ("B", Value::Int32(0))],
        ),
        object("MathOpEntityData", &[]),
        object("PropertyCastEntityData", &[]),
    ] {
        assert!(compile(&obj).is_err(), "{}", obj.class);
    }
}
