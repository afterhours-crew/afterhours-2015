// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::bootstrap::body_limits;
use crate::world_readiness::{self as ready, Binding, Decision, InitialReliableSyncPrerequisite};
use nfs_protocol::users::ObjectId;
fn binding() -> Binding {
    Binding {
        group: 100,
        matchmaking: 200,
        game: 300,
        persona: 7,
        user_session: 8,
        local_connection: 9,
        host_connection: 10,
        baseline_join_micros: 1000,
    }
}
fn permit(policy: [u8; 32], b: Binding) -> ready::ContinuationPermit {
    let mut s = ready::Session::new(&ready::Config::new(policy), b).unwrap();
    let body = UpdateMeshConnectionRequest {
        game_id: Some(b.game),
        source_group_id: Some(ObjectId(30722, 2, b.local_connection as i64)),
        target_group_id: Some(ObjectId(30722, 2, b.host_connection as i64)),
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
    let q = ready::wire(
        &body,
        Fields {
            routing_a: 4,
            routing_b: 29,
            correlation: 5,
            ..Default::default()
        },
    )
    .unwrap();
    let Decision::Prepared(batch) = s
        .request(
            &q,
            InitialReliableSyncPrerequisite::Published { binding: b },
            Some(2000),
        )
        .unwrap()
    else {
        panic!("ready batch")
    };
    s.commit_written(batch).unwrap();
    s.take_continuation_permit().unwrap()
}
fn query(group: u64, value: &[u8], corr: u32) -> Vec<u8> {
    let body = SetGameAttributesRequest {
        game_id: Some(group),
        game_attribs: Some(ConfigEntries(vec![(b"gameSessionId", value)])),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    ready::wire(
        &body,
        Fields {
            routing_a: 4,
            routing_b: 7,
            correlation: corr,
            ..Default::default()
        },
    )
    .unwrap()
}
fn session() -> Session {
    let mut s = Session::new(&Config::new([1; 32]));
    s.enable(permit([1; 32], binding())).unwrap();
    s
}
#[test]
fn association_publishes_after_full_pair_and_retries_only_ack() {
    let mut s = session();
    let q = query(100, b"300", 51);
    assert_eq!(s.association(), None);
    let frames = s.response(&q).unwrap().unwrap();
    assert_eq!(frames.len(), 2);
    assert!(ready::frame(&frames[0], 7, 1).unwrap().body.is_empty());
    let n = NotifyGameAttribChange::decode(
        ready::frame(&frames[1], 80, 2).unwrap().body,
        body_limits(),
    )
    .unwrap();
    assert_eq!(n.game_id, Some(100));
    assert_eq!(
        n.game_attribs.unwrap().0,
        vec![(b"gameSessionId".as_slice(), b"300".as_slice())]
    );
    assert!(s.has_pending_write());
    assert_eq!(s.association(), None);
    s.commit_after_write().unwrap();
    assert_eq!(s.association(), Some((100, 300)));
    let retry = s.response(&query(100, b"300", 52)).unwrap().unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(
        ready::frame(&retry[0], 7, 1).unwrap().fields.correlation,
        52
    );
    s.commit_after_write().unwrap();
    assert_eq!(s.association(), Some((100, 300)));
}
#[test]
fn absent_foreign_and_duplicate_permits_cannot_authorize() {
    let q = query(100, b"300", 1);
    let mut disabled = Session::new(&Config::new([1; 32]));
    assert!(disabled.response(&q).unwrap().is_none());
    assert_eq!(
        disabled.enable(permit([2; 32], binding())),
        Err(Error::Permit)
    );
    assert_eq!(disabled.response(&q), Err(Error::Closed));
    let mut zero = Session::new(&Config::new([0; 32]));
    assert_eq!(zero.enable(permit([1; 32], binding())), Err(Error::Permit));
    let mut s = session();
    assert_eq!(
        s.enable(permit([1; 32], binding())),
        Err(Error::DuplicateEnable)
    );
    assert!(!s.is_enabled());
}
#[test]
fn partial_write_abort_and_retry_before_commit_close() {
    for abort in [true, false] {
        let mut s = session();
        let q = query(100, b"300", 1);
        s.response(&q).unwrap();
        if abort {
            s.abort_write();
        } else {
            assert_eq!(s.response(&q), Err(Error::Write));
        }
        assert_eq!(s.association(), None);
        assert!(!s.has_pending_write());
        assert_eq!(s.commit_after_write(), Err(Error::Closed));
        assert_eq!(s.response(&q), Err(Error::Closed));
    }
}
#[test]
fn malformed_foreign_and_noncanonical_requests_fail_closed() {
    let q = query(100, b"300", 1);
    for length in 0..q.len() {
        let mut s = session();
        assert!(s.response(&q[..length]).is_err());
        assert_eq!(s.association(), None);
    }
    let mut concat = q.clone();
    concat.extend(&q);
    let mut wrong_key = q.clone();
    let pos = wrong_key
        .windows(13)
        .position(|v| v == b"gameSessionId")
        .unwrap();
    wrong_key[pos] = b'x';
    for bad in [
        query(101, b"300", 1),
        query(100, b"301", 1),
        query(100, b"0300", 1),
        query(100, b"300", 0),
        concat,
        wrong_key,
    ] {
        let mut s = session();
        assert!(s.response(&bad).is_err());
        assert_eq!(s.response(&q), Err(Error::Closed));
    }
}
#[test]
fn request_bound_and_cross_session_commit_isolation() {
    let q = query(100, b"300", 1);
    let mut a = session();
    let mut b = session();
    a.response(&q).unwrap();
    b.commit_after_write().unwrap();
    assert_eq!(b.association(), None);
    assert!(a.has_pending_write());
    a.commit_after_write().unwrap();
    for _ in 1..MAX_ATTRIBUTE_REQUESTS {
        assert_eq!(a.response(&q).unwrap().unwrap().len(), 1);
    }
    assert_eq!(a.response(&q), Err(Error::Bound));
    assert_eq!(a.association(), None);
    let mut s = session();
    assert_eq!(
        s.response(&[0; MAX_ATTRIBUTE_FRAME_BYTES + 1]),
        Err(Error::Bound)
    );
}
#[test]
fn distinct_worlds_remain_isolated_and_unrelated_routes_do_not_publish() {
    let mut other = binding();
    other.group = 110;
    other.game = 310;
    let mut a = session();
    let mut b = Session::new(&Config::new([1; 32]));
    b.enable(permit([1; 32], other)).unwrap();
    let unrelated = ready::wire(
        &[],
        Fields {
            routing_a: 9,
            routing_b: 2,
            correlation: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(a.response(&unrelated).unwrap().is_none());
    a.response(&query(100, b"300", 1)).unwrap();
    b.response(&query(110, b"310", 2)).unwrap();
    a.commit_after_write().unwrap();
    b.commit_after_write().unwrap();
    assert_eq!(a.association(), Some((100, 300)));
    assert_eq!(b.association(), Some((110, 310)));
}
