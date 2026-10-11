// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
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
fn session() -> Session {
    Session::new(&Config::new([1; 32]), binding()).unwrap()
}
fn request(correlation: u32, target: u64) -> Vec<u8> {
    let b = binding();
    let q = UpdateMeshConnectionRequest {
        game_id: Some(b.game),
        source_group_id: Some(ObjectId(30722, 2, b.local_connection as i64)),
        target_group_id: Some(ObjectId(30722, 2, target as i64)),
        player_net_connection_flags: Some(0),
        player_net_connection_status: Some(2),
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
            routing_b: 29,
            correlation,
            ..Default::default()
        },
    )
    .unwrap()
}
fn gate() -> InitialReliableSyncPrerequisite {
    InitialReliableSyncPrerequisite::Published { binding: binding() }
}
fn prepare(s: &mut Session) -> PreparedBatch {
    let Decision::Prepared(batch) = s.request(&request(5, 29), gate(), Some(2000)).unwrap() else {
        panic!("prepared")
    };
    batch
}
#[test]
fn sync_pending_resume_and_full_write_order() {
    let mut s = session();
    let q = request(5, 29);
    assert!(matches!(
        s.request(&q, InitialReliableSyncPrerequisite::Pending, None)
            .unwrap(),
        Decision::PendingPrerequisite
    ));
    assert_eq!(s.admitted_requests(), 1);
    assert!(s.has_pending_request());
    assert!(matches!(
        s.resume(InitialReliableSyncPrerequisite::Pending, None)
            .unwrap(),
        Decision::PendingPrerequisite
    ));
    assert_eq!(s.admitted_requests(), 1);
    let Decision::Prepared(batch) = s.resume(gate(), Some(2000)).unwrap() else {
        panic!("prepared")
    };
    let frames = batch.frames();
    assert_eq!(frames.len(), 3);
    let ack = frame(&frames[0], 29, 1).unwrap();
    assert_eq!(ack.fields.correlation, 5);
    assert!(ack.body.is_empty());
    let state =
        NotifyGamePlayerStateChange::decode(frame(&frames[1], 116, 2).unwrap().body, body_limits())
            .unwrap();
    assert_eq!(
        (state.game_id, state.player_id, state.player_state),
        (Some(300), Some(7), Some(4))
    );
    let join =
        NotifyPlayerJoinCompleted::decode(frame(&frames[2], 30, 2).unwrap().body, body_limits())
            .unwrap();
    assert_eq!(
        (join.game_id, join.player_id, join.joined_game_timestamp),
        (Some(300), Some(7), Some(2000))
    );
    assert!(s.emitted_join_time().is_none());
    assert!(s.take_continuation_permit().is_none());
    assert!(matches!(
        s.request(&q, gate(), Some(9000)).unwrap(),
        Decision::PendingWrite
    ));
    s.commit_written(batch).unwrap();
    assert_eq!(s.emitted_join_time(), Some(2000));
    let permit = s.take_continuation_permit().unwrap();
    assert_eq!(permit.binding(), binding());
    assert!(s.take_continuation_permit().is_none());
    assert_eq!(permit.consume([1; 32]).unwrap(), binding());
    let Decision::Ack(ack) = s.request(&request(6, 29), gate(), Some(9000)).unwrap() else {
        panic!("ACK only")
    };
    assert_eq!(frame(&ack, 29, 1).unwrap().fields.correlation, 6);
    assert_eq!(s.emitted_join_time(), Some(2000));
}
#[test]
fn dropped_foreign_and_failed_writes_revoke_readiness() {
    let mut dropped = session();
    drop(prepare(&mut dropped));
    assert_eq!(
        dropped
            .request(&request(5, 29), gate(), Some(2000))
            .unwrap_err(),
        Error::Write
    );
    assert!(dropped.emitted_join_time().is_none());
    let mut a = session();
    let mut b = session();
    let ticket_a = prepare(&mut a);
    let ticket_b = prepare(&mut b);
    assert_eq!(b.commit_written(ticket_a), Err(Error::Write));
    drop(ticket_b);
    assert!(b.take_continuation_permit().is_none());
    let mut failed = session();
    let ticket = prepare(&mut failed);
    failed.abort_write();
    assert_eq!(failed.commit_written(ticket), Err(Error::Write));
    assert!(failed.take_continuation_permit().is_none());
    let mut pending = session();
    pending
        .request(
            &request(5, 29),
            InitialReliableSyncPrerequisite::Pending,
            None,
        )
        .unwrap();
    pending.abort_write();
    assert!(!pending.has_pending_request());
    assert_eq!(
        pending.resume(gate(), Some(2000)).unwrap_err(),
        Error::Closed
    );
}
#[test]
fn proof_identity_clock_and_permit_association_are_independent() {
    let mut foreign = binding();
    foreign.game += 1;
    assert_eq!(
        session()
            .request(
                &request(5, 29),
                InitialReliableSyncPrerequisite::Published { binding: foreign },
                Some(2000)
            )
            .unwrap_err(),
        Error::Prerequisite
    );
    for time in [None, Some(0), Some(1000)] {
        assert_eq!(
            session()
                .request(&request(5, 29), gate(), time)
                .unwrap_err(),
            Error::Clock
        );
    }
    let mut s = session();
    let ticket = prepare(&mut s);
    s.commit_written(ticket).unwrap();
    assert_eq!(
        s.take_continuation_permit().unwrap().consume([2; 32]),
        Err(Error::Source)
    );
    let mut invalid = binding();
    invalid.local_connection = invalid.host_connection;
    assert!(Session::new(&Config::new([1; 32]), invalid).is_err());
}
#[test]
fn malformed_partial_and_foreign_requests_never_emit() {
    let q = request(5, 29);
    for end in 0..q.len() {
        let mut s = session();
        assert_eq!(
            s.request(&q[..end], gate(), Some(2000)).unwrap_err(),
            Error::Request
        );
        assert!(s.emitted_join_time().is_none());
    }
    for bad in [
        request(0, 29),
        request(5, 99),
        [q.clone(), q.clone()].concat(),
    ] {
        assert!(session().request(&bad, gate(), Some(2000)).is_err());
    }
    let f = frame(&q, 29, 0).unwrap();
    let mut model = UpdateMeshConnectionRequest::decode(f.body, body_limits()).unwrap();
    model.player_net_connection_status = Some(0);
    let bad = wire(&model.encode(body_limits()).unwrap(), f.fields).unwrap();
    assert!(session().request(&bad, gate(), Some(2000)).is_err());
    let s = session();
    assert!(s.is_self_mesh(&request(5, 9)));
    assert!(!s.is_self_mesh(&q));
}
#[test]
fn admitted_retry_bound_and_pending_backpressure() {
    let mut s = session();
    let ticket = prepare(&mut s);
    s.commit_written(ticket).unwrap();
    for correlation in 6..6 + MAX_HOST_REQUESTS as u32 - 1 {
        assert!(matches!(
            s.request(&request(correlation, 29), gate(), None).unwrap(),
            Decision::Ack(_)
        ));
    }
    assert_eq!(s.admitted_requests(), MAX_HOST_REQUESTS);
    assert_eq!(
        s.request(&request(99, 29), gate(), None).unwrap_err(),
        Error::Bound
    );
    let mut s = session();
    s.request(
        &request(5, 29),
        InitialReliableSyncPrerequisite::Pending,
        None,
    )
    .unwrap();
    assert_eq!(
        s.request(&request(6, 29), gate(), Some(2000)).unwrap_err(),
        Error::AdmissionBusy
    );
}
