// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
#[derive(Default)]
pub struct Memory(pub BTreeMap<Key, Object>);
impl Source for Memory {
    fn object(&mut self, key: &Key) -> Result<Object, Error> {
        self.0
            .get(key)
            .cloned()
            .ok_or_else(|| bad("test object missing"))
    }
    fn target(&mut self, source: &str, pointer: Pointer) -> Result<Option<Key>, Error> {
        let key = match pointer {
            Pointer::Null => return Ok(None),
            Pointer::Internal(i) => (source.into(), i),
            Pointer::Import {
                partition,
                instance,
            } => ((partition[0] as char).to_string(), instance[0] as usize),
        };
        self.object(&key)?;
        Ok(Some(key))
    }
}
impl Memory {
    pub fn put(&mut self, asset: &str, index: usize, class: &str, fields: &[(&str, Value)]) {
        self.0.insert(
            (asset.into(), index),
            Object {
                class: class.into(),
                guid: None,
                fields: nfs_frostbite::ebx::Fields(
                    fields
                        .iter()
                        .map(|(k, v)| ((*k).into(), v.clone()))
                        .collect(),
                ),
            },
        );
    }
}
pub fn ptr(asset: &str, index: usize) -> Value {
    Value::Pointer(Pointer::Import {
        partition: [asset.as_bytes()[0]; 16],
        instance: [index as u8; 16],
    })
}
pub fn local(index: usize) -> Value {
    Value::Pointer(Pointer::Internal(index))
}
pub fn edge(source: Value, sp: u32, target: Value, tp: u32) -> Value {
    Value::Struct(nfs_frostbite::ebx::Fields(vec![
        ("Source".into(), source),
        ("SourceFieldId".into(), Value::Int32(sp as i32)),
        ("Target".into(), target),
        ("TargetFieldId".into(), Value::Int32(tp as i32)),
    ]))
}
fn arrays(rows: Vec<Value>) -> Value {
    Value::Array(rows)
}

#[test]
fn wide_link_lists_obey_the_traversal_budget() {
    let mut source = Memory::default();
    source.put(
        "a",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            arrays(vec![edge(local(1), 7, local(2), 0); MAX_GRAPH_OBJECTS + 1]),
        )],
    );
    source.put("a", 1, "Source", &[]);
    source.put("a", 2, "Target", &[]);
    let mut site = Site::root("a");
    site.key.1 = 1;
    assert!(
        Graph::new(&mut source)
            .links(&site, 7)
            .unwrap_err()
            .to_string()
            .contains("bound")
    );
}

#[test]
fn prefab_occurrences_are_distinct_and_layer_edges_use_the_scene_bus() {
    let mut source = Memory::default();
    source.put(
        "a",
        0,
        "SubWorldData",
        &[
            ("Objects", arrays(vec![local(1), local(2), local(3)])),
            (
                "LinkConnections",
                arrays(vec![edge(ptr("c", 1), 10, local(4), 0)]),
            ),
        ],
    );
    for i in [1, 2] {
        source.put(
            "a",
            i,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("b", 0))],
        );
    }
    source.put(
        "a",
        3,
        "WorldPartReferenceObjectData",
        &[("Blueprint", ptr("c", 0))],
    );
    source.put("a", 4, "Target", &[]);
    source.put("b", 0, "LogicPrefabBlueprint", &[("Object", local(1))]);
    source.put("b", 1, "Leaf", &[]);
    source.put(
        "c",
        0,
        "WorldPartData",
        &[("Objects", arrays(vec![local(1)]))],
    );
    source.put("c", 1, "LayerSource", &[]);
    let mut graph = Graph::new(&mut source);
    let nodes = graph.read(Site::root("a")).unwrap();
    let leaves: Vec<_> = nodes.iter().filter(|n| n.object.class == "Leaf").collect();
    assert_eq!(leaves.len(), 2);
    assert_eq!(leaves[0].site.key, leaves[1].site.key);
    assert_ne!(leaves[0].site, leaves[1].site);
    let layer = &nodes
        .iter()
        .find(|n| n.object.class == "LayerSource")
        .unwrap()
        .site;
    assert_eq!(layer.bus, ("a".into(), 0));
    assert!(layer.parents.is_empty());
    assert_eq!(graph.links(layer, 10).unwrap()[0].site.key, ("a".into(), 4));
}

#[test]
fn links_cross_subworld_interfaces_and_properties_return_through_prefabs() {
    let mut source = Memory::default();
    source.put(
        "a",
        0,
        "LevelData",
        &[
            (
                "LinkConnections",
                arrays(vec![edge(local(1), 8, local(2), 9)]),
            ),
            (
                "PropertyConnections",
                arrays(vec![edge(local(3), 44, local(1), 50)]),
            ),
        ],
    );
    source.put(
        "a",
        1,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("b", 0))],
    );
    source.put(
        "a",
        2,
        "SubWorldReferenceObjectData",
        &[("BundleName", Value::String("c".into()))],
    );
    source.put("a", 3, "BoolEntityData", &[]);
    source.put(
        "b",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Interface", local(1)),
            (
                "LinkConnections",
                arrays(vec![edge(local(2), 7, local(1), 8)]),
            ),
            (
                "PropertyConnections",
                arrays(vec![edge(local(1), 50, local(2), 51)]),
            ),
        ],
    );
    source.put("b", 1, "InterfaceDescriptorData", &[]);
    source.put("b", 2, "Source", &[]);
    source.put(
        "c",
        0,
        "SubWorldData",
        &[
            ("Interface", local(1)),
            (
                "LinkConnections",
                arrays(vec![edge(local(1), 9, local(2), 0)]),
            ),
        ],
    );
    source.put("c", 1, "InterfaceDescriptorData", &[]);
    source.put("c", 2, "Target", &[]);
    let site = Site {
        key: ("b".into(), 2),
        bus: ("b".into(), 0),
        parents: vec![Frame {
            bus: ("a".into(), 0),
            reference: ("a".into(), 1),
        }],
    };
    let mut graph = Graph::new(&mut source);
    let links = graph.links(&site, 7).unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].site.key, ("c".into(), 2));
    assert_eq!(
        links[0].site.parents,
        vec![Frame {
            bus: ("a".into(), 0),
            reference: ("a".into(), 2)
        }]
    );
    let input = graph.input(&site, 51).unwrap();
    assert_eq!(input.site.key, ("a".into(), 3));
    assert_eq!(input.port, 44);
    assert!(input.site.parents.is_empty());
}

#[test]
fn malformed_composition_and_connection_cycles_are_bounded_errors() {
    let mut source = Memory::default();
    source.put("a", 0, "Blueprint", &[("Object", local(0))]);
    assert!(
        Graph::new(&mut source)
            .read(Site::root("a"))
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
    source.put(
        "a",
        0,
        "Blueprint",
        &[("Objects", Value::Array(vec![Value::Int32(3)]))],
    );
    assert!(Graph::new(&mut source).read(Site::root("a")).is_err());
    source.put("a", 0, "Blueprint", &[("Object", local(1))]);
    for i in 1..=MAX_GRAPH_DEPTH {
        source.put("a", i, "Node", &[("Object", local(i + 1))]);
    }
    assert!(
        Graph::new(&mut source)
            .read(Site::root("a"))
            .unwrap_err()
            .to_string()
            .contains("bound")
    );
    source.put(
        "a",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            arrays(vec![edge(local(1), 7, local(1), 7)]),
        )],
    );
    source.put(
        "a",
        1,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("b", 0))],
    );
    source.put(
        "b",
        0,
        "Blueprint",
        &[
            ("Interface", local(1)),
            (
                "LinkConnections",
                arrays(vec![edge(local(1), 7, local(1), 7)]),
            ),
        ],
    );
    source.put("b", 1, "InterfaceDescriptorData", &[]);
    let mut site = Site::root("a");
    site.key.1 = 1;
    assert!(
        Graph::new(&mut source)
            .links(&site, 7)
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
}

#[test]
fn missing_duplicate_property_inputs_and_unbound_interfaces_fail() {
    for edges in [
        vec![],
        vec![
            edge(local(1), 4, local(2), 5),
            edge(local(1), 4, local(2), 5),
        ],
    ] {
        let mut source = Memory::default();
        source.put(
            "a",
            0,
            "Blueprint",
            &[("PropertyConnections", arrays(edges))],
        );
        source.put("a", 1, "Bool", &[]);
        source.put("a", 2, "Target", &[]);
        let mut site = Site::root("a");
        site.key.1 = 2;
        assert!(Graph::new(&mut source).input(&site, 5).is_err());
    }
    let mut source = Memory::default();
    source.put(
        "a",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            arrays(vec![edge(local(2), 1, local(1), 2)]),
        )],
    );
    source.put("a", 1, "InterfaceDescriptorData", &[]);
    source.put("a", 2, "Source", &[]);
    let mut site = Site::root("a");
    site.key.1 = 2;
    assert!(
        Graph::new(&mut source)
            .links(&site, 1)
            .unwrap_err()
            .to_string()
            .contains("unbound root")
    );
}
