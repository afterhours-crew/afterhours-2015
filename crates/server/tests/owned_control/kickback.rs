// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Kickback screenshot allocation (`2053/8 addPendingScreenshot`) and commit
//! (`2053/9 commitScreenshot`) on the owned control session, driven by the
//! request bodies the client sent after the first two race finishes of the
//! official E1026 Amy capture (`control-000-control_send.bin`, correlations
//! 199/200 and 229/230). The official replies were
//! `{ID 249017865, RECN "1761118161"}` / `{SCC 1, SCCM 70}` and
//! `{ID 249017895, RECN "1761118161"}` / `{SCC 1, SCCM 70}`.
use super::*;
use nfs_heat2::Value;
use nfs_protocol::users::*;
use nfs_services::kickback::State;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
/// Race 1: AUTO 1, DESC "2:4:6:0:ID_CAR_NISSAN_SKYLINEGTRNISMOZTUNE_2005_SHORT_NAME",
/// NAME 1761118161, ST 1, X 159, Y 102 (95 bytes).
const ADD_RACE_1: &str = "875d2f0001925ce3013b323a343a363a303a49445f4341525f4e495353414e5f534b594c494e454754524e49534d4f5a54554e455f323030355f53484f52545f4e414d4500ba1b6500919fc48f0dcf40000001e00000009f02e4000000a601";
/// Race 2: same but X 55 (94 bytes).
const ADD_RACE_2: &str = "875d2f0001925ce3013b323a343a363a303a49445f4341525f4e495353414e5f534b594c494e454754524e49534d4f5a54554e455f323030355f53484f52545f4e414d4500ba1b6500919fc48f0dcf40000001e000000037e4000000a601";
/// Official `2053/9` reply body in all seven captured commits: SCC 1, SCCM 70.
const COMMIT_REPLY: &str = "ce38c00001ce38ed008601";
const ID: [u8; 3] = [0xa6, 0x40, 0x00];
const RECN: [u8; 3] = [0xca, 0x58, 0xee];

fn open_services(session: &mut ControlSession<'_>) {
    let mut network = UpdateNetworkInfoRequest {
        network_info: Some(NetworkInfo {
            address: Some(NetworkAddress::IpPair(IpPairAddress {
                external_address: Some(IpAddress {
                    ip: Some(0x7f00_0001),
                    machine_id: Some(1),
                    port: Some(32123),
                    ..Default::default()
                }),
                internal_address: Some(IpAddress {
                    ip: Some(0x7f00_0001),
                    machine_id: Some(1),
                    port: Some(32123),
                    ..Default::default()
                }),
                machine_id: Some(1),
                ..Default::default()
            })),
            ping_site_latency_by_alias: Some(PingSiteLatencyMap(vec![(b"local", 0)])),
            qos_data: Some(NetworkQosData {
                bandwidth_error_code: Some(0),
                downstream_bits_per_second: Some(0),
                nat_error_code: Some(0),
                nat_type: Some(5),
                upstream_bits_per_second: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        }),
        opts: Some(1),
        ..Default::default()
    };
    reply(
        session,
        &frame(30722, 20, &network.encode(control_body_limits()).unwrap()),
    );
    session.committed().unwrap();
    reply(
        session,
        &frame(
            30722,
            8,
            &UpdateHardwareFlagsRequest {
                hardware_flags: Some(0),
                ..Default::default()
            }
            .encode(control_body_limits())
            .unwrap(),
        ),
    );
    session.committed().unwrap();
    network.opts = Some(4);
    reply(
        session,
        &frame(30722, 20, &network.encode(control_body_limits()).unwrap()),
    );
    session.committed().unwrap();
}

/// Generic field view: (tag, value) pairs of one reply body.
fn fields(body: &[u8]) -> Vec<([u8; 3], Value<'_>)> {
    nfs_heat2::decode(body, nfs_services::kickback::body_limits())
        .unwrap()
        .fields()
        .map(|f| {
            let f = f.unwrap();
            (f.tag(), f.item().value())
        })
        .collect()
}
fn decoded(wire: &[u8]) -> nfs_fire2::Frame<'_> {
    let d = nfs_fire2::decode(wire, nfs_server::frame_limits())
        .unwrap()
        .unwrap();
    assert_eq!(d.consumed, wire.len());
    d.frame
}
fn commit_body(id: i64) -> Vec<u8> {
    let mut w = nfs_heat2::Encoder::new(nfs_services::kickback::body_limits());
    w.integer(ID, id).unwrap();
    w.finish().unwrap()
}
fn opened_session<'p>(p: &'p SessionProfiles, state: &'p State) -> ControlSession<'p> {
    let account = nfs_storage::AccountId::from_owned_config([6; 16]).unwrap();
    let mut session = ControlSession::new(p).with_kickback(state, account);
    bootstrap(&mut session);
    reply(&mut session, &login());
    session.committed().unwrap();
    reply(&mut session, &postauth());
    session.committed().unwrap();
    open_services(&mut session);
    assert_eq!(session.phase(), PhaseName::Startup);
    session
}

#[test]
fn kickback_screenshot_is_allocated_then_committed_and_the_session_stays_open() {
    let p = profiles(6);
    let account = nfs_storage::AccountId::from_owned_config([6; 16]).unwrap();
    // The official account answered 2053/2 with SSC 1 / SCCM 70 in the same capture.
    let state = State::new(account, 1, 70, None).unwrap();
    let mut session = opened_session(&p, &state);
    let mut ids = Vec::new();
    for (race, body) in [(1, ADD_RACE_1), (2, ADD_RACE_2)] {
        let request = frame(2053, 8, &hex(body));
        let replies = reply(&mut session, &request);
        assert_eq!(
            replies.len(),
            1,
            "race {race}: one addPendingScreenshot reply"
        );
        let f = decoded(&replies[0]);
        assert_eq!(
            (
                f.fields.routing_a,
                f.fields.routing_b,
                f.fields.category,
                f.fields.correlation
            ),
            (2053, 8, 1, 2053 * 100 + 8)
        );
        assert!(f.metadata.is_empty());
        let fields = fields(f.body);
        assert_eq!(fields.len(), 2, "race {race}: exactly ID and RECN");
        let Value::Integer(id) = fields[0].1 else {
            panic!("race {race}: ID is an integer")
        };
        assert_eq!(fields[0].0, ID);
        assert!(id > 0, "race {race}: allocated id");
        assert_eq!(
            (fields[1].0, fields[1].1),
            (RECN, Value::String(b"1761118161")),
            "race {race}: RECN is the request NAME as decimal text"
        );
        session.committed().unwrap();
        assert_eq!(session.phase(), PhaseName::Startup, "connection stays open");
        ids.push(id);

        let commit = frame(2053, 9, &commit_body(id));
        let replies = reply(&mut session, &commit);
        assert_eq!(replies.len(), 1, "race {race}: one commitScreenshot reply");
        let f = decoded(&replies[0]);
        assert_eq!(
            (f.fields.routing_b, f.fields.category, f.fields.correlation),
            (9, 1, 2053 * 100 + 9)
        );
        assert_eq!(f.body, hex(COMMIT_REPLY), "race {race}: SCC 1 / SCCM 70");
        session.committed().unwrap();
        assert_eq!(session.phase(), PhaseName::Startup);
    }
    assert!(ids[1] > ids[0], "later screenshot gets a later id");

    // A committed or never-allocated id is refused with the client's own
    // KICKBACK_ERR_NOT_FOUND code, and the connection still stays open.
    for id in [ids[0], ids[1] + 1] {
        let replies = reply(&mut session, &frame(2053, 9, &commit_body(id)));
        let f = decoded(&replies[0]);
        assert_eq!((f.fields.routing_b, f.fields.category), (9, 3));
        assert!(f.body.is_empty());
        let m =
            nfs_protocol::metadata::Fire2Metadata::decode(f.metadata, Default::default()).unwrap();
        assert_eq!(
            m.error_code,
            Some(nfs_protocol::metadata::KICKBACK_ERR_NOT_FOUND)
        );
        session.committed().unwrap();
        assert_eq!(session.phase(), PhaseName::Startup);
    }
}

#[test]
fn kickback_screenshot_routes_stay_unsupported_without_kickback_state() {
    let p = profiles(7);
    let mut session = ControlSession::new(&p);
    bootstrap(&mut session);
    reply(&mut session, &login());
    session.committed().unwrap();
    reply(&mut session, &postauth());
    session.committed().unwrap();
    open_services(&mut session);
    assert_eq!(
        session.on_frame(&frame(2053, 8, &hex(ADD_RACE_1)), 100, 100_000_000),
        Ok(Outcome::Unsupported)
    );
}
