// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
fn current() -> Current {
    Current {
        group_id: 100,
        session_id: 200,
        game_id: 300,
        persona_id: 7,
        user_session_id: 8,
        local_connection_group_id: 9,
        host_connection_group_id: 10,
        scenario_id: 0,
    }
}
fn query(
    c: Current,
    corr: u32,
    edit: impl FnOnce(&mut UpdateMeshConnectionRequest<'_>),
) -> Vec<u8> {
    let mut q = UpdateMeshConnectionRequest {
        game_id: Some(c.game_id),
        source_group_id: Some(ObjectId(30722, 2, c.local_connection_group_id as i64)),
        target_group_id: Some(ObjectId(30722, 2, c.local_connection_group_id as i64)),
        player_net_connection_flags: Some(0),
        player_net_connection_status: Some(2),
        qos_info: Some(MeshConnectionQosInfo {
            packet_loss: Some(PacketLossBits(0)),
            latency_ms: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    };
    edit(&mut q);
    wire(
        &q.encode(body_limits()).unwrap(),
        Fields {
            routing_a: 4,
            routing_b: 29,
            correlation: corr,
            ..Default::default()
        },
    )
    .unwrap()
}
fn session() -> Session {
    Session::new(&Config, current()).unwrap()
}
#[test]
fn typed_validation_precedes_ack_and_publishes_after_full_write() {
    let mut s = session();
    let out = s.reply(&query(current(), 51, |_| {})).unwrap();
    assert_eq!(out.len(), 2);
    let n = NotifyMatchmakingSessionConnectionValidated::decode(
        frame(&out[0], 11, 2).unwrap().body,
        body_limits(),
    )
    .unwrap();
    assert_eq!(n.game_id, Some(300));
    assert_eq!(n.session_id, Some(200));
    assert_eq!(n.user_session_id, Some(8));
    assert_eq!(n.scenario_id, Some(0));
    assert_eq!(n.dispatch_session_finished, Some(true));
    assert_eq!(n.qos_validation_performed, Some(false));
    assert_eq!(n.user_group_id, Some(ObjectId(0, 0, 0)));
    let r = n.connection_validated_results.unwrap();
    assert_eq!(r.fail_count, Some(0));
    assert_eq!(r.network_topology, Some(0));
    assert_eq!(r.tier, Some(0));
    assert!(r.avoid_player_id_list.is_none() && r.avoid_game_id_list.is_none());
    assert!(frame(&out[1], 29, 1).unwrap().body.is_empty());
    assert_eq!(frame(&out[1], 29, 1).unwrap().fields.correlation, 51);
    assert!(!s.validation_emitted());
    assert!(s.has_pending_write());
    s.commit_after_write().unwrap();
    assert!(s.validation_emitted());
    let retry = s.reply(&query(current(), 52, |_| {})).unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(frame(&retry[0], 29, 1).unwrap().fields.correlation, 52);
    assert_eq!(s.self_request_count(), 2);
}
#[test]
fn failure_or_second_request_before_commit_revokes_publication() {
    for abort in [false, true] {
        let mut s = session();
        let q = query(current(), 1, |_| {});
        s.reply(&q).unwrap();
        if abort {
            s.abort_write()
        } else {
            assert_eq!(s.reply(&q), Err(Error::Write));
        }
        assert!(!s.validation_emitted());
        assert!(!s.has_pending_write());
        assert_eq!(s.commit_after_write(), Err(Error::Closed));
        assert_eq!(s.reply(&q), Err(Error::Closed));
    }
}
#[test]
fn malformed_frames_and_foreign_connections_close() {
    let q = query(current(), 1, |_| {});
    for size in 0..q.len() {
        let mut s = session();
        assert!(s.reply(&q[..size]).is_err());
        assert_eq!(s.reply(&q), Err(Error::Closed));
    }
    let mut concat = q.clone();
    concat.extend(&q);
    assert!(session().reply(&concat).is_err());
    for kind in 0..9 {
        let bad = query(current(), 1, |q| match kind {
            0 => q.game_id = Some(301),
            1 => q.source_group_id = Some(ObjectId(30722, 2, 11)),
            2 => q.target_group_id = Some(ObjectId(30722, 2, 10)),
            3 => q.player_net_connection_flags = Some(1),
            4 => q.player_net_connection_status = Some(0),
            5 => q.qos_info.as_mut().unwrap().latency_ms = Some(1),
            6 => q.qos_info.as_mut().unwrap().packet_loss = Some(PacketLossBits(1)),
            7 => q.qos_info = None,
            _ => q.game_id = None,
        });
        assert!(session().reply(&bad).is_err());
    }
    assert!(session().reply(&query(current(), 0, |_| {})).is_err());
}
#[test]
fn retry_bound_and_current_binding_guards() {
    let mut s = session();
    let q = query(current(), 1, |_| {});
    s.reply(&q).unwrap();
    s.commit_after_write().unwrap();
    for _ in 1..MAX_SELF_REQUESTS {
        assert_eq!(s.reply(&q).unwrap().len(), 1);
    }
    assert_eq!(s.reply(&q), Err(Error::Bound));
    assert!(!s.validation_emitted());
    for kind in 0..5 {
        let mut c = current();
        match kind {
            0 => c.persona_id = 0,
            1 => c.game_id = c.group_id,
            2 => c.session_id = 0,
            3 => c.host_connection_group_id = c.local_connection_group_id,
            _ => c.game_id = u64::MAX,
        }
        assert!(Session::new(&Config, c).is_err());
    }
}
#[test]
fn distinct_sessions_and_commit_ownership() {
    let mut a = session();
    let mut c = current();
    c.game_id = 301;
    c.user_session_id = 18;
    c.local_connection_group_id = 19;
    let mut b = Session::new(&Config, c).unwrap();
    a.reply(&query(current(), 1, |_| {})).unwrap();
    b.commit_after_write().unwrap();
    assert!(a.has_pending_write());
    assert!(!b.validation_emitted());
    let out = b.reply(&query(c, 2, |_| {})).unwrap();
    let n = NotifyMatchmakingSessionConnectionValidated::decode(
        frame(&out[0], 11, 2).unwrap().body,
        body_limits(),
    )
    .unwrap();
    assert_eq!(n.game_id, Some(301));
    assert_eq!(n.user_session_id, Some(18));
    b.commit_after_write().unwrap();
    assert!(b.validation_emitted());
    assert!(!a.validation_emitted());
    a.abort_write();
    assert!(b.validation_emitted());
}
