// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local};

fn enumeration(name: &str) -> Value {
    Value::Enum {
        value: if name == "EventConnectionTargetType_Server" {
            3
        } else {
            0
        },
        name: Some(name.into()),
    }
}
fn structure(fields: &[(&str, Value)]) -> Value {
    Value::Struct(Fields(
        fields
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect(),
    ))
}
struct Fixture {
    memory: Memory,
    ids: BTreeMap<&'static str, usize>,
    next: usize,
}
impl Fixture {
    fn put(&mut self, name: &'static str, class: &str, fields: &[(&str, Value)]) {
        let index = self.next;
        self.next += 3;
        self.ids.insert(name, index);
        self.memory.put("a", index, class, fields);
        self.set(name, "Flags", Value::UInt32((2 << 25) | (2 << 27)));
    }
    fn set(&mut self, name: &str, field: &str, value: Value) {
        let fields = &mut self
            .memory
            .0
            .get_mut(&("a".into(), self.ids[name]))
            .unwrap()
            .fields
            .0;
        fields.retain(|(k, _)| k != field);
        fields.push((field.into(), value));
    }
    fn root(&mut self, field: &str) -> &mut Vec<Value> {
        let value = &mut self
            .memory
            .0
            .get_mut(&("a".into(), 0))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == field)
            .unwrap()
            .1;
        let Value::Array(rows) = value else { panic!() };
        rows
    }
    fn edge(&self, s: &str, sp: u32, t: &str, tp: u32) -> Value {
        edge(local(self.ids[s]), sp, local(self.ids[t]), tp)
    }
    fn event(&self, s: &str, output: u32, t: &str, input: u32) -> Value {
        structure(&[
            ("Source", local(self.ids[s])),
            ("Target", local(self.ids[t])),
            (
                "SourceEvent",
                structure(&[("Id", Value::Int32(output as i32))]),
            ),
            (
                "TargetEvent",
                structure(&[("Id", Value::Int32(input as i32))]),
            ),
            (
                "TargetType",
                enumeration("EventConnectionTargetType_Server"),
            ),
        ])
    }
    fn read(&mut self) -> Result<Binding, Error> {
        let rules = Rules::from_json(
            &crate::BuildIdentity::supported()
                .participant_logic
                .to_json(),
        )
        .unwrap();
        read(&mut self.memory, "a", &rules)
    }
}

fn fixture(reverse: bool) -> Fixture {
    let mut f = Fixture {
        memory: Memory::default(),
        ids: BTreeMap::new(),
        next: 11,
    };
    f.put("owner", "ParticipantLogicEntityData", &[]);
    f.put("channel", "LinkedSchematicChannelReferenceEntityData", &[]);
    for (name, stat) in [
        ("km", "TotalDistanceDrivenKm"),
        ("mile", "TotalDistanceDriven"),
    ] {
        f.put(name, "OnlineStatUpdateEntityData", &[]);
        let asset = f.next;
        f.next += 3;
        f.memory.put(
            "a",
            asset,
            "OnlineStatAsset",
            &[
                ("StatName", Value::String(stat.into())),
                ("StatOperation", enumeration("EOnlineStatOperation_Replace")),
            ],
        );
        f.set(name, "StatAsset", local(asset));
    }
    for (name, class, op) in [
        ("km_div", "MathIntOpEntityData", "IntMathOp_Divide"),
        ("mile_div", "MathOpEntityData", "MathOp_Divide"),
        ("sum", "MathIntOpEntityData", "IntMathOp_Add"),
    ] {
        f.put(
            name,
            class,
            &[("Operators", Value::Array(vec![enumeration(op)]))],
        );
    }
    f.put(
        "metric",
        "IntEntityData",
        &[("DefaultValue", Value::Int32(1000))],
    );
    f.put(
        "imperial",
        "FloatEntityData",
        &[("DefaultValue", Value::Float32(1609.344))],
    );
    for name in ["mile_cast", "total_cast", "accum_cast"] {
        f.put(name, "PropertyCastEntityData", &[]);
    }
    for name in ["total", "initial"] {
        f.put(
            name,
            "IntCacheEntityData",
            &[("LoadOnCreate", Value::Bool(false))],
        );
    }
    for name in ["current", "previous"] {
        f.put(
            name,
            "Vector3CacheEntityData",
            &[("LoadOnCreate", Value::Bool(true))],
        );
    }
    f.put("property", "IntParticipantPropertyAccessorEntityData", &[]);
    f.put("accum", "FloatAccumulatorEntityData", &[]);
    for (name, op) in [
        ("length", "VectorMathOp_Length"),
        ("subtract", "VectorMathOp_Subtract"),
    ] {
        f.put(
            name,
            "VectorMathOpEntityData",
            &[("MathOperator", enumeration(op))],
        );
    }
    f.put("builder", "Vec3BuilderEntityData", &[]);
    f.put("split", "Vec3SplitterEntityData", &[]);
    f.put(
        "conditional",
        "ConditionalFloatEntityData",
        &[("ValueIfTrue", Value::Float32(0.))],
    );
    f.put("flags", "OrEntityData", &[("InputCount", Value::UInt32(2))]);
    f.put("physics", "RaceVehiclePhysicsQueryEntityData", &[]);
    f.put("transform", "TransformSplitterEntityData", &[]);
    f.put(
        "delay",
        "DelayEntityData",
        &[
            ("Delay", Value::Float32(1.)),
            ("AutoStart", Value::Bool(false)),
            ("RunOnce", Value::Bool(false)),
            ("RemoveDuplicateEvents", Value::Bool(true)),
            ("TimeDeltaType", enumeration("TimeDeltaType_World")),
        ],
    );
    for name in ["km_changed", "mile_changed"] {
        f.put(
            name,
            "PropertyChangedEntityData",
            &[
                ("AlwaysFire", Value::Bool(false)),
                ("SuppressFirstChange", Value::Bool(false)),
                ("Enabled", Value::Bool(true)),
            ],
        );
    }
    f.put(
        "participant",
        "ParticipantLogicParticipantProviderEntityData",
        &[],
    );
    f.put(
        "controllable",
        "ParticipantLogicControllableProviderEntityData",
        &[],
    );
    let properties = [
        ("km_div", OUT, "km", STAT_VALUE),
        ("mile_cast", 245372264, "mile", STAT_VALUE),
        ("total", CACHE_OUT, "km_div", A),
        ("metric", 225375086, "km_div", B),
        ("mile_div", OUT, "mile_cast", 3123792190),
        ("total_cast", 936870923, "mile_div", A),
        ("imperial", 225375086, "mile_div", B),
        ("total", CACHE_OUT, "total_cast", 5685821),
        ("sum", OUT, "total", CACHE_IN),
        ("initial", CACHE_OUT, "sum", A),
        ("accum_cast", 245372264, "sum", B),
        ("property", 731416433, "initial", CACHE_IN),
        ("total", CACHE_OUT, "property", 225375086),
        ("accum", 2895736442, "accum_cast", 3123792190),
        ("length", 2595815478, "accum", 214522259),
        ("builder", OUT, "length", 2412549647),
        ("split", 177661, "builder", 177661),
        ("split", 177663, "builder", 177663),
        ("conditional", 2895736442, "builder", 177660),
        ("split", 177660, "conditional", 2915912540),
        ("flags", OUT, "conditional", 1800624758),
        ("physics", 0x4e559cf1, "flags", 193450867),
        ("physics", 0x3c230e35, "flags", 193450864),
        ("subtract", 976788535, "split", IN),
        ("current", CACHE_OUT, "subtract", 2412549647),
        ("previous", CACHE_OUT, "subtract", 2301908460),
        ("current", CACHE_OUT, "previous", CACHE_IN),
        ("transform", 227190399, "current", CACHE_IN),
        ("physics", 2270319721, "transform", 2270319721),
        ("km_div", OUT, "km_changed", 910586961),
        ("mile_cast", 245372264, "mile_changed", 910586961),
    ]
    .into_iter()
    .map(|(s, sp, t, tp)| f.edge(s, sp, t, tp))
    .collect();
    let links = [
        ("property", 3062566331, "participant", 0),
        ("km", 3062566331, "participant", 0),
        ("mile", 3062566331, "participant", 0),
        ("participant", 560080367, "owner", 0),
        ("physics", 4024553726, "controllable", 0),
        ("controllable", 3867345844, "owner", 0),
        ("property", 2249073098, "channel", 901),
        ("owner", 440997395, "channel", 0),
    ]
    .into_iter()
    .map(|(s, sp, t, tp)| f.edge(s, sp, t, tp))
    .collect();
    let events = [
        ("delay", OUT, "current", LOAD),
        ("delay", OUT, "accum", ACCUMULATE),
        ("delay", OUT, "previous", LOAD),
        ("delay", OUT, "delay", IN),
        ("owner", STARTED, "initial", LOAD),
        ("owner", STARTED, "current", LOAD),
        ("owner", STARTED, "previous", LOAD),
        ("owner", STARTED, "delay", IN),
        ("accum", ACCUMULATED, "total", LOAD),
        ("km_changed", CHANGED, "km", STAT_UPDATE),
        ("mile_changed", CHANGED, "mile", STAT_UPDATE),
    ]
    .into_iter()
    .map(|(s, sp, t, tp)| f.event(s, sp, t, tp))
    .collect();
    let mut components: Vec<_> = f
        .ids
        .iter()
        .filter(|(name, _)| **name != "owner")
        .map(|(_, id)| *id)
        .collect();
    components.sort();
    if reverse {
        components.reverse();
    }
    f.set(
        "owner",
        "Components",
        Value::Array(components.into_iter().map(local).collect()),
    );
    f.memory.put(
        "a",
        0,
        "ParticipantLogicBlueprint",
        &[
            ("Flags", Value::UInt32(1)),
            ("Objects", Value::Array(vec![local(f.ids["owner"])])),
            ("PropertyConnections", Value::Array(properties)),
            ("LinkConnections", Value::Array(links)),
            ("EventConnections", Value::Array(events)),
        ],
    );
    f
}

#[test]
fn binds_the_authored_calculation_and_tracks_serializer_order() {
    let first = fixture(false).read().unwrap();
    assert_eq!(
        (
            first.kilometres_serializer,
            first.miles_serializer,
            first.channel_serializer
        ),
        (2, 3, 1)
    );
    assert_eq!(
        (
            first.distance_property,
            first.period_seconds,
            first.metres_per_mile
        ),
        (901, 1., 1609.344)
    );
    let changed = fixture(true).read().unwrap();
    assert_eq!(
        (
            changed.kilometres_serializer,
            changed.miles_serializer,
            changed.channel_serializer
        ),
        (2, 1, 3)
    );
    assert_eq!(changed.distance_property, first.distance_property);
}

#[test]
fn rejects_changed_semantics_owners_ambiguous_inputs_and_trigger_order() {
    for case in 0..17 {
        let mut f = fixture(false);
        match case {
            0 => f.set("metric", "DefaultValue", Value::Int32(10)),
            1 => f.set("imperial", "DefaultValue", Value::Float32(f32::NAN)),
            2 => f.set("initial", "LoadOnCreate", Value::Bool(true)),
            3 => f.set("conditional", "ValueIfTrue", Value::Float32(1.)),
            4 => f.set("km_changed", "AlwaysFire", Value::Bool(true)),
            5 => f.set("delay", "RunOnce", Value::Bool(true)),
            6 => f.set("delay", "TimeDeltaType", enumeration("TimeDeltaType_Real")),
            7 => {
                f.root("PropertyConnections").remove(0);
            }
            8 => {
                let row = f.root("PropertyConnections")[0].clone();
                f.root("PropertyConnections").push(row);
            }
            9 => f.root("EventConnections").swap(0, 1),
            10 => {
                let e = f.edge("mile", 3062566331, "controllable", 0);
                f.root("LinkConnections")[2] = e;
            }
            11 => {
                let e = f.event("owner", STARTED, "total", LOAD);
                f.root("EventConnections").push(e);
            }
            12 => f.set(
                "km_div",
                "Operators",
                Value::Array(vec![enumeration("IntMathOp_Multiply")]),
            ),
            13 => f.set("flags", "InputCount", Value::Int32(2)),
            14 => f.set("sum", "Flags", Value::UInt32(1 << 27)),
            15 => f.set("accum", "Flags", Value::UInt32(2 << 27)),
            _ => {
                let e = f.event("owner", STARTED, "controllable", LOAD);
                f.root("EventConnections").push(e);
            }
        }
        assert!(f.read().is_err(), "case {case}");
    }
}
