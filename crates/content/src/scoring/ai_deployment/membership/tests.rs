// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::graph::tests::{Memory, edge, local, ptr};
use nfs_frostbite::ebx::Value;

fn fixture(target: usize, dispatches: usize) -> Memory {
    let mut m = Memory::default();
    m.put(
        "s",
        0,
        "Blueprint",
        &[
            ("Objects", Value::Array(vec![local(1), local(2), local(4)])),
            (
                "LinkConnections",
                Value::Array(vec![edge(local(2), 77, local(target), 0)]),
            ),
        ],
    );
    m.put("s", 1, "ParticipantSetEntityData", &[]);
    m.put("s", 4, "ParticipantSetEntityData", &[]);
    m.put(
        "s",
        2,
        "LogicPrefabReferenceObjectData",
        &[("Blueprint", ptr("o", 0))],
    );
    let mut objects = vec![local(1), local(2), local(3)];
    let mut links = vec![
        edge(local(1), COP_SET, local(3), 77),
        edge(local(2), COP_SET, local(3), 77),
    ];
    if dispatches == 2 {
        objects.push(local(4));
        links.push(edge(local(4), COP_SET, local(3), 77));
        m.put("o", 4, "CopDispatchManagerEntityData", &[]);
    }
    m.put(
        "o",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Objects", Value::Array(objects)),
            ("Interface", local(3)),
            ("LinkConnections", Value::Array(links)),
        ],
    );
    m.put(
        "o",
        1,
        if dispatches == 0 {
            "TrafficManagerEntityData"
        } else {
            "CopDispatchManagerEntityData"
        },
        &[],
    );
    m.put("o", 2, "RoadblockManagerEntityData", &[]);
    m.put("o", 3, "InterfaceDescriptorData", &[]);
    m
}

#[test]
fn mutable_owner_follows_prefab_interface_and_rejects_unbound_or_ambiguous_sources() {
    let set = Site {
        key: ("s".into(), 1),
        ..Site::root("s")
    };
    for (target, dispatches, expected) in
        [(1, 1, true), (4, 1, false), (1, 0, false), (1, 2, false)]
    {
        let mut source = fixture(target, dispatches);
        let actual = cops(&mut Graph::new(&mut source), &set);
        assert_eq!(
            actual.is_ok(),
            expected,
            "{target}/{dispatches}: {actual:?}"
        );
        if let Ok(Owner::Cops {
            dispatch,
            roadblocks,
        }) = actual
        {
            assert_eq!(dispatch.asset, "o");
            assert_eq!(dispatch.index, 1);
            assert_eq!(roadblocks.index, 2);
            assert_eq!(dispatch.instances, vec![("s".into(), 2)]);
            assert_eq!(dispatch.instances, roadblocks.instances);
        }
    }
}

#[test]
fn resolved_membership_keeps_manager_source_not_adapter_or_asset_match() {
    let mut m = Memory::default();
    m.put(
        "s",
        0,
        "Blueprint",
        &[(
            "LinkConnections",
            Value::Array(vec![edge(local(1), 3_806_204_033, local(2), 0)]),
        )],
    );
    m.put("s", 1, "ParticipantSetAllAiAdapterEntityData", &[]);
    m.put("s", 2, "AiParticipantManagerEntityData", &[]);
    m.put("s", 3, "AiParticipantManagerEntityData", &[]);
    let mut graph = Graph::new(&mut m);
    let root = Site {
        key: ("s".into(), 1),
        ..Site::root("s")
    };
    let resolved = sets::read_set_sources(
        &mut graph,
        root,
        &BTreeMap::new(),
        &mut |_, _| unreachable!(),
    )
    .unwrap();
    let mut requested = Vec::new();
    let bindings = bind(&mut graph, &resolved, &mut |_, site| {
        requested.push(site.clone());
        Ok(SceneEndpoint {
            scene_key: 100,
            serializer: 7,
        })
    })
    .unwrap();
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].key, ("s".into(), 2));
    assert_eq!(
        bindings[0],
        Binding {
            fact: Fact::Ai(Identity::from(&requested[0])),
            owner: Owner::Ai {
                manager: SceneEndpoint {
                    scene_key: 100,
                    serializer: 7
                }
            },
        }
    );
    assert!(resolved.program.evaluate(|_| None).is_err());
}
