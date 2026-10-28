// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::world_readiness::frame;

fn binding() -> Binding {
    Binding {
        group: 100,
        matchmaking: 200,
        game: 300,
        persona: 7,
        user_session: 8,
        local_connection: 9,
        host_connection: 29,
        baseline_join_micros: 1000,
    }
}
fn departure() -> Departure {
    Departure::new(binding()).unwrap()
}
fn leave_with(correlation: u32, edit: impl FnOnce(&mut LeaveGameByGroupRequest<'_>)) -> Vec<u8> {
    let b = binding();
    let mut q = LeaveGameByGroupRequest {
        blaze_object_type_and_id: Some(ObjectId(30722, 2, b.local_connection as i64)),
        player_removed_title_context: Some(0),
        game_id: Some(b.game),
        player_id: Some(b.persona),
        player_removed_reason: Some(GROUP_LEFT),
        title_context_string: Some(b""),
        ..Default::default()
    };
    edit(&mut q);
    wire(
        &q.encode(body_limits()).unwrap(),
        Fields {
            routing_a: 4,
            routing_b: LEAVE_GAME_BY_GROUP,
            correlation,
            ..Default::default()
        },
    )
    .unwrap()
}
fn leave(correlation: u32) -> Vec<u8> {
    leave_with(correlation, |_| {})
}
fn mesh(correlation: u32, game: u64, target: i64, status: i32, flags: u32) -> Vec<u8> {
    let q = UpdateMeshConnectionRequest {
        game_id: Some(game),
        source_group_id: Some(ObjectId(30722, 2, binding().local_connection as i64)),
        target_group_id: Some(ObjectId(30722, 2, target)),
        player_net_connection_flags: Some(flags),
        player_net_connection_status: Some(status),
        qos_info: Some(MeshConnectionQosInfo {
            packet_loss: Some(PacketLossBits(0)),
            latency_ms: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    wire(
        &q,
        Fields {
            routing_a: 4,
            routing_b: UPDATE_MESH_CONNECTION,
            correlation,
            ..Default::default()
        },
    )
    .unwrap()
}
fn assert_ack(frame_bytes: &[u8], command: u16, correlation: u32) {
    let f = frame(frame_bytes, command, 1).unwrap();
    assert_eq!(f.fields.correlation, correlation);
    assert!(f.body.is_empty());
}

#[test]
fn leave_acknowledges_then_notifies_player_left_after_the_write() {
    let mut d = departure();
    assert!(owns(&leave(60)));
    let out = d.reply(&leave(60)).unwrap();
    assert_eq!(out.len(), 2);
    assert_ack(&out[0], LEAVE_GAME_BY_GROUP, 60);
    let n = frame(&out[1], NOTIFY_PLAYER_REMOVED, 2).unwrap();
    let n = NotifyPlayerRemoved::decode(n.body, body_limits()).unwrap();
    assert_eq!(
        (
            n.player_removed_title_context,
            n.game_id,
            n.is_lockable_for_preferred_joins,
            n.player_id,
            n.player_removed_reason
        ),
        (Some(0), Some(300), Some(false), Some(7), Some(PLAYER_LEFT))
    );
    assert!(!d.left() && d.pending());
    d.committed().unwrap();
    assert!(d.left() && !d.pending());
    let again = d.reply(&leave(61)).unwrap();
    assert_eq!(again.len(), 1);
    assert_ack(&again[0], LEAVE_GAME_BY_GROUP, 61);
    d.committed().unwrap();
}

#[test]
fn disconnected_reports_for_the_current_meshes_are_acknowledged() {
    let mut d = departure();
    let b = binding();
    for (corr, game, target, flags) in [
        (61, b.group, b.local_connection as i64, 193),
        (62, b.game, b.host_connection as i64, 0),
        (63, b.game, 0, 0),
    ] {
        let q = mesh(corr, game, target, DISCONNECTED, flags);
        assert!(owns(&q));
        let out = d.reply(&q).unwrap();
        assert_eq!(out.len(), 1);
        assert_ack(&out[0], UPDATE_MESH_CONNECTION, corr);
        d.committed().unwrap();
    }
    assert!(!d.left());
}

#[test]
fn foreign_or_connected_reports_and_altered_leaves_are_not_mine() {
    let mut d = departure();
    let b = binding();
    assert!(!owns(&mesh(5, b.game, b.host_connection as i64, 2, 0)));
    for q in [
        mesh(5, b.game, b.host_connection as i64, 2, 0),
        mesh(5, b.game + 1, b.host_connection as i64, DISCONNECTED, 0),
        mesh(5, b.group, b.host_connection as i64, DISCONNECTED, 0),
        mesh(5, b.game, 77, DISCONNECTED, 0),
        mesh(0, b.game, 0, DISCONNECTED, 0),
        leave(0),
        leave_with(5, |q| q.game_id = Some(301)),
        leave_with(5, |q| q.player_id = Some(8)),
        leave_with(5, |q| q.player_removed_reason = Some(PLAYER_LEFT)),
        leave_with(5, |q| q.title_context_string = Some(b"x")),
        leave_with(5, |q| q.player_removed_title_context = Some(1)),
        leave_with(5, |q| {
            q.blaze_object_type_and_id = Some(ObjectId(30722, 2, 10))
        }),
        leave_with(5, |q| q.title_context_string = None),
    ] {
        assert_eq!(d.reply(&q), Err(Error::Ineligible));
    }
    assert!(!d.pending() && !d.left());
    let q = leave(5);
    assert_eq!(
        d.reply(&[q.clone(), q.clone()].concat()),
        Err(Error::Ineligible)
    );
    for split in 0..q.len() {
        assert_eq!(d.reply(&q[..split]), Err(Error::Ineligible));
    }
    assert!(d.reply(&q).is_ok());
}

#[test]
fn an_unwritten_batch_or_failed_write_closes_without_leaving() {
    let mut d = departure();
    d.reply(&leave(60)).unwrap();
    assert_eq!(d.reply(&leave(61)), Err(Error::Write));
    assert!(!d.left());
    assert_eq!(d.reply(&leave(62)), Err(Error::Closed));
    let mut d = departure();
    d.reply(&leave(60)).unwrap();
    d.abort_write();
    assert!(!d.left());
    assert_eq!(d.committed(), Err(Error::Write));
    assert_eq!(d.reply(&leave(60)), Err(Error::Closed));
}

#[test]
fn mesh_reports_are_bounded_and_bindings_validated() {
    let mut d = departure();
    let b = binding();
    for corr in 1..=MAX_MESH_REPORTS as u32 {
        d.reply(&mesh(corr, b.game, 0, DISCONNECTED, 0)).unwrap();
        d.committed().unwrap();
    }
    assert_eq!(
        d.reply(&mesh(99, b.game, 0, DISCONNECTED, 0)),
        Err(Error::Bound)
    );
    let mut bad = binding();
    bad.game = bad.group;
    assert!(Departure::new(bad).is_err());
    bad = binding();
    bad.baseline_join_micros = 0;
    assert!(Departure::new(bad).is_err());
}

/// The client leaves its group in the world and creates a new one (official
/// E1026 1311.173-1311.367 s): ACK, player removed for the group with `LFPJ`
/// 1, the old group's disconnected report to no connection; then the new
/// group can be left as well (1972.342 s) and the world game stays bound.
#[test]
fn the_group_is_left_and_rebound_to_the_new_group() {
    let mut d = departure();
    let b = binding();
    let q = leave_with(275, |q| q.game_id = Some(b.group));
    assert!(owns(&q));
    // The old group's disconnected report to no connection is not mine
    // while the group is current.
    assert_eq!(
        d.reply(&mesh(5, b.group, 0, DISCONNECTED, 0)),
        Err(Error::Ineligible)
    );
    let out = d.reply(&q).unwrap();
    assert_eq!(out.len(), 2);
    assert_ack(&out[0], LEAVE_GAME_BY_GROUP, 275);
    let n = frame(&out[1], NOTIFY_PLAYER_REMOVED, 2).unwrap();
    let n = NotifyPlayerRemoved::decode(n.body, body_limits()).unwrap();
    assert_eq!(
        (
            n.game_id,
            n.is_lockable_for_preferred_joins,
            n.player_removed_reason
        ),
        (Some(b.group), Some(true), Some(PLAYER_LEFT))
    );
    assert_eq!(d.rebind_group(500), Err(Error::Context), "not yet written");
    d.committed().unwrap();
    assert!(d.group_left() && !d.left());
    let out = d.reply(&mesh(279, b.group, 0, DISCONNECTED, 0)).unwrap();
    assert_ack(&out[0], UPDATE_MESH_CONNECTION, 279);
    d.committed().unwrap();
    assert_eq!(d.rebind_group(0), Err(Error::Context));
    assert_eq!(d.rebind_group(b.group), Err(Error::Context));
    d.rebind_group(500).unwrap();
    assert!(!d.group_left());
    assert_eq!(d.reply(&q), Err(Error::Ineligible), "the old group is gone");
    let out = d
        .reply(&leave_with(281, |q| q.game_id = Some(500)))
        .unwrap();
    assert_eq!(out.len(), 2);
    d.committed().unwrap();
    let out = d.reply(&leave(282)).unwrap();
    let n = frame(&out[1], NOTIFY_PLAYER_REMOVED, 2).unwrap();
    let n = NotifyPlayerRemoved::decode(n.body, body_limits()).unwrap();
    assert_eq!(
        (n.game_id, n.is_lockable_for_preferred_joins),
        (Some(300), Some(false))
    );
}

#[test]
fn retired_group_disconnects_survive_both_renewal_orders_and_retries() {
    for disconnect_first in [false, true] {
        let mut d = departure();
        let b = binding();
        d.reply(&leave_with(275, |q| q.game_id = Some(b.group)))
            .unwrap();
        d.committed().unwrap();
        let q = mesh(279, b.group, 0, DISCONNECTED, 0);
        if disconnect_first {
            assert_eq!(d.reply(&q).unwrap().len(), 1);
            d.committed().unwrap();
        }
        d.rebind_group(500).unwrap();
        for _ in 0..3 {
            let out = d.reply(&q).unwrap();
            assert_ack(&out[0], UPDATE_MESH_CONNECTION, 279);
            d.committed().unwrap();
            assert!(!d.group_left() && !d.left());
        }
        // A foreign group, source/target or still-connected report is not a
        // retired departure and must never be acknowledged by this service.
        for invalid in [
            mesh(280, 101, 0, DISCONNECTED, 0),
            mesh(280, b.group, 77, DISCONNECTED, 0),
            mesh(280, b.group, 0, 2, 0),
        ] {
            assert_eq!(d.reply(&invalid), Err(Error::Ineligible));
        }
        d.reply(&leave_with(281, |q| q.game_id = Some(500)))
            .unwrap();
        d.committed().unwrap();
        assert!(d.group_left());
        d.rebind_group(600).unwrap();
        // Another renewal retains only its immediately preceding group.
        assert_eq!(d.reply(&q), Err(Error::Ineligible));
        d.reply(&mesh(282, 500, 0, DISCONNECTED, 0)).unwrap();
        d.committed().unwrap();
        assert!(!d.group_left());
    }
}

#[test]
fn retired_group_reports_keep_the_existing_bound_and_account_isolation() {
    let mut d = departure();
    d.reply(&leave_with(1, |q| q.game_id = Some(binding().group)))
        .unwrap();
    d.committed().unwrap();
    d.rebind_group(500).unwrap();
    let q = mesh(2, binding().group, 0, DISCONNECTED, 0);
    let mut other = Departure::new(Binding {
        group: 101,
        local_connection: 10,
        ..binding()
    })
    .unwrap();
    assert_eq!(other.reply(&q), Err(Error::Ineligible));
    for _ in 0..MAX_MESH_REPORTS {
        d.reply(&q).unwrap();
        d.committed().unwrap();
    }
    assert_eq!(d.reply(&q), Err(Error::Bound));
    assert!(!d.group_left());
}
