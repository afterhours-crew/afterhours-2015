// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_protocol::autolog::*;
use nfs_protocol::users::*;
use nfs_services::speedwall::{Current, Sprint, State, body_limits, walls_limits};
use nfs_storage::{Batch, InventoryRepository, MemoryRepository, Timestamp};

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

#[test]
fn both_speedwall_routes_use_current_committed_results_in_an_owned_session() {
    let p = profiles(5);
    let account = nfs_storage::AccountId::from_owned_config([5; 16]).unwrap();
    let repo = MemoryRepository::default();
    repo.open(account).unwrap();
    let baseline = State::new(account, vec![]).unwrap();
    let mut session = ControlSession::new(&p).with_speedwall(&baseline, account);
    let walls = frame(
        2050,
        20,
        &InGameSpeedWallsRequest {
            blaze_id: Some(p.persona),
            speed_wall_ids: Some(SpeedWallIds(vec![7])),
            usge: Some(0),
            uspg: Some(0),
            ..Default::default()
        }
        .encode(walls_limits())
        .unwrap(),
    );
    let brief = frame(
        2050,
        79,
        &SpeedWallBriefInfoRequest {
            blaze_id: Some(p.persona),
            speed_wall_id: Some(7),
            speed_wall_stat_type: Some(0),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap(),
    );
    assert!(!session.needs_speedwall_view(&walls));
    bootstrap(&mut session);
    reply(&mut session, &login());
    session.committed().unwrap();
    reply(&mut session, &postauth());
    session.committed().unwrap();
    open_services(&mut session);
    for request in [&walls, &brief] {
        assert!(session.needs_speedwall_view(request));
    }
    for (id, time_ms, rep, car) in [(1, 90_000, 12_000, "CAR_A"), (2, 80_000, 10_000, "CAR_B")] {
        let snapshot = repo.snapshot(account).unwrap();
        let result = Sprint {
            time_ms,
            rep,
            vehicle: car.into(),
        };
        let batch = Batch {
            id,
            expected_generation: snapshot.generation,
            ops: vec![result.prepare(&snapshot, 7).unwrap()],
        };
        repo.apply(account, &batch, Timestamp(id)).unwrap();
        assert!(
            repo.apply(account, &batch, Timestamp(id + 1))
                .unwrap()
                .replayed
        );
        session.speedwall_view(
            Current::from_snapshot(account, &repo.snapshot(account).unwrap()).unwrap(),
        );
        for request in [&walls, &brief] {
            let replies = reply(&mut session, request);
            assert!(
                !session.needs_speedwall_view(request),
                "pending replies keep the session ordered"
            );
            let decoded = nfs_fire2::decode(&replies[0], nfs_server::frame_limits())
                .unwrap()
                .unwrap();
            let rows = if decoded.frame.fields.routing_b == 20 {
                let response =
                    InGameSpeedWallResponse::decode(decoded.frame.body, walls_limits()).unwrap();
                response
                    .speed_walls
                    .unwrap()
                    .0
                    .into_iter()
                    .flat_map(|(_, walls)| walls.into_iter().flat_map(|w| w.rows.unwrap().0))
                    .collect::<Vec<_>>()
            } else {
                vec![
                    SpeedWallBriefInfoResponse::decode(decoded.frame.body, body_limits())
                        .unwrap()
                        .speed_wall
                        .unwrap(),
                ]
            };
            assert_eq!(rows.len(), if request == &walls { 2 } else { 1 });
            for row in rows {
                assert_eq!(row.blaze_user.unwrap().blaze_id, Some(p.persona));
                assert_eq!(
                    row.stats_int.unwrap().0,
                    vec![(b"Attempts".as_slice(), id as i32)]
                );
                assert_eq!(
                    row.stats_flt.unwrap().0,
                    vec![
                        (b"REP".as_slice(), 12_000f32.to_bits()),
                        (b"Time".as_slice(), (time_ms as f32).to_bits()),
                    ]
                );
                assert_eq!(
                    row.stats_str.unwrap().0,
                    vec![
                        (b"SecondaryVehicle".as_slice(), car.as_bytes()),
                        (b"Vehicle".as_slice(), b"CAR_A".as_slice()),
                    ]
                );
            }
            session.committed().unwrap();
            assert!(session.needs_speedwall_view(request));
        }
    }
}
