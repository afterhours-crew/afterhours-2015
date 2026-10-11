// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::{Blob, users::*};
use nfs_services::user_session::{
    body_limits, frame_limits,
    users_followup::{Followup, State},
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

#[test]
fn first_update_orders_ack_then_notification_and_preserves_local_baseline() {
    let mut h = handler();
    assert_eq!(h.state(), State::Network);
    assert!(!h.complete());
    let out = h.response(&request(0x123456)).unwrap().unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(h.state(), State::Hardware);
    assert!(!h.complete());
    assert_eq!(
        out[0],
        [
            0, 0, 0, 0, 0, 0, 0x78, 2, 0, 20, 0x12, 0x34, 0x56, 0x20, 0, 0
        ]
    );
    let notify = decode(&out[1]);
    assert_eq!(
        notify.fields,
        Fields {
            routing_a: 30722,
            routing_b: 1,
            category: 2,
            ..Default::default()
        }
    );
    assert!(notify.metadata.is_empty());
    let update = UserSessionExtendedDataUpdate::decode(notify.body, body_limits()).unwrap();
    assert_eq!(update.subscribed, Some(true));
    assert_eq!(update.user_id, Some(100));
    let data = update.extended_data.unwrap();
    let Some(NetworkAddress::IpPair(pair)) = data.address else {
        panic!("missing address");
    };
    let ext = pair.external_address.unwrap();
    let int = pair.internal_address.unwrap();
    assert_eq!(ext.ip, Some(0xc0000201));
    assert_eq!(ext.machine_id, Some(999));
    assert_eq!(ext.port, Some(32123));
    assert_eq!(int.ip, Some(0x7f000001));
    assert_eq!(int.machine_id, Some(998));
    assert_eq!(int.port, Some(32124));
    assert_eq!(pair.machine_id, Some(997));
    assert!(data.latency_list.is_none());
    assert_eq!(data.qos_data.as_ref().unwrap().nat_type, Some(0));
    for s in [
        data.best_ping_site_alias,
        data.country,
        data.isp,
        data.time_zone,
    ] {
        assert_eq!(s, Some(b"".as_slice()));
    }
    assert_eq!(
        data.data_map.unwrap().0,
        [
            (1, 0),
            (0x70001, 0),
            (0x70002, 0),
            (0xe0001, 0),
            (0xe0002, 0),
            (0x78020001, 0)
        ]
    );
    assert_eq!(data.blaze_object_id_list.unwrap().0, [ObjectId(3, 7, 200)]);
    assert_eq!(data.hardware_flags, Some(0));
    assert_eq!(data.user_info_attribute, Some(0));
    assert_eq!(data.client_data, Some(AbsentClientData));
    let hw = h.response(&hardware(0xfedcba, 0)).unwrap().unwrap();
    assert_eq!(hw.len(), 1);
    assert_eq!(
        hw[0],
        [
            0, 0, 0, 0, 0, 0, 0x78, 2, 0, 8, 0xfe, 0xdc, 0xba, 0x20, 0, 0
        ]
    );
    assert!(h.complete());
    assert_eq!(h.state(), State::ObserveOnly);
    assert!(h.response(&request(9)).unwrap().is_none());
}

#[test]
fn observed_bandwidth_error_updates_only_address_then_accepts_hardware() {
    let mut n = network(0xc0000201);
    let info = n.network_info.as_mut().unwrap();
    info.qos_data.as_mut().unwrap().bandwidth_error_code = Some(2_693_005_310);
    let Some(NetworkAddress::IpPair(pair)) = info.address.as_mut() else {
        panic!("synthetic address");
    };
    pair.machine_id = Some(0);
    pair.external_address.as_mut().unwrap().machine_id = Some(0);
    pair.internal_address.as_mut().unwrap().machine_id = Some(0);
    let req = wire(20, 0, 5, &n.encode(body_limits()).unwrap());
    let mut h = Followup::new(&baseline_wire(100, 8192)).unwrap();
    let out = h.response(&req).unwrap().unwrap();
    assert_eq!(out.iter().map(Vec::len).collect::<Vec<_>>(), [16, 210]);
    assert_eq!(decode(&out[0]).fields.correlation, 5);
    assert_eq!(decode(&out[1]).fields.correlation, 0);
    assert_eq!(h.state(), State::Hardware);
    let update =
        UserSessionExtendedDataUpdate::decode(decode(&out[1]).body, body_limits()).unwrap();
    assert_eq!(update.user_id, Some(100));
    let data = update.extended_data.unwrap();
    let Some(NetworkAddress::IpPair(pair)) = data.address else {
        panic!("propagated address");
    };
    assert_eq!(pair.external_address.unwrap().ip, Some(0xc0000201));
    assert_eq!(pair.internal_address.unwrap().port, Some(32124));
    let q = data.qos_data.unwrap();
    assert_eq!(q.bandwidth_error_code, Some(0));
    assert_eq!(q.downstream_bits_per_second, Some(0));
    assert_eq!(q.nat_error_code, Some(0));
    assert_eq!(q.nat_type, Some(0));
    assert_eq!(q.upstream_bits_per_second, Some(0));
    assert!(data.latency_list.is_none());
    assert_eq!(h.response(&req).unwrap().unwrap(), [out[0].clone()]);
    let hw = h.response(&hardware(6, 0)).unwrap().unwrap();
    assert_eq!(hw.iter().map(Vec::len).collect::<Vec<_>>(), [16]);
    assert_eq!(decode(&hw[0]).fields.correlation, 6);
    assert!(h.complete());
}

#[test]
fn bandwidth_error_allowlist_does_not_relax_other_shapes_or_local_baseline() {
    for code in [1, 2_693_005_309, 2_693_005_311, u32::MAX] {
        let mut n = network(0xc0000201);
        n.network_info
            .as_mut()
            .unwrap()
            .qos_data
            .as_mut()
            .unwrap()
            .bandwidth_error_code = Some(code);
        let req = wire(20, 0, 5, &n.encode(body_limits()).unwrap());
        let mut h = handler();
        assert!(h.response(&req).is_err());
        assert_eq!(h.state(), State::ObserveOnly);
        assert!(!h.complete());
    }
    for change in 0..5 {
        let mut n = network(0xc0000201);
        let q = n.network_info.as_mut().unwrap().qos_data.as_mut().unwrap();
        q.bandwidth_error_code = Some(2_693_005_310);
        match change {
            0 => n.opts = Some(4),
            1 => q.downstream_bits_per_second = Some(1),
            2 => q.nat_error_code = Some(1),
            3 => q.nat_type = Some(0),
            4 => q.upstream_bits_per_second = Some(1),
            _ => unreachable!(),
        }
        assert!(
            handler()
                .response(&wire(20, 0, 5, &n.encode(body_limits()).unwrap()))
                .is_err()
        );
    }
    let mut local = baseline(100, 200);
    local
        .extended_data
        .as_mut()
        .unwrap()
        .qos_data
        .as_mut()
        .unwrap()
        .bandwidth_error_code = Some(2_693_005_310);
    assert!(Followup::new(&wire(2, 2, 0, &local.encode(body_limits()).unwrap())).is_err());
}

#[test]
fn exact_retries_only_repeat_stable_acknowledgement_and_never_effects() {
    let mut h = handler();
    let n = request(5);
    let first = h.response(&n).unwrap().unwrap();
    for _ in 0..3 {
        assert_eq!(h.response(&n).unwrap().unwrap(), [first[0].clone()]);
        assert_eq!(h.state(), State::Hardware);
        assert!(!h.complete());
    }
    let hw = hardware(6, 0);
    let ack = h.response(&hw).unwrap().unwrap();
    assert_eq!(h.response(&hw).unwrap().unwrap(), ack);
    assert!(h.complete());
    assert!(h.response(&n).unwrap().is_none());
    assert!(h.response(&hw).unwrap().is_none());
    assert!(h.complete());
}

#[test]
fn unknown_out_of_order_and_changed_retries_permanently_stop_handling() {
    let valid = request(5);
    for invalid in [
        hardware(6, 0),
        wire(99, 0, 5, &[]),
        wire(20, 1, 5, &[]),
        wire(20, 0, 5, &[]),
    ] {
        let mut h = handler();
        assert!(!matches!(h.response(&invalid), Ok(Some(_))));
        assert_eq!(h.state(), State::ObserveOnly);
        assert!(!h.complete());
        assert!(h.response(&valid).unwrap().is_none());
    }
    let mut h = handler();
    h.response(&valid).unwrap();
    assert!(h.response(&request(7)).unwrap().is_none());
    assert!(!h.complete());
    assert!(h.response(&hardware(6, 0)).unwrap().is_none());
    for flags in [1, 2, u32::MAX] {
        let mut h = handler();
        h.response(&valid).unwrap();
        assert!(h.response(&hardware(6, flags)).is_err());
        assert!(!h.complete());
        assert!(h.response(&hardware(6, 0)).unwrap().is_none());
    }
}

#[test]
fn request_gates_require_complete_canonical_nested_address_and_metrics() {
    let mut invalid = Vec::new();
    let mut n = network(1);
    n.opts = Some(4);
    invalid.push(n);
    let mut n = network(1);
    n.opts = None;
    invalid.push(n);
    let mut n = network(1);
    n.network_info = None;
    invalid.push(n);
    let mut n = network(1);
    n.network_info.as_mut().unwrap().address = Some(NetworkAddress::Unset);
    invalid.push(n);
    let mut n = network(1);
    n.network_info.as_mut().unwrap().qos_data = Some(qos(0));
    invalid.push(n);
    let mut n = network(1);
    n.network_info
        .as_mut()
        .unwrap()
        .qos_data
        .as_mut()
        .unwrap()
        .bandwidth_error_code = None;
    invalid.push(n);
    let mut n = network(1);
    n.network_info
        .as_mut()
        .unwrap()
        .ping_site_latency_by_alias
        .as_mut()
        .unwrap()
        .0
        .pop();
    invalid.push(n);
    let mut n = network(1);
    n.network_info
        .as_mut()
        .unwrap()
        .ping_site_latency_by_alias
        .as_mut()
        .unwrap()
        .0[0]
        .0 = b"";
    invalid.push(n);
    let mut n = network(1);
    if let Some(NetworkAddress::IpPair(p)) = &mut n.network_info.as_mut().unwrap().address {
        p.external_address.as_mut().unwrap().machine_id = None;
    }
    invalid.push(n);
    let mut n = network(1);
    if let Some(NetworkAddress::IpPair(p)) = &mut n.network_info.as_mut().unwrap().address {
        p.machine_id = None;
    }
    invalid.push(n);
    for n in invalid {
        let mut h = handler();
        let req = wire(20, 0, 5, &n.encode(body_limits()).unwrap());
        assert!(h.response(&req).is_err());
        assert_eq!(h.state(), State::ObserveOnly);
    }
    let raw = request(5);
    let f = decode(&raw);
    let mut b = f.body.to_vec();
    b.extend_from_slice(&[0xff, 0xff, 0xff, 0, 1]);
    assert!(handler().response(&wire(20, 0, 5, &b)).is_err());
    let mut h = handler();
    h.response(&raw).unwrap();
    assert!(
        h.response(&wire(8, 0, 6, &[0xa3, 0x79, 0xa7, 0, 0x80, 0]))
            .is_err()
    ); // nonminimal HWFG0
}

#[test]
fn malformed_partial_concatenated_metadata_headers_and_limits_get_no_success() {
    let req = request(5);
    for end in 0..req.len() {
        let mut h = handler();
        assert!(h.response(&req[..end]).is_err());
        assert!(!h.complete());
        assert!(h.response(&req).unwrap().is_none());
    }
    let mut too_big = req.clone();
    too_big[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    let f = decode(&req);
    let meta = nfs_fire2::encode(
        Frame {
            fields: f.fields,
            metadata: &[1],
            body: f.body,
        },
        nfs_fire2::Limits::default(),
    )
    .unwrap();
    for r in [too_big, meta, [req.clone(), req.clone()].concat()] {
        assert!(handler().response(&r).is_err());
    }
    for (offset, value) in [(7, 1), (9, 21), (13, 1), (13, 0x40), (14, 1), (15, 1)] {
        let mut r = req.clone();
        r[offset] = value;
        assert!(!matches!(handler().response(&r), Ok(Some(_))));
    }
}

#[test]
fn baseline_rejects_nonlocal_policy_missing_fields_unknowns_and_wrong_headers() {
    let source = baseline_wire(100, 200);
    for end in 0..source.len() {
        assert!(Followup::new(&source[..end]).is_err());
    }
    assert!(Followup::new(&[source.clone(), source.clone()].concat()).is_err());
    for (offset, value) in [(9, 1), (10, 1), (13, 0), (14, 1)] {
        let mut b = source.clone();
        b[offset] = value;
        assert!(Followup::new(&b).is_err());
    }
    let mut cases = Vec::new();
    let mut b = baseline(100, 200);
    b.extended_data = None;
    cases.push(b);
    let mut b = baseline(100, 200);
    b.user_info = None;
    cases.push(b);
    for name in [
        b"".as_slice(),
        b"bad\0name",
        b"bad\nname",
        b"\xff",
        &[b'a'; 65],
    ] {
        let mut b = baseline(100, 200);
        b.user_info.as_mut().unwrap().name = Some(name);
        cases.push(b);
    }
    let mut b = baseline(100, 200);
    b.user_info.as_mut().unwrap().external_id = Some(999);
    cases.push(b);
    let mut b = baseline(100, 200);
    b.extended_data
        .as_mut()
        .unwrap()
        .data_map
        .as_mut()
        .unwrap()
        .0[0]
        .1 = 1;
    cases.push(b);
    let mut b = baseline(100, 200);
    b.extended_data.as_mut().unwrap().qos_data = Some(qos(5));
    cases.push(b);
    let mut b = baseline(100, 200);
    b.extended_data.as_mut().unwrap().latency_list = Some(LatencyList(vec![]));
    cases.push(b);
    let mut b = baseline(100, 200);
    b.extended_data
        .as_mut()
        .unwrap()
        .blaze_object_id_list
        .as_mut()
        .unwrap()
        .0
        .clear();
    cases.push(b);
    let mut b = baseline(100, 200);
    b.extended_data.as_mut().unwrap().client_data = None;
    cases.push(b);
    for b in cases {
        assert!(Followup::new(&wire(2, 2, 0, &b.encode(body_limits()).unwrap())).is_err());
    }
    let f = decode(&source);
    let mut b = f.body.to_vec();
    b.extend_from_slice(&[0xff, 0xff, 0xff, 0, 1]);
    assert!(Followup::new(&wire(2, 2, 0, &b)).is_err());
}

#[test]
fn concurrent_connections_keep_addresses_personas_baselines_and_progress_isolated() {
    let result = std::thread::scope(|s| {
        [(100, 200, 0xc0000201), (300, 400, 0xc0000202)]
            .map(|(person, group, ip)| {
                s.spawn(move || {
                    let mut h = Followup::new(&baseline_wire(person, group)).unwrap();
                    let r = wire(20, 0, 123, &network(ip).encode(body_limits()).unwrap());
                    let out = h.response(&r).unwrap().unwrap();
                    assert!(!h.complete());
                    if person == 100 {
                        h.response(&hardware(456, 0)).unwrap();
                        assert!(h.complete());
                    }
                    (out, person, group, ip)
                })
            })
            .map(|h| h.join().unwrap())
    });
    for (out, person, group, ip) in result {
        let u = UserSessionExtendedDataUpdate::decode(decode(&out[1]).body, body_limits()).unwrap();
        assert_eq!(u.user_id, Some(person));
        let d = u.extended_data.unwrap();
        assert_eq!(d.blaze_object_id_list.unwrap().0, [ObjectId(3, 7, group)]);
        let Some(NetworkAddress::IpPair(p)) = d.address else {
            panic!("address")
        };
        assert_eq!(p.external_address.unwrap().ip, Some(ip));
    }
}

#[test]
fn dropped_source_buffer_cannot_change_owned_baseline() {
    let mut source = baseline_wire(100, 200);
    let mut h = Followup::new(&source).unwrap();
    source.fill(0);
    drop(source);
    let out = h.response(&request(5)).unwrap().unwrap();
    let update =
        UserSessionExtendedDataUpdate::decode(decode(&out[1]).body, body_limits()).unwrap();
    assert_eq!(update.user_id, Some(100));
    assert_eq!(
        update
            .extended_data
            .unwrap()
            .blaze_object_id_list
            .unwrap()
            .0,
        [ObjectId(3, 7, 200)]
    );
}

#[test]
fn nested_unknown_nonminimal_encoding_and_alias_limits_stop_before_ack() {
    let raw = request(5);
    let body = decode(&raw).body;
    let doc = nfs_heat2::decode(body, body_limits()).unwrap();
    let info = doc.fields().next().unwrap().unwrap();
    let mut changed = info.as_bytes().to_vec();
    assert_eq!(changed.pop(), Some(0));
    changed.extend_from_slice(&[0xff, 0xff, 0xff, 0, 1, 0]);
    changed.extend_from_slice(&body[info.as_bytes().len()..]);
    assert!(handler().response(&wire(20, 0, 5, &changed)).is_err());
    let mut changed = body.to_vec();
    assert_eq!(changed.pop(), Some(1));
    changed.extend_from_slice(&[0x81, 0]);
    assert!(handler().response(&wire(20, 0, 5, &changed)).is_err());
    for alias in [
        vec![b'a'; 65],
        vec![b'a'; 1025],
        vec![0xff],
        b"with space".to_vec(),
    ] {
        let mut n = network(1);
        n.network_info
            .as_mut()
            .unwrap()
            .ping_site_latency_by_alias
            .as_mut()
            .unwrap()
            .0[0]
            .0 = &alias;
        let raw = n.encode(nfs_heat2::Limits::default()).unwrap();
        assert!(handler().response(&wire(20, 0, 5, &raw)).is_err());
    }
}

#[test]
fn baseline_metadata_nested_unknown_and_noncanonical_state_are_rejected() {
    let original = baseline_wire(100, 200);
    let f = decode(&original);
    let with_meta = nfs_fire2::encode(
        Frame {
            fields: f.fields,
            metadata: &[1],
            body: f.body,
        },
        nfs_fire2::Limits::default(),
    )
    .unwrap();
    assert!(Followup::new(&with_meta).is_err());
    let doc = nfs_heat2::decode(f.body, body_limits()).unwrap();
    let data = doc.fields().next().unwrap().unwrap();
    let mut changed = data.as_bytes().to_vec();
    assert_eq!(changed.pop(), Some(0));
    changed.extend_from_slice(&[0xff, 0xff, 0xff, 0, 1, 0]);
    changed.extend_from_slice(&f.body[data.as_bytes().len()..]);
    assert!(Followup::new(&wire(2, 2, 0, &changed)).is_err());
    let mut changed = f.body.to_vec();
    let hw = changed
        .windows(5)
        .position(|w| w == [0xa3, 0x79, 0xa7, 0, 0])
        .unwrap();
    changed.splice(hw + 4..hw + 5, [0x80, 0]);
    assert!(Followup::new(&wire(2, 2, 0, &changed)).is_err());
}

#[test]
fn actual_fire2_metadata_is_refused_at_baseline_network_and_hardware_stages() {
    fn add_metadata(bytes: &[u8]) -> Vec<u8> {
        let source = decode(bytes);
        let metadata = [0x8e, 0xed, 0x38, 0, 0, 0x97, 0x2c, 0xa3, 0, 0];
        let result = nfs_fire2::encode(
            Frame {
                fields: source.fields,
                metadata: &metadata,
                body: source.body,
            },
            nfs_fire2::Limits::default(),
        )
        .unwrap();
        let decoded = nfs_fire2::decode(&result, nfs_fire2::Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(decoded.frame.metadata, metadata);
        assert_eq!(decoded.frame.body, source.body);
        result
    }
    assert!(Followup::new(&add_metadata(&baseline_wire(100, 200))).is_err());
    let mut network_handler = handler();
    assert!(
        network_handler
            .response(&add_metadata(&request(5)))
            .is_err()
    );
    assert_eq!(network_handler.state(), State::ObserveOnly);
    assert!(!network_handler.complete());
    assert!(network_handler.response(&request(5)).unwrap().is_none());
    let mut hardware_handler = handler();
    hardware_handler.response(&request(5)).unwrap().unwrap();
    assert!(
        hardware_handler
            .response(&add_metadata(&hardware(6, 0)))
            .is_err()
    );
    assert_eq!(hardware_handler.state(), State::ObserveOnly);
    assert!(!hardware_handler.complete());
    assert!(
        hardware_handler
            .response(&hardware(6, 0))
            .unwrap()
            .is_none()
    );
}
