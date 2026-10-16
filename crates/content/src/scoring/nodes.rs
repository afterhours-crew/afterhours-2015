// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed configuration for the supported transient scalar node subset.
use super::{boolean, field, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::{Fields, Object, Value};
use nfs_world::scoring::{
    math::Value as Scalar,
    nodes::{self, Comparison, Definition},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Configuration {
    pub definition: Definition,
    /// Authored input fallbacks, not output initialization. The interpreter must
    /// apply each node getter's binding/readiness contract before resolving them.
    pub defaults: Vec<(u32, Scalar)>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: super::events::Identity,
    pub configuration: Configuration,
}
pub(super) fn supported(class: &str) -> bool {
    matches!(
        class,
        "FloatCacheEntityData"
            | "FloatAccumulatorEntityData"
            | "FloatEntityData"
            | "EventGateEntityData"
            | "EventSplitterEntityData"
            | "CompareFloatEntityData"
            | "CompareIntEntityData"
            | "BoolEntityData"
            | "IntEntityData"
            | "IntCacheEntityData"
            | "BoolCacheEntityData"
            | "CompareBoolEntityData"
            | "ConditionalFloatEntityData"
    )
}
fn integer(fields: &Fields, name: &str) -> Result<i32, Error> {
    match field(fields, name)? {
        Value::Int32(value) => Ok(*value),
        _ => Err(bad("scoring signed integer field")),
    }
}
/// Compile one supported class. This does not validate the composition's
/// property wiring or instantiate other classes such as queries or MathOp.
pub fn compile(object: &Object) -> Result<Configuration, Error> {
    let fields = &object.fields;
    let mut defaults = vec![];
    let definition = match object.class.as_str() {
        "FloatCacheEntityData" => {
            defaults.push((
                nodes::CACHE_INPUT,
                Scalar::Float(scalar(fields, "InValue")?),
            ));
            Definition::Cache
        }
        "FloatAccumulatorEntityData" => {
            defaults.push((nodes::INPUT, Scalar::Float(scalar(fields, "Input")?)));
            Definition::Accumulator
        }
        "FloatEntityData" => Definition::Float {
            default: scalar(fields, "DefaultValue")?,
            step: scalar(fields, "IncDecValue")?,
        },
        "EventGateEntityData" => Definition::Gate {
            open: boolean(fields, "Default")?,
        },
        "EventSplitterEntityData" => Definition::Splitter {
            run_once: boolean(fields, "RunOnce")?,
        },
        "BoolEntityData" => Definition::Boolean {
            default: boolean(fields, "DefaultValue")?,
        },
        "IntEntityData" => Definition::Integer {
            default: integer(fields, "DefaultValue")?,
            step: integer(fields, "IncDecValue")?,
        },
        "IntCacheEntityData" => {
            if !boolean(fields, "LoadOnCreate")? {
                return Err(bad("unsupported uninitialized integer cache"));
            }
            defaults.push((
                nodes::CACHE_INPUT,
                Scalar::Integer(integer(fields, "InValue")?),
            ));
            Definition::IntegerCache
        }
        "BoolCacheEntityData" => {
            defaults.push((
                nodes::CACHE_INPUT,
                Scalar::Boolean(boolean(fields, "InValue")?),
            ));
            Definition::BooleanCache {
                load_on_create: boolean(fields, "LoadOnCreate")?,
            }
        }
        "CompareBoolEntityData" => {
            defaults.push((nodes::BOOL, Scalar::Boolean(boolean(fields, "Bool")?)));
            Definition::CompareBoolean {
                on_change: boolean(fields, "TriggerOnPropertyChange")?,
                on_start: boolean(fields, "TriggerOnStart")?,
                always_send: boolean(fields, "AlwaysSend")?,
            }
        }
        "ConditionalFloatEntityData" => {
            defaults.extend([
                (
                    nodes::CONDITION,
                    Scalar::Boolean(boolean(fields, "Condition")?),
                ),
                (
                    nodes::VALUE_IF_TRUE,
                    Scalar::Float(scalar(fields, "ValueIfTrue")?),
                ),
                (
                    nodes::VALUE_IF_FALSE,
                    Scalar::Float(scalar(fields, "ValueIfFalse")?),
                ),
            ]);
            Definition::ConditionalFloat
        }
        "CompareFloatEntityData" | "CompareIntEntityData" => {
            let kind = if object.class == "CompareFloatEntityData" {
                Comparison::Float
            } else {
                Comparison::Integer
            };
            for (port, name) in [(nodes::A, "A"), (nodes::B, "B")] {
                let value = match kind {
                    Comparison::Float => Scalar::Float(scalar(fields, name)?),
                    Comparison::Integer => match field(fields, name)? {
                        Value::Int32(value) => Scalar::Integer(*value),
                        _ => return Err(bad("scoring signed integer field")),
                    },
                };
                defaults.push((port, value));
            }
            Definition::Compare {
                kind,
                on_change: boolean(fields, "TriggerOnPropertyChange")?,
                on_start: boolean(fields, "TriggerOnStart")?,
            }
        }
        _ => return Err(bad("unsupported stateful scoring class")),
    };
    nodes::Node::new(definition).map_err(|e| bad(&e.to_string()))?;
    Ok(Configuration {
        definition,
        defaults,
    })
}
impl AssetIndex {
    /// Read the supported subset, preserving each prefab occurrence. Other
    /// classes are outside this method's scope, not silently supported nodes.
    pub fn scoring_nodes(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter(|node| supported(&node.object.class))
            .map(|node| {
                Ok(Node {
                    identity: super::events::Identity::from(&node.site),
                    configuration: compile(&node.object).map_err(|e| {
                        bad(&format!("{}:{}: {e}", node.site.key.0, node.site.key.1))
                    })?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
