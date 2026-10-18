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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Configuration {
    Arithmetic(Vec<Operation>),
    VectorLength,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub configuration: Configuration,
}

pub fn compile(object: &Object) -> Result<Configuration, Error> {
    match object.class.as_str() {
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
                        (2, Some("MathOp_Multiply")) => Ok(Operation::Multiply),
                        (3, Some("MathOp_Divide")) => Ok(Operation::Divide),
                        _ => Err(bad("unsupported MathOp operator")),
                    }
                })
                .collect::<Result<_, _>>()?;
            Ok(Configuration::Arithmetic(operations))
        }
        "VectorMathOpEntityData" => match field(&object.fields, "MathOperator")? {
            Value::Enum { value: 6, name } if name.as_deref() == Some("VectorMathOp_Length") => {
                Ok(Configuration::VectorLength)
            }
            _ => Err(bad("unsupported VectorMathOp operator")),
        },
        _ => Err(bad("unsupported arithmetic class")),
    }
}
fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    if blueprint.is_empty() || blueprint.len() > 512 {
        return Err(bad("scoring blueprint name"));
    }
    Graph::new(source)
        .read(Site::root(blueprint))?
        .into_iter()
        .filter(|n| {
            matches!(
                n.object.class.as_str(),
                "MathOpEntityData" | "VectorMathOpEntityData"
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
    fn operator(value: i32, name: &str) -> Value {
        Value::Enum {
            value,
            name: Some(name.into()),
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
