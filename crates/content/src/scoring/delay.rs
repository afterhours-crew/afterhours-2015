// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed delay settings. The caller resolves the actual engine time scale.
use super::{boolean, events::Identity, field, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::scoring::delay::Definition;
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Configuration {
    pub definition: Definition,
    pub delay: f32,
    pub time_domain: u8,
    pub time_domain_name: String,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Node {
    pub identity: Identity,
    pub configuration: Configuration,
}
pub fn compile(object: &Object) -> Result<Configuration, Error> {
    if object.class != "DelayEntityData" {
        return Err(bad("unsupported delay class"));
    }
    let f = &object.fields;
    let (time_domain, time_domain_name) = match field(f, "TimeDeltaType")? {
        Value::Enum {
            value,
            name: Some(name),
        } if (0..8).contains(value) && name.starts_with("TimeDeltaType_") && name.len() <= 64 => {
            (*value as u8, name.clone())
        }
        _ => return Err(bad("delay time domain")),
    };
    Ok(Configuration {
        definition: Definition {
            auto_start: boolean(f, "AutoStart")?,
            run_once: boolean(f, "RunOnce")?,
            remove_duplicates: boolean(f, "RemoveDuplicateEvents")?,
        },
        delay: scalar(f, "Delay")?,
        time_domain,
        time_domain_name,
    })
}
impl AssetIndex {
    pub fn scoring_delays(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter(|n| n.object.class == "DelayEntityData")
            .map(|n| {
                Ok(Node {
                    identity: Identity::from(&n.site),
                    configuration: compile(&n.object)?,
                })
            })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use nfs_frostbite::ebx::Fields;
    fn object(delay: Value, domain: Value) -> Object {
        Object {
            class: "DelayEntityData".into(),
            guid: None,
            fields: Fields(vec![
                ("Delay".into(), delay),
                ("TimeDeltaType".into(), domain),
                ("AutoStart".into(), Value::Bool(true)),
                ("RunOnce".into(), Value::Bool(true)),
                ("RemoveDuplicateEvents".into(), Value::Bool(false)),
            ]),
        }
    }
    fn domain() -> Value {
        Value::Enum {
            value: 1,
            name: Some("TimeDeltaType_Test".into()),
        }
    }
    #[test]
    fn preserves_flags_signed_delay_and_time_domain_without_assuming_scale() {
        let c = compile(&object(Value::Float32(-0.5), domain())).unwrap();
        assert_eq!(
            c.definition,
            Definition {
                auto_start: true,
                run_once: true,
                remove_duplicates: false
            }
        );
        assert_eq!(c.delay, -0.5);
        assert_eq!(c.time_domain, 1);
        assert_eq!(c.time_domain_name, "TimeDeltaType_Test");
    }
    #[test]
    fn malformed_nonfinite_delay_and_time_domain_are_rejected() {
        for delay in [Value::Int32(1), Value::Float32(f32::NAN)] {
            assert!(compile(&object(delay, domain())).is_err());
        }
        for d in [
            Value::Int32(1),
            Value::Enum {
                value: 8,
                name: Some("TimeDeltaType_Test".into()),
            },
            Value::Enum {
                value: 1,
                name: None,
            },
        ] {
            assert!(compile(&object(Value::Float32(0.0), d)).is_err());
        }
    }
}
