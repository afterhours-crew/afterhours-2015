// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::graph::{Frame, Site, Source, tests::Memory};
use nfs_frostbite::ebx::Value;

fn node(instance: usize, class: &str, rpc: bool) -> Node {
    let mut source = Memory::default();
    source.put(
        "prefab",
        2,
        class,
        &[
            ("HandlesMarkedForDelete", Value::Bool(true)),
            ("SendRpcToClientSide", Value::Bool(rpc)),
        ],
    );
    Node {
        site: Site {
            key: ("prefab".into(), 2),
            bus: ("prefab".into(), 0),
            parents: vec![Frame {
                bus: ("scene".into(), 0),
                reference: ("scene".into(), instance),
            }],
        },
        object: source.object(&("prefab".into(), 2)).unwrap(),
    }
}

#[test]
fn scene_membership_preserves_occurrences_flags_and_service_ownership() {
    let first = node(4, "ParticipantStateEntityData", true);
    let second = node(7, "StateSequenceParticipantStateEntityData", false);
    let membership = extract(71, [(&first, Some(8)), (&second, Some(19))]).unwrap();
    assert_eq!(membership.content_key, 71);
    assert_ne!(membership.states[0].identity, membership.states[1].identity);
    assert_eq!(membership.states[1].class, second.object.class);
    let scope = membership.scope();
    assert_eq!(
        scope
            .states
            .iter()
            .map(|s| s.serializer)
            .collect::<Vec<_>>(),
        [8, 19]
    );
    assert!(
        scope
            .states
            .iter()
            .all(|s| s.kind == StateKind::HostOwned && s.handles_marked_for_delete)
    );
    assert!(scope.states[0].send_rpc_to_client);
    assert!(!scope.states[1].send_rpc_to_client);
    assert!(scope.transitions.is_empty() && scope.imports.is_empty());
    assert!(scope.machine().is_ok());
}

#[test]
fn scene_membership_rejects_missing_unknown_or_ambiguous_state_contracts() {
    let mut first = node(4, "ParticipantStateEntityData", true);
    let second = node(7, "StateSequenceParticipantStateEntityData", false);
    assert!(extract(71, [(&first, None)]).is_err());
    assert!(extract(71, [(&first, Some(0))]).is_err());
    assert!(extract(71, [(&first, Some(usize::from(u16::MAX) + 1))]).is_err());
    assert!(extract(71, [(&first, Some(8)), (&second, Some(8))]).is_err());
    assert!(extract(71, [(&first, Some(8)), (&first, Some(9))]).is_err());
    first
        .object
        .fields
        .0
        .retain(|(name, _)| name != "SendRpcToClientSide");
    assert!(extract(71, [(&first, Some(8))]).is_err());
    let unknown = node(4, "UnknownParticipantStateEntityData", true);
    assert!(extract(71, [(&unknown, Some(8))]).is_err());
    let unrelated = node(4, "BooleanPropertyEntityData", true);
    assert!(extract(71, [(&unrelated, None)]).unwrap().states.is_empty());
}
