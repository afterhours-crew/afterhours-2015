// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local};
use nfs_frostbite::ebx::Value;

fn fixture() -> (Memory, Links) {
    let mut m = Memory::default();
    m.put(
        "a",
        0,
        "SubWorldData",
        &[
            ("Objects", Value::Array((1..=7).map(local).collect())),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 10, local(5), 71),
                    edge(local(1), 10, local(4), 0),
                    edge(local(1), 20, local(2), 0),
                    edge(local(2), 30, local(7), 0),
                    edge(local(2), 30, local(3), 0),
                ]),
            ),
        ],
    );
    for (i, class) in [
        (1, "TrafficManagerEntityData"),
        (2, "PersonaSetEntityData"),
        (3, "PersonaEntityData"),
        (4, "TrafficSplineEntityData"),
        (6, "TrafficSplineEntityData"),
        (7, "PersonaEntityData"),
    ] {
        m.put("a", i, class, &[]);
    }
    m.put(
        "a",
        5,
        "SubWorldReferenceObjectData",
        &[("BundleName", Value::String("b".into()))],
    );
    m.put(
        "b",
        0,
        "SubWorldData",
        &[
            ("Interface", local(1)),
            (
                "LinkConnections",
                Value::Array(vec![
                    edge(local(1), 71, local(3), 0),
                    edge(local(1), 71, local(2), 0),
                ]),
            ),
        ],
    );
    m.put("b", 1, "InterfaceDescriptorData", &[]);
    m.put("b", 2, "TrafficSplineEntityData", &[]);
    m.put("b", 3, "TrafficSplineEntityData", &[]);
    (
        m,
        Links {
            roads: 10,
            provider: 20,
            personas: 30,
        },
    )
}
#[test]
fn linked_roads_cross_scene_interfaces_and_provider_order_is_independent() {
    let (mut m, ports) = fixture();
    let result = resolve(
        &mut Graph::new(&mut m),
        Site::root("a"),
        &BTreeSet::from(["a".into(), "b".into()]),
        ports,
    )
    .unwrap();
    assert_eq!(
        result.roads,
        vec![
            ("b".into(), ("b".into(), 3)),
            ("b".into(), ("b".into(), 2)),
            ("a".into(), ("a".into(), 4)),
        ]
    );
    assert_eq!(result.personas, vec![("a".into(), 7), ("a".into(), 3)]);
    // The unlinked road a:6 is deliberately absent despite being constructed.
}
#[test]
fn unregistered_scene_wrong_provider_and_duplicate_links_fail_closed() {
    for variant in 0..4 {
        let (mut m, ports) = fixture();
        let mut scenes = BTreeSet::from(["a".into(), "b".into()]);
        match variant {
            0 => {
                scenes.remove("b");
            }
            1 => m.put("a", 2, "FutureVehicleProviderEntityData", &[]),
            2 => {
                let root = m.0.get_mut(&("a".into(), 0)).unwrap();
                let Value::Array(edges) = &mut root
                    .fields
                    .0
                    .iter_mut()
                    .find(|(n, _)| n == "LinkConnections")
                    .unwrap()
                    .1
                else {
                    panic!()
                };
                edges.push(edge(local(2), 30, local(3), 0));
            }
            _ => m.put("b", 2, "ScriptedAISplineEntityData", &[]),
        }
        assert!(
            resolve(&mut Graph::new(&mut m), Site::root("a"), &scenes, ports).is_err(),
            "{variant}"
        );
    }
}
