// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! MathOp and VectorMathOp Length configuration, separate from input binding.
use super::{events::Identity, field};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad},
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::scoring::arithmetic::{MAX_OPERATIONS, Operation};

#[derive(Clone, Debug, PartialEq)]
pub enum Configuration {
    /// In, InMin, InMax, OutMin, OutMax in native getter order.
    ScaleClamp([f32; 5]),
    StringConstant(nfs_world::scoring::value::Text),
    FloatSelect {
        positive: f32,
        negative: f32,
        select: f32,
    },
    RandomFloat {
        min: f32,
        max: f32,
        true_random: bool,
    },
    ParticipantRank {
        descending: bool,
    },
    FloatSwitch(nfs_world::scoring::float_switch::Definition),
    Arithmetic(Vec<Operation>),
    VectorLength,
    IntegerSum(u16),
    IntegerArithmetic(Vec<nfs_world::scoring::integers::Operation>),
    VectorDot,
    IntegerHub {
        count: u16,
        selection: i32,
    },
    FloatHub {
        count: u16,
        selection: i32,
    },
    RankIntegers {
        count: u16,
        on_change: bool,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub configuration: Configuration,
}

pub fn compile(object: &Object) -> Result<Configuration, Error> {
    match object.class.as_str() {
        "ScaleClampEntityData" => Ok(Configuration::ScaleClamp([
            super::scalar(&object.fields, "In")?,
            super::scalar(&object.fields, "InMin")?,
            super::scalar(&object.fields, "InMax")?,
            super::scalar(&object.fields, "OutMin")?,
            super::scalar(&object.fields, "OutMax")?,
        ])),
        "StringEntityData" => {
            let Value::String(value) = field(&object.fields, "DefaultString")? else {
                return Err(bad("string constant type"));
            };
            Ok(Configuration::StringConstant(
                nfs_world::scoring::value::Text::new(value).map_err(|e| bad(&e.to_string()))?,
            ))
        }
        "FloatSelectEntityData" => Ok(Configuration::FloatSelect {
            positive: super::scalar(&object.fields, "InPosOrZero")?,
            negative: super::scalar(&object.fields, "InNeg")?,
            select: super::scalar(&object.fields, "Select")?,
        }),
        "RandomFloatEntityData" => Ok(Configuration::RandomFloat {
            min: super::scalar(&object.fields, "Min")?,
            max: super::scalar(&object.fields, "Max")?,
            true_random: super::boolean(&object.fields, "TrueRandom")?,
        }),
        "SortAndRankIntParticipantPropertyEntityData" => {
            if super::boolean(&object.fields, "UseTieBreaker")? {
                return Err(bad("participant rank tie-breaker is not modeled"));
            }
            let descending = match field(&object.fields, "SortingOrder")? {
                Value::Enum {
                    value: 0,
                    name: Some(n),
                } if n == "SortOrder_Ascending" => false,
                Value::Enum {
                    value: 1,
                    name: Some(n),
                } if n == "SortOrder_Descending" => true,
                _ => return Err(bad("participant rank order")),
            };
            Ok(Configuration::ParticipantRank { descending })
        }
        "SwitchPropertyFloatEntityData" => {
            use nfs_world::scoring::float_switch::{Definition, Entry, MAX_ENTRIES};
            let rows = field(&object.fields, "FloatProperties")?
                .as_array()
                .filter(|v| !v.is_empty() && v.len() <= MAX_ENTRIES)
                .ok_or_else(|| bad("float switch entries"))?;
            let mut entries = Vec::with_capacity(rows.len());
            for row in rows {
                let fields = row.as_fields().ok_or_else(|| bad("float switch entry"))?;
                let Value::String(name) = field(fields, "Name")? else {
                    return Err(bad("float switch name"));
                };
                if name.len() > 512 || name.as_bytes().contains(&0) {
                    return Err(bad("float switch name bound"));
                }
                let hash = |s: &str| {
                    s.bytes()
                        .fold(5381u32, |v, b| v.wrapping_mul(33) ^ u32::from(b))
                };
                entries.push(Entry {
                    property: hash(name),
                    event: hash(&format!("Select{name}")),
                    fallback: super::scalar(fields, "Value")?,
                });
            }
            let definition = Definition {
                entries,
                on_change: super::boolean(&object.fields, "TriggerOnPropertyChange")?,
            };
            definition.validate().map_err(|e| bad(&e.to_string()))?;
            Ok(Configuration::FloatSwitch(definition))
        }
        "IntHubEntityData" | "FloatHubEntityData" => {
            let count = field(&object.fields, "InputCount")?
                .bits32()
                .and_then(|v| u16::try_from(v).ok())
                .filter(|&v| v > 0 && v <= nfs_world::scoring::integer_hub::MAX_INPUTS)
                .ok_or_else(|| bad("integer hub count"))?;
            let Value::Int32(selection) = field(&object.fields, "InputSelect")? else {
                return Err(bad("integer hub selection"));
            };
            Ok(if object.class == "FloatHubEntityData" {
                Configuration::FloatHub {
                    count,
                    selection: *selection,
                }
            } else {
                Configuration::IntegerHub {
                    count,
                    selection: *selection,
                }
            })
        }
        "MathIntOpEntityData" => {
            let operators = field(&object.fields, "Operators")?
                .as_array()
                .ok_or_else(|| bad("integer operations"))?;
            if operators.len() > MAX_OPERATIONS {
                return Err(bad("integer operation bound"));
            }
            use nfs_world::scoring::integers::Operation as IntOp;
            let operations = operators
                .iter()
                .map(|v| match v {
                    Value::Enum {
                        value: 0,
                        name: Some(n),
                    } if n == "IntMathOp_Add" => Ok(IntOp::Add),
                    Value::Enum {
                        value: 1,
                        name: Some(n),
                    } if n == "IntMathOp_Subtract" => Ok(IntOp::Subtract),
                    Value::Enum {
                        value: 3,
                        name: Some(n),
                    } if n == "IntMathOp_Divide" => Ok(IntOp::Divide),
                    _ => Err(bad("unsupported integer operation")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            if operations.iter().all(|op| *op == IntOp::Add) {
                Ok(Configuration::IntegerSum(operations.len() as u16 + 1))
            } else {
                Ok(Configuration::IntegerArithmetic(operations))
            }
        }
        "RankIntegersEntityData" => {
            let count = field(&object.fields, "InputCount")?
                .bits32()
                .and_then(|v| u16::try_from(v).ok())
                .filter(|v| *v > 0 && *v <= nfs_world::scoring::integers::MAX_RANK_INPUTS)
                .ok_or_else(|| bad("integer rank input bound"))?;
            let on_change = super::boolean(&object.fields, "SortOnPropertyChange")?;
            Ok(Configuration::RankIntegers { count, on_change })
        }
        "MathOpEntityData" => {
            let Value::Array(values) = field(&object.fields, "Operators")? else {
                return Err(bad("MathOp operators"));
            };
            if values.len() > MAX_OPERATIONS {
                return Err(bad("MathOp operation bound"));
            }
            let operations = values
                .iter()
                .map(|v| {
                    let Value::Enum { value, name } = v else {
                        return Err(bad("MathOp operator type"));
                    };
                    match (*value, name.as_deref()) {
                        (0, Some("MathOp_Add")) => Ok(Operation::Add),
                        (1, Some("MathOp_Subtract")) => Ok(Operation::Subtract),
                        (2, Some("MathOp_Multiply")) => Ok(Operation::Multiply),
                        (3, Some("MathOp_Divide")) => Ok(Operation::Divide),
                        _ => Err(bad("unsupported MathOp operator")),
                    }
                })
                .collect::<Result<_, _>>()?;
            Ok(Configuration::Arithmetic(operations))
        }
        "VectorMathOpEntityData" => match field(&object.fields, "MathOperator")? {
            Value::Enum { value: 5, name } if name.as_deref() == Some("VectorMathOp_Dot") => {
                Ok(Configuration::VectorDot)
            }
            Value::Enum { value: 6, name } if name.as_deref() == Some("VectorMathOp_Length") => {
                Ok(Configuration::VectorLength)
            }
            _ => Err(bad("unsupported VectorMathOp operator")),
        },
        _ => Err(bad("unsupported arithmetic class")),
    }
}
pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    if blueprint.is_empty() || blueprint.len() > 512 {
        return Err(bad("scoring blueprint name"));
    }
    Graph::new(source)
        .read(Site::root(blueprint))?
        .into_iter()
        .filter(|n| {
            matches!(
                n.object.class.as_str(),
                "MathOpEntityData"
                    | "ScaleClampEntityData"
                    | "StringEntityData"
                    | "FloatSelectEntityData"
                    | "RandomFloatEntityData"
                    | "SortAndRankIntParticipantPropertyEntityData"
                    | "SwitchPropertyFloatEntityData"
                    | "VectorMathOpEntityData"
                    | "MathIntOpEntityData"
                    | "RankIntegersEntityData"
                    | "IntHubEntityData"
                    | "FloatHubEntityData"
            )
        })
        .map(|n| {
            Ok(Node {
                identity: Identity::from(&n.site),
                configuration: compile(&n.object)?,
            })
        })
        .collect()
}
impl AssetIndex {
    /// Preserve operator order and prefab instances. Does not resolve inputs,
    /// initialize properties, activate nodes or restore a saved graph.
    pub fn scoring_arithmetic(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        read(&mut self.assets, blueprint)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, local, ptr};
    #[test]
    fn constants_and_selectors_require_typed_bounded_authored_values() {
        let mut source = Memory::default();
        source.put(
            "s",
            0,
            "StringEntityData",
            &[("DefaultString", Value::String("Restart".into()))],
        );
        assert_eq!(
            read(&mut source, "s").unwrap()[0].configuration,
            Configuration::StringConstant(nfs_world::scoring::value::Text::new("Restart").unwrap())
        );
        for value in [
            Value::Int32(1),
            Value::String("a\0b".into()),
            Value::String("a".repeat(1024)),
        ] {
            source.put("s", 0, "StringEntityData", &[("DefaultString", value)]);
            assert!(read(&mut source, "s").is_err());
        }
        source.put(
            "s",
            0,
            "FloatSelectEntityData",
            &[
                ("InPosOrZero", Value::Float32(2.)),
                ("InNeg", Value::Float32(3.)),
                ("Select", Value::Float32(-0.)),
            ],
        );
        assert_eq!(
            read(&mut source, "s").unwrap()[0].configuration,
            Configuration::FloatSelect {
                positive: 2.,
                negative: 3.,
                select: -0.
            }
        );
        source.put(
            "s",
            0,
            "FloatSelectEntityData",
            &[
                ("InPosOrZero", Value::Float32(2.)),
                ("InNeg", Value::Float32(f32::INFINITY)),
                ("Select", Value::Float32(1.)),
            ],
        );
        assert!(read(&mut source, "s").is_err());
    }
    #[test]
    fn participant_sort_checks_order_enum_and_rejects_unmodeled_tie_breaking() {
        let mut source = Memory::default();
        for (value, name, tie, expected) in [
            (0, "SortOrder_Ascending", false, Some(false)),
            (1, "SortOrder_Descending", false, Some(true)),
            (1, "SortOrder_Ascending", false, None),
            (0, "SortOrder_Ascending", true, None),
        ] {
            source.put(
                "r",
                0,
                "SortAndRankIntParticipantPropertyEntityData",
                &[
                    (
                        "SortingOrder",
                        Value::Enum {
                            value,
                            name: Some(name.into()),
                        },
                    ),
                    ("UseTieBreaker", Value::Bool(tie)),
                ],
            );
            let result = read(&mut source, "r");
            if let Some(descending) = expected {
                assert_eq!(
                    result.unwrap()[0].configuration,
                    Configuration::ParticipantRank { descending }
                );
            } else {
                assert!(result.is_err());
            }
        }
    }
    #[test]
    fn float_switch_reads_named_entries_and_preserves_duplicates() {
        let entry = |name: &str, value| {
            Value::Struct(nfs_frostbite::ebx::Fields(vec![
                ("Name".into(), Value::String(name.into())),
                ("Value".into(), Value::Float32(value)),
            ]))
        };
        let mut source = Memory::default();
        for rows in [
            vec![entry("Gold", 1.0), entry("Gold", 2.0)],
            vec![],
            vec![entry("Gold", f32::NAN)],
            vec![entry("Bad\0Name", 0.0)],
        ] {
            let valid = rows.len() == 2;
            source.put(
                "s",
                0,
                "SwitchPropertyFloatEntityData",
                &[
                    ("FloatProperties", Value::Array(rows)),
                    ("TriggerOnPropertyChange", Value::Bool(true)),
                ],
            );
            let result = read(&mut source, "s");
            if valid {
                let Configuration::FloatSwitch(definition) = &result.unwrap()[0].configuration
                else {
                    panic!()
                };
                assert_eq!(
                    definition.entries[0].property,
                    definition.entries[1].property
                );
                assert_eq!(definition.entries[0].event, definition.entries[1].event);
                assert_ne!(definition.entries[0].event, definition.entries[0].property);
                assert_eq!(definition.entries[1].fallback, 2.0);
            } else {
                assert!(result.is_err());
            }
        }
    }
    fn operator(value: i32, name: &str) -> Value {
        Value::Enum {
            value,
            name: Some(name.into()),
        }
    }
    #[test]
    fn integer_operations_require_supported_operators_and_bounded_rank_inputs() {
        let mut source = Memory::default();
        for count in [1, 19, 28] {
            source.put(
                "r",
                0,
                "RankIntegersEntityData",
                &[
                    ("InputCount", Value::Int32(count)),
                    ("SortOnPropertyChange", Value::Bool(false)),
                ],
            );
            assert_eq!(
                read(&mut source, "r").unwrap()[0].configuration,
                Configuration::RankIntegers {
                    count: count as u16,
                    on_change: false
                }
            );
        }
        for count in [0, -1, 29, i32::MAX] {
            source.put(
                "r",
                0,
                "RankIntegersEntityData",
                &[
                    ("InputCount", Value::Int32(count)),
                    ("SortOnPropertyChange", Value::Bool(false)),
                ],
            );
            assert!(read(&mut source, "r").is_err());
        }
        source.put(
            "r",
            0,
            "MathIntOpEntityData",
            &[(
                "Operators",
                Value::Array(vec![operator(0, "IntMathOp_Add"); 18]),
            )],
        );
        assert_eq!(
            read(&mut source, "r").unwrap()[0].configuration,
            Configuration::IntegerSum(19)
        );
        for op in [
            operator(0, "MathOp_Add"),
            operator(1, "IntMathOp_Add"),
            Value::Int32(0),
        ] {
            source.put(
                "r",
                0,
                "MathIntOpEntityData",
                &[("Operators", Value::Array(vec![op]))],
            );
            assert!(read(&mut source, "r").is_err());
        }
    }
    #[test]
    fn ordered_operations_and_vector_length_keep_prefab_instances() {
        let mut s = Memory::default();
        s.put(
            "a",
            0,
            "Blueprint",
            &[("Objects", Value::Array(vec![local(1), local(2)]))],
        );
        for i in [1, 2] {
            s.put(
                "a",
                i,
                "LogicPrefabReferenceObjectData",
                &[("Blueprint", ptr("b", 0))],
            );
        }
        s.put(
            "b",
            0,
            "Blueprint",
            &[("Objects", Value::Array(vec![local(1), local(2)]))],
        );
        s.put(
            "b",
            1,
            "MathOpEntityData",
            &[(
                "Operators",
                Value::Array(vec![
                    operator(2, "MathOp_Multiply"),
                    operator(0, "MathOp_Add"),
                    operator(3, "MathOp_Divide"),
                ]),
            )],
        );
        s.put(
            "b",
            2,
            "VectorMathOpEntityData",
            &[("MathOperator", operator(6, "VectorMathOp_Length"))],
        );
        let nodes = read(&mut s, "a").unwrap();
        assert_eq!(nodes.len(), 4);
        for (i, pair) in nodes.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(
                pair[0].configuration,
                Configuration::Arithmetic(vec![
                    Operation::Multiply,
                    Operation::Add,
                    Operation::Divide
                ])
            );
            assert_eq!(pair[1].configuration, Configuration::VectorLength);
            for n in pair {
                assert_eq!(n.identity.instances, vec![("a".into(), i + 1)]);
            }
        }
    }
    #[test]
    fn rejects_unknown_mismatched_untyped_and_oversized_operators() {
        let mut s = Memory::default();
        for value in [
            operator(6, "MathOp_Unknown"),
            operator(1, "MathOp_Add"),
            Value::UInt32(0),
        ] {
            s.put(
                "a",
                0,
                "MathOpEntityData",
                &[("Operators", Value::Array(vec![value]))],
            );
            assert!(read(&mut s, "a").is_err());
        }
        s.put(
            "a",
            0,
            "MathOpEntityData",
            &[(
                "Operators",
                Value::Array(vec![operator(0, "MathOp_Add"); MAX_OPERATIONS + 1]),
            )],
        );
        assert!(read(&mut s, "a").is_err());
        s.put(
            "a",
            0,
            "MathOpEntityData",
            &[("Operators", Value::Array(vec![]))],
        );
        assert_eq!(
            read(&mut s, "a").unwrap()[0].configuration,
            Configuration::Arithmetic(vec![])
        );
        for value in [
            operator(0, "VectorMathOp_Length"),
            operator(6, "Unknown"),
            Value::UInt32(6),
        ] {
            s.put("a", 0, "VectorMathOpEntityData", &[("MathOperator", value)]);
            assert!(read(&mut s, "a").is_err());
        }
        s.put("a", 0, "MathOpEntityData", &[]);
        assert!(read(&mut s, "a").is_err());
    }
}
