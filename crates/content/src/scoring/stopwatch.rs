// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed stopwatch settings. Resolve live inputs and context time scale
//! separately; this reader does not schedule the engine's update passes.
use super::{events::Identity, field, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::{Object, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Configuration {
    /// Authored update-pass enum, preserved without selecting a scheduler.
    pub update_pass: u8,
    pub update_pass_name: String,
    pub multiplier: f32,
    pub trigger_time: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub configuration: Configuration,
}

pub fn compile(object: &Object) -> Result<Configuration, Error> {
    if object.class != "StopWatchEntityData" {
        return Err(bad("unsupported stopwatch class"));
    }
    let fields = &object.fields;
    let (update_pass, update_pass_name) = match field(fields, "UpdatePass")? {
        Value::Enum {
            value,
            name: Some(name),
        } if (0..9).contains(value) && name.starts_with("UpdatePass_") && name.len() <= 64 => {
            (*value as u8, name.clone())
        }
        _ => return Err(bad("stopwatch update pass")),
    };
    Ok(Configuration {
        update_pass,
        update_pass_name,
        multiplier: scalar(fields, "Multiplier")?,
        trigger_time: scalar(fields, "TriggerOnTime")?,
    })
}
impl AssetIndex {
    /// Preserve each stopwatch occurrence; other node classes are outside this
    /// reader's scope. Bound input properties take precedence over these defaults.
    pub fn scoring_stopwatches(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter(|node| node.object.class == "StopWatchEntityData")
            .map(|node| {
                Ok(Node {
                    identity: Identity::from(&node.site),
                    configuration: compile(&node.object)?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_frostbite::ebx::Fields;
    fn object(pass: Value, multiplier: Value, trigger: Value) -> Object {
        Object {
            class: "StopWatchEntityData".into(),
            guid: None,
            fields: Fields(vec![
                ("UpdatePass".into(), pass),
                ("Multiplier".into(), multiplier),
                ("TriggerOnTime".into(), trigger),
            ]),
        }
    }
    #[test]
    fn preserve_signed_floats_and_enum_without_assuming_context_scale() {
        let c = compile(&object(
            Value::Enum {
                value: 2,
                name: Some("UpdatePass_Test".into()),
            },
            Value::Float32(-0.5),
            Value::Float32(3.0),
        ))
        .unwrap();
        assert_eq!(
            c,
            Configuration {
                update_pass: 2,
                update_pass_name: "UpdatePass_Test".into(),
                multiplier: -0.5,
                trigger_time: 3.0
            }
        );
    }
    #[test]
    fn malformed_and_nonfinite_timer_settings_fail() {
        for pass in [
            Value::Int32(1),
            Value::Enum {
                value: 9,
                name: Some("UpdatePass_Test".into()),
            },
            Value::Enum {
                value: 0,
                name: None,
            },
            Value::Enum {
                value: 0,
                name: Some("Other".into()),
            },
        ] {
            assert!(compile(&object(pass, Value::Float32(1.0), Value::Float32(0.0))).is_err());
        }
        let pass = Value::Enum {
            value: 0,
            name: Some("UpdatePass_Test".into()),
        };
        assert!(
            compile(&object(
                pass.clone(),
                Value::Float32(f32::INFINITY),
                Value::Float32(0.0)
            ))
            .is_err()
        );
        assert!(compile(&object(pass, Value::Float32(1.0), Value::Float32(f32::NAN))).is_err());
    }
}
