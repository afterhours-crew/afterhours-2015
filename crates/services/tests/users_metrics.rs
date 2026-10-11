// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::{Blob, users::*};
use nfs_services::user_session::{
    body_limits, frame_limits,
    users_metrics::{Followup, State},
};

fn wire(command: u16, category: u8, correlation: u32, body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 30722,
                routing_b: command,
                category,
                correlation,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .unwrap()
}
fn decode(wire: &[u8]) -> Frame<'_> {
    let d = nfs_fire2::decode(wire, frame_limits()).unwrap().unwrap();
    assert_eq!(d.consumed, wire.len());
    d.frame
}
fn qos(nat: i64) -> NetworkQosData<'static> {
    NetworkQosData {
        bandwidth_error_code: Some(0),
        downstream_bits_per_second: Some(0),
        nat_error_code: Some(0),
        nat_type: Some(nat),
        upstream_bits_per_second: Some(0),
        ..Default::default()
    }
}
fn baseline(persona: i64, connection: i64) -> NotifyUserAddedInitial<'static> {
    NotifyUserAddedInitial {
        extended_data: Some(UserSessionExtendedDataInitial {
            address: Some(UnsetNetworkAddress),
            best_ping_site_alias: Some(b""),
            country: Some(b""),
            client_data: Some(AbsentClientData),
            data_map: Some(ExtendedDataMap(vec![
                (1, 0),
                (0x70001, 0),
                (0x70002, 0),
                (0xe0001, 0),
                (0xe0002, 0),
                (0x78020001, 0),
            ])),
            hardware_flags: Some(0),
            isp: Some(b""),
            latency_list: None,
            qos_data: Some(qos(0)),
            time_zone: Some(b""),
            user_info_attribute: Some(0),
            blaze_object_id_list: Some(ObjectIdList(vec![ObjectId(3, 7, connection)])),
            ..Default::default()
        }),
        user_info: Some(UserIdentification {
            account_id: Some(persona + 1),
            account_locale: Some(64),
            external_blob: Some(Blob(&[])),
            external_id: Some((persona + 1) as u64),
            blaze_id: Some(persona),
            name: Some(b"Offline Driver"),
            persona_namespace: Some(b"synthetic"),
            origin_persona_id: Some(persona as u64),
            pid_id: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn baseline_wire(persona: i64, connection: i64) -> Vec<u8> {
    wire(
        2,
        2,
        0,
        &baseline(persona, connection).encode(body_limits()).unwrap(),
    )
}
fn handler() -> Followup {
    Followup::new(&baseline_wire(100, 200)).unwrap()
}
fn address(ip: u32) -> NetworkAddress<'static> {
    NetworkAddress::IpPair(IpPairAddress {
        external_address: Some(IpAddress {
            ip: Some(ip),
            machine_id: Some(999),
            port: Some(32123),
            ..Default::default()
        }),
        internal_address: Some(IpAddress {
            ip: Some(0x7f000001),
            machine_id: Some(998),
            port: Some(32124),
            ..Default::default()
        }),
        machine_id: Some(997),
        ..Default::default()
    })
}
fn network(ip: u32) -> UpdateNetworkInfoRequest<'static> {
    UpdateNetworkInfoRequest {
        network_info: Some(NetworkInfo {
            address: Some(address(ip)),
            ping_site_latency_by_alias: Some(PingSiteLatencyMap(vec![
                (b"a", -1),
                (b"b", 17),
                (b"c", i32::MIN),
                (b"d", i32::MAX),
            ])),
            qos_data: Some(qos(5)),
            ..Default::default()
        }),
        opts: Some(1),
        ..Default::default()
    }
}
fn request(correlation: u32) -> Vec<u8> {
    wire(
        20,
        0,
        correlation,
        &network(0xc0000201).encode(body_limits()).unwrap(),
    )
}
fn hardware(correlation: u32, flags: u32) -> Vec<u8> {
    wire(
        8,
        0,
        correlation,
        &UpdateHardwareFlagsRequest {
            hardware_flags: Some(flags),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap(),
    )
}

fn metrics(values: [i32; 4]) -> UpdateNetworkInfoRequest<'static> {
    let mut n = network(0xc0000201);
    n.opts = Some(4);
    let info = n.network_info.as_mut().unwrap();
    info.qos_data = Some(qos(6));
    for (entry, value) in info
        .ping_site_latency_by_alias
        .as_mut()
        .unwrap()
        .0
        .iter_mut()
        .zip(values)
    {
        entry.1 = value;
    }
    n
}
fn metrics_wire(n: &UpdateNetworkInfoRequest<'_>) -> Vec<u8> {
    wire(20, 0, 7, &n.encode(body_limits()).unwrap())
}
fn ready() -> (Followup, Vec<u8>) {
    let mut h = handler();
    let before = h.response(&request(5)).unwrap().unwrap().remove(1);
    h.response(&hardware(6, 0)).unwrap().unwrap();
    assert_eq!(h.state(), State::Metrics);
    assert!(!h.complete());
    (h, before)
}
fn fields(body: &[u8]) -> Vec<([u8; 3], Vec<u8>)> {
    nfs_heat2::decode(body, body_limits())
        .unwrap()
        .fields()
        .map(|field| {
            let field = field.unwrap();
            (field.tag(), field.as_bytes().to_vec())
        })
        .collect()
}

#[test]
fn metrics_orders_independent_ack_and_fields_then_preserves_all_other_state() {
    let (mut h, before) = ready();
    let request = metrics_wire(&metrics([4; 4]));
    let output = h.response(&request).unwrap().unwrap();
    assert_eq!(output.len(), 2);
    assert_eq!(
        output[0],
        [0, 0, 0, 0, 0, 0, 0x78, 2, 0, 20, 0, 0, 7, 0x20, 0, 0]
    );
    let notification = decode(&output[1]);
    assert_eq!(
        notification.fields,
        Fields {
            routing_a: 30722,
            routing_b: 1,
            category: 2,
            ..Default::default()
        }
    );
    let old = UserSessionExtendedDataUpdate::decode(decode(&before).body, body_limits()).unwrap();
    let new = UserSessionExtendedDataUpdate::decode(notification.body, body_limits()).unwrap();
    assert_eq!(new.user_id, Some(100));
    assert_eq!(new.subscribed, Some(true));
    let old = fields(&old.extended_data.unwrap().encode(body_limits()).unwrap());
    let new = fields(&new.extended_data.unwrap().encode(body_limits()).unwrap());
    assert_eq!(
        new.iter()
            .find(|(t, _)| *t == [0x8b, 0x0c, 0xc0])
            .unwrap()
            .1,
        [0x8b, 0x0c, 0xc0, 1, 2, b'a', 0]
    );
    assert_eq!(
        new.iter()
            .find(|(t, _)| *t == [0xc3, 0x3b, 0x2d])
            .unwrap()
            .1,
        [0xc3, 0x3b, 0x2d, 4, 0, 4, 4, 4, 4, 4]
    );
    assert_eq!(
        new.iter()
            .find(|(t, _)| *t == [0xc6, 0x48, 0x74])
            .unwrap()
            .1,
        [
            0xc6, 0x48, 0x74, 3, 0x8b, 0x7a, 0x32, 0, 0, 0x92, 0x2c, 0x33, 0, 0, 0xba, 0x1a, 0x32,
            0, 0, 0xba, 0x1d, 0x34, 0, 6, 0xd6, 0x2c, 0x33, 0, 0, 0
        ]
    );
    assert_eq!(new.len(), 12);
    for (tag, bytes) in old {
        if tag != [0x8b, 0x0c, 0xc0] && tag != [0xc6, 0x48, 0x74] {
            assert_eq!(&new.iter().find(|(t, _)| *t == tag).unwrap().1, &bytes);
        }
    }
    assert_eq!(h.state(), State::ObserveOnly);
    assert!(h.complete());
}

#[test]
fn minimum_and_tie_policies_follow_the_first_request_alias_order() {
    for (latencies, expected) in [
        ([4, 4, 4, 4], b"a".as_slice()),
        ([17, 3, 3, 20], b"b"),
        ([99, 21, 4, 7], b"c"),
        ([i32::MAX, i32::MAX, 1, 0], b"d"),
    ] {
        let (mut h, _) = ready();
        let output = h
            .response(&metrics_wire(&metrics(latencies)))
            .unwrap()
            .unwrap();
        let update =
            UserSessionExtendedDataUpdate::decode(decode(&output[1]).body, body_limits()).unwrap();
        let data = update.extended_data.unwrap();
        assert_eq!(data.best_ping_site_alias, Some(expected));
        assert_eq!(data.latency_list.unwrap().0, latencies);
    }
    let mut first = network(0xc0000201);
    first
        .network_info
        .as_mut()
        .unwrap()
        .ping_site_latency_by_alias
        .as_mut()
        .unwrap()
        .0
        .reverse();
    let mut last = metrics([4; 4]);
    last.network_info
        .as_mut()
        .unwrap()
        .ping_site_latency_by_alias
        .as_mut()
        .unwrap()
        .0
        .reverse();
    let mut h = handler();
    h.response(&wire(20, 0, 5, &first.encode(body_limits()).unwrap()))
        .unwrap()
        .unwrap();
    h.response(&hardware(6, 0)).unwrap().unwrap();
    let output = h.response(&metrics_wire(&last)).unwrap().unwrap();
    let update =
        UserSessionExtendedDataUpdate::decode(decode(&output[1]).body, body_limits()).unwrap();
    assert_eq!(
        update.extended_data.unwrap().best_ping_site_alias,
        Some(b"d".as_slice())
    );
}

#[test]
fn opaque_qos_echo_includes_saved_error_values_and_full_native_numeric_ranges() {
    for (berr, dbps, nerr, nat, ubps) in [
        (2_692_874_238, 0, 2_692_874_238, 6, 0),
        (u32::MAX, u32::MAX, u32::MAX, i64::MIN, u32::MAX),
        (0, 1, 2, i64::MAX, 3),
    ] {
        let mut m = metrics([4; 4]);
        let q = m.network_info.as_mut().unwrap().qos_data.as_mut().unwrap();
        q.bandwidth_error_code = Some(berr);
        q.downstream_bits_per_second = Some(dbps);
        q.nat_error_code = Some(nerr);
        q.nat_type = Some(nat);
        q.upstream_bits_per_second = Some(ubps);
        let expected = q.encode(body_limits()).unwrap();
        let (mut h, _) = ready();
        let output = h.response(&metrics_wire(&m)).unwrap().unwrap();
        let update =
            UserSessionExtendedDataUpdate::decode(decode(&output[1]).body, body_limits()).unwrap();
        assert_eq!(
            update
                .extended_data
                .unwrap()
                .qos_data
                .unwrap()
                .encode(body_limits())
                .unwrap(),
            expected
        );
    }
}

#[test]
fn exact_immediate_retries_ack_only_in_all_stages_and_unknown_clears_cache() {
    let mut h = handler();
    for request in [request(5), hardware(6, 0), metrics_wire(&metrics([4; 4]))] {
        let output = h.response(&request).unwrap().unwrap();
        for _ in 0..3 {
            assert_eq!(h.response(&request).unwrap().unwrap(), [output[0].clone()]);
        }
    }
    assert!(h.complete());
    assert!(h.response(&wire(99, 0, 8, &[])).unwrap().is_none());
    assert!(
        h.response(&metrics_wire(&metrics([4; 4])))
            .unwrap()
            .is_none()
    );
    assert!(h.complete());
}

#[test]
fn unsupported_shape_missing_fields_alias_changes_and_address_changes_stop() {
    for change in 0..17 {
        let mut m = metrics([4; 4]);
        match change {
            0 => m.opts = Some(1),
            1 => m.opts = Some(2),
            2 => m.opts = Some(5),
            3 => m.opts = None,
            4 => m.network_info = None,
            _ => {
                let i = m.network_info.as_mut().unwrap();
                match change {
                    5 => i.address = Some(address(1)),
                    6 => i.address = Some(NetworkAddress::Unset),
                    7 => i.address = None,
                    8 => i.ping_site_latency_by_alias = None,
                    9 => {
                        i.ping_site_latency_by_alias.as_mut().unwrap().0.pop();
                    }
                    10 => i.ping_site_latency_by_alias.as_mut().unwrap().0[0].0 = b"other",
                    11 => i.ping_site_latency_by_alias.as_mut().unwrap().0.swap(0, 1),
                    12 => i.ping_site_latency_by_alias.as_mut().unwrap().0[0].1 = -1,
                    13 => i.qos_data = None,
                    14 => i.qos_data.as_mut().unwrap().bandwidth_error_code = None,
                    15 => i.qos_data.as_mut().unwrap().nat_type = None,
                    16 => {
                        if let Some(NetworkAddress::IpPair(p)) = &mut i.address {
                            p.external_address.as_mut().unwrap().machine_id = None;
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
        let (mut h, _) = ready();
        assert!(h.response(&metrics_wire(&m)).is_err(), "case {change}");
        assert_eq!(h.state(), State::ObserveOnly);
        assert!(!h.complete());
        assert!(
            h.response(&metrics_wire(&metrics([4; 4])))
                .unwrap()
                .is_none()
        );
    }
    for change in 0..3 {
        let mut m = metrics([4; 4]);
        let q = m.network_info.as_mut().unwrap().qos_data.as_mut().unwrap();
        match change {
            0 => q.downstream_bits_per_second = None,
            1 => q.nat_error_code = None,
            _ => q.upstream_bits_per_second = None,
        }
        assert!(ready().0.response(&metrics_wire(&m)).is_err());
    }
}

#[test]
fn unknown_nested_duplicate_and_noncanonical_fields_never_receive_success() {
    let raw = metrics_wire(&metrics([4; 4]));
    let body = decode(&raw).body;
    let info = nfs_heat2::decode(body, body_limits())
        .unwrap()
        .fields()
        .next()
        .unwrap()
        .unwrap();
    let mut nested = info.as_bytes().to_vec();
    assert_eq!(nested.pop(), Some(0));
    nested.extend_from_slice(&[0xff, 0xff, 0xff, 0, 1, 0]);
    nested.extend_from_slice(&body[info.as_bytes().len()..]);
    let mut nonminimal = body.to_vec();
    assert_eq!(nonminimal.pop(), Some(4));
    nonminimal.extend_from_slice(&[0x84, 0]);
    let mut duplicate = body.to_vec();
    duplicate.extend_from_slice(&[0xbf, 0x0d, 0x33, 0, 4]);
    let mut unknown = body.to_vec();
    unknown.extend_from_slice(&[0xff, 0xff, 0xff, 0, 1]);
    for body in [nested, nonminimal, duplicate, unknown] {
        let (mut h, _) = ready();
        assert!(h.response(&wire(20, 0, 7, &body)).is_err());
        assert!(!h.complete());
    }
}

#[test]
fn partial_concatenated_oversized_metadata_and_routing_frames_stop() {
    let raw = metrics_wire(&metrics([4; 4]));
    for end in 0..raw.len() {
        let (mut h, _) = ready();
        assert!(h.response(&raw[..end]).is_err());
        assert!(!h.complete());
    }
    let mut invalid = vec![[raw.clone(), raw.clone()].concat(), vec![0; 65537]];
    let f = decode(&raw);
    let metadata = [0x8e, 0xed, 0x38, 0, 0, 0x97, 0x2c, 0xa3, 0, 0];
    let limits = nfs_fire2::Limits::default();
    invalid.push(
        nfs_fire2::encode(
            Frame {
                fields: f.fields,
                metadata: &metadata,
                body: f.body,
            },
            limits,
        )
        .unwrap(),
    );
    for change in 0..5 {
        let mut fields = f.fields;
        match change {
            0 => fields.routing_a = 1,
            1 => fields.routing_b = 8,
            2 => fields.category = 1,
            3 => fields.slot = 1,
            _ => fields.reserved = [1, 0],
        }
        invalid.push(
            nfs_fire2::encode(
                Frame {
                    fields,
                    metadata: &[],
                    body: f.body,
                },
                frame_limits(),
            )
            .unwrap(),
        );
    }
    for invalid in invalid {
        let (mut h, _) = ready();
        assert!(!matches!(h.response(&invalid), Ok(Some(_))));
        assert_eq!(h.state(), State::ObserveOnly);
        assert!(!h.complete());
    }
}

#[test]
fn out_of_order_and_changed_retries_do_not_skip_transitions() {
    let metric = metrics_wire(&metrics([4; 4]));
    let mut h = handler();
    assert!(h.response(&metric).is_err());
    assert!(h.response(&request(5)).unwrap().is_none());
    let mut h = handler();
    h.response(&request(5)).unwrap().unwrap();
    assert!(h.response(&metric).unwrap().is_none());
    assert!(!h.complete());
    let (mut h, _) = ready();
    assert!(h.response(&hardware(9, 0)).unwrap().is_none());
    assert!(h.response(&metric).unwrap().is_none());
    assert!(!h.complete());
    let (mut h, _) = ready();
    h.response(&metric).unwrap().unwrap();
    assert!(
        h.response(&metrics_wire(&metrics([5; 4])))
            .unwrap()
            .is_none()
    );
    assert!(h.response(&metric).unwrap().is_none());
}

#[test]
fn concurrent_connections_isolate_identity_address_and_disconnect_state() {
    let outputs: Vec<_> = (0..2)
        .map(|i| {
            std::thread::spawn(move || {
                let mut h = Followup::new(&baseline_wire(100 + i, 200 + i)).unwrap();
                let first = wire(
                    20,
                    0,
                    5,
                    &network(0xc0000201 + i as u32)
                        .encode(body_limits())
                        .unwrap(),
                );
                h.response(&first).unwrap().unwrap();
                h.response(&hardware(6, 0)).unwrap().unwrap();
                let mut m = metrics([4; 4]);
                m.network_info.as_mut().unwrap().address = Some(address(0xc0000201 + i as u32));
                let output = h.response(&metrics_wire(&m)).unwrap().unwrap();
                assert!(h.complete());
                output
            })
        })
        .collect();
    for (i, handle) in outputs.into_iter().enumerate() {
        let output = handle.join().unwrap();
        let update =
            UserSessionExtendedDataUpdate::decode(decode(&output[1]).body, body_limits()).unwrap();
        assert_eq!(update.user_id, Some(100 + i as i64));
        let data = update.extended_data.unwrap();
        assert_eq!(
            data.blaze_object_id_list.unwrap().0,
            [ObjectId(3, 7, 200 + i as i64)]
        );
        let Some(NetworkAddress::IpPair(address)) = data.address else {
            panic!("missing address")
        };
        assert_eq!(
            address.external_address.unwrap().ip,
            Some(0xc0000201 + i as u32)
        );
    }
    let (old, _) = ready();
    drop(old);
    let mut fresh = handler();
    assert_eq!(fresh.state(), State::Network);
    assert!(!fresh.complete());
    assert!(fresh.response(&metrics_wire(&metrics([4; 4]))).is_err());
}

#[test]
fn first_pair_remains_exactly_the_unchanged_adapter_and_rejects_bad_baselines() {
    let baseline = baseline_wire(100, 200);
    let mut original =
        nfs_services::user_session::users_followup::Followup::new(&baseline).unwrap();
    let mut extended = Followup::new(&baseline).unwrap();
    for request in [request(5), hardware(6, 0)] {
        assert_eq!(
            extended.response(&request).unwrap(),
            original.response(&request).unwrap()
        );
    }
    assert!(original.complete());
    assert!(!extended.complete());
    for end in 0..baseline.len() {
        assert!(Followup::new(&baseline[..end]).is_err());
    }
    assert!(Followup::new(&[baseline.clone(), baseline].concat()).is_err());
}
