// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! And/Or configuration; the graph compiler supplies resolved bound inputs.
use super::{events::Identity, field};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad},
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::scoring::boolean_fold::MAX_INPUTS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Configuration {
    pub all: bool,
    pub input_count: u16,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub configuration: Configuration,
}
pub fn compile(object: &Object) -> Result<Configuration, Error> {
    let all = match object.class.as_str() {
        "AndEntityData" => true,
        "OrEntityData" => false,
        _ => return Err(bad("unsupported Boolean fold class")),
    };
    let input_count = match field(&object.fields, "InputCount")? {
        Value::UInt32(n) if *n <= MAX_INPUTS as u32 => *n as u16,
        _ => return Err(bad("Boolean input count")),
    };
    Ok(Configuration { all, input_count })
}
fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    if blueprint.is_empty() || blueprint.len() > 512 {
        return Err(bad("scoring blueprint name"));
    }
    Graph::new(source)
        .read(Site::root(blueprint))?
        .into_iter()
        .filter(|n| matches!(n.object.class.as_str(), "AndEntityData" | "OrEntityData"))
        .map(|n| {
            Ok(Node {
                identity: Identity::from(&n.site),
                configuration: compile(&n.object)?,
            })
        })
        .collect()
}
impl AssetIndex {
    /// Preserves configuration and prefab instances. Does not choose between
    /// multiple sources, apply interface defaults or activate the nodes.
    pub fn scoring_boolean_folds(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        read(&mut self.assets, blueprint)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, local, ptr};
    #[test]
    fn retains_counts_and_independent_prefab_occurrences() {
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
            &[("Objects", Value::Array(vec![local(1)]))],
        );
        s.put(
            "b",
            1,
            "AndEntityData",
            &[("InputCount", Value::UInt32(12))],
        );
        let nodes = read(&mut s, "a").unwrap();
        assert_eq!(nodes.len(), 2);
        for (i, n) in nodes.iter().enumerate() {
            assert_eq!(
                n.configuration,
                Configuration {
                    all: true,
                    input_count: 12
                }
            );
            assert_eq!(n.identity.instances, vec![("a".into(), i + 1)]);
        }
    }
    #[test]
    fn validates_count_type_bounds_and_empty_or() {
        let mut s = Memory::default();
        for value in [Value::UInt32(257), Value::Int32(-1), Value::Bool(true)] {
            s.put("a", 0, "OrEntityData", &[("InputCount", value)]);
            assert!(read(&mut s, "a").is_err());
        }
        s.put("a", 0, "OrEntityData", &[("InputCount", Value::UInt32(0))]);
        assert_eq!(
            read(&mut s, "a").unwrap()[0].configuration,
            Configuration {
                all: false,
                input_count: 0
            }
        );
    }
}
