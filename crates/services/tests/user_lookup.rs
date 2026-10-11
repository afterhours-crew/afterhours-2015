// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::{Blob, metadata::Fire2Metadata, users::*};
use nfs_services::user_lookup::{
    self, Error, ExternalResolution, Profile, body_limits, frame_limits,
};
fn wire(command: u16, category: u8, correlation: u32, metadata: &[u8], body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 30722,
                routing_b: command,
                category,
                correlation,
                ..Default::default()
            },
            metadata,
            body,
        },
        nfs_fire2::Limits::default(),
    )
    .unwrap()
}
fn frame(w: &[u8]) -> Frame<'_> {
    let d = nfs_fire2::decode(w, frame_limits()).unwrap().unwrap();
    assert_eq!(d.consumed, w.len());
    d.frame
}
fn current(persona: i64) -> (Vec<u8>, Vec<u8>) {
    let qos = |nat| NetworkQosData {
        bandwidth_error_code: Some(0),
        downstream_bits_per_second: Some(0),
        nat_error_code: Some(0),
        nat_type: Some(nat),
        upstream_bits_per_second: Some(0),
        ..Default::default()
    };
    let initial = NotifyUserAddedInitial {
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
        extended_data: Some(UserSessionExtendedDataInitial {
            address: Some(UnsetNetworkAddress),
            best_ping_site_alias: Some(b""),
            country: Some(b""),
            client_data: Some(AbsentClientData),
            data_map: Some(ExtendedDataMap(vec![(1, 0)])),
            hardware_flags: Some(0),
            isp: Some(b""),
            qos_data: Some(qos(0)),
            time_zone: Some(b""),
            user_info_attribute: Some(0),
            blaze_object_id_list: Some(ObjectIdList(vec![ObjectId(3, 7, persona + 100)])),
            ..Default::default()
        }),
        ..Default::default()
    };
    let latest = UserSessionExtendedDataUpdate {
        subscribed: Some(true),
        user_id: Some(persona),
        extended_data: Some(UserSessionExtendedDataNetwork {
            address: Some(NetworkAddress::Unset),
            best_ping_site_alias: Some(b"local"),
            country: Some(b"XX"),
            client_data: Some(AbsentClientData),
            data_map: Some(ExtendedDataMap(vec![(1, 0)])),
            hardware_flags: Some(0),
            isp: Some(b""),
            latency_list: Some(LatencyList(vec![4, 8, 12, 16])),
            qos_data: Some(qos(6)),
            time_zone: Some(b""),
            user_info_attribute: Some(0),
            blaze_object_id_list: Some(ObjectIdList(vec![ObjectId(3, 7, persona + 100)])),
            ..Default::default()
        }),
        ..Default::default()
    };
    (
        wire(
            USER_ADDED,
            2,
            0,
            &[],
            &initial.encode(body_limits()).unwrap(),
        ),
        wire(
            USER_SESSION_EXTENDED_DATA_UPDATE,
            2,
            0,
            &[],
            &latest.encode(body_limits()).unwrap(),
        ),
    )
}
fn profile(persona: i64) -> Profile {
    let (a, b) = current(persona);
    Profile::from_current(&a, &b).unwrap()
}
fn query(id: i64, external: u64) -> UserIdentification<'static> {
    UserIdentification {
        account_id: Some(0),
        account_locale: Some(0),
        external_blob: Some(Blob(&[])),
        external_id: Some(external),
        blaze_id: Some(id),
        name: Some(b""),
        persona_namespace: Some(b""),
        origin_persona_id: Some(0),
        pid_id: Some(0),
        ..Default::default()
    }
}
fn request(id: i64, external: u64, correlation: u32) -> Vec<u8> {
    wire(
        12,
        0,
        correlation,
        &[],
        &query(id, external).encode(body_limits()).unwrap(),
    )
}
fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn hardware_update_changes_only_current_session_and_retries_are_idempotent() {
    let mut a = profile(42);
    let b = profile(43);
    let current = |p: &Profile, id| {
        p.reply(&request(id, 0, 77), ExternalResolution::Unsupported)
            .unwrap()
            .unwrap()
    };
    let before = current(&a, 42);
    let other = current(&b, 43);
    let q = wire(8, 0, 66, &[], &[0xa3, 0x79, 0xa7, 0, 1]);
    assert_eq!(a.update_hardware(&q).unwrap(), wire(8, 1, 66, &[], &[]));
    let after = current(&a, 42);
    let mut expected = UserData::decode(frame(&before).body, body_limits()).unwrap();
    expected.extended_data.as_mut().unwrap().hardware_flags = Some(1);
    assert_eq!(frame(&after).body, expected.encode(body_limits()).unwrap());
    a.update_hardware(&q).unwrap();
    assert_eq!(current(&a, 42), after);
    assert_eq!(current(&b, 43), other);
    for body in [
        vec![],
        vec![0xa3, 0x79, 0xa7, 0, 2],
        vec![0xa3, 0x79, 0xa7, 0, 1, 0xff, 0xff, 0xff, 0, 0],
    ] {
        assert!(a.update_hardware(&wire(8, 0, 67, &[], &body)).is_err());
        assert_eq!(current(&a, 42), after);
    }
    assert_eq!(current(&profile(42), 42), before); // reconnect has fresh state
    a.update_hardware(&wire(8, 0, 68, &[], &[0xa3, 0x79, 0xa7, 0, 0]))
        .unwrap();
    assert_eq!(current(&a, 42), before);
}

#[test]
fn self_lookup_uses_initial_identity_and_latest_current_data_not_initial_edat() {
    let (a, b) = current(42);
    let p = Profile::from_current(&a, &b).unwrap();
    assert_eq!(p.persona_id(), 42);
    let output = p
        .reply(&request(42, 0, 17), ExternalResolution::Unsupported)
        .unwrap()
        .unwrap();
    let f = frame(&output);
    assert_eq!(
        (
            f.fields.routing_a,
            f.fields.routing_b,
            f.fields.category,
            f.fields.correlation
        ),
        (30722, 12, 1, 17)
    );
    assert!(f.metadata.is_empty());
    let m = UserData::decode(f.body, body_limits()).unwrap();
    let initial = NotifyUserAddedInitial::decode(frame(&a).body, body_limits()).unwrap();
    let latest = UserSessionExtendedDataUpdate::decode(frame(&b).body, body_limits()).unwrap();
    assert_eq!(m.status_flags, Some(2));
    assert_eq!(
        m.user_info.unwrap().encode(body_limits()).unwrap(),
        initial.user_info.unwrap().encode(body_limits()).unwrap()
    );
    let edat = m.extended_data.unwrap().encode(body_limits()).unwrap();
    assert_eq!(
        edat,
        latest.extended_data.unwrap().encode(body_limits()).unwrap()
    );
    assert_ne!(
        edat,
        initial
            .extended_data
            .unwrap()
            .encode(body_limits())
            .unwrap()
    );
}

#[test]
fn changing_the_current_notification_changes_self_reply_without_copying_old_state() {
    let (a, b) = current(42);
    let original = Profile::from_current(&a, &b).unwrap();
    let mut update = UserSessionExtendedDataUpdate::decode(frame(&b).body, body_limits()).unwrap();
    update.extended_data.as_mut().unwrap().country = Some(b"ZZ");
    let current = wire(1, 2, 0, &[], &update.encode(body_limits()).unwrap());
    let changed = Profile::from_current(&a, &current).unwrap();
    let q = request(42, 0, 17);
    let old = original
        .reply(&q, ExternalResolution::Unsupported)
        .unwrap()
        .unwrap();
    let new = changed
        .reply(&q, ExternalResolution::Unsupported)
        .unwrap()
        .unwrap();
    assert_ne!(old, new);
    assert_eq!(
        UserData::decode(frame(&new).body, body_limits())
            .unwrap()
            .extended_data
            .unwrap()
            .country,
        Some(b"ZZ".as_slice())
    );
}

#[test]
fn external_absence_is_explicit_and_matches_independent_error_frame() {
    let p = profile(42);
    let q = request(0, 999, 0x123456);
    assert!(
        p.reply(&q, ExternalResolution::Unsupported)
            .unwrap()
            .is_none()
    );
    let actual = p.reply(&q, ExternalResolution::Absent).unwrap().unwrap();
    let expected = hex("00000000000c7802000c1234566000008eed380000972ca30082e00b");
    assert_eq!(actual, expected);
    let f = frame(&actual);
    assert!(f.body.is_empty());
    let m = Fire2Metadata::decode(f.metadata, body_limits()).unwrap();
    assert_eq!(m.context, Some(0));
    assert_eq!(m.error_code, Some(user_lookup::USER_NOT_FOUND));
    assert!(m.session_key.is_none());
    assert_eq!(m.unknown_field_count(), 0);
    assert_eq!(f.metadata.len(), 12);
    assert!(
        p.reply(&request(999, 0, 7), ExternalResolution::Absent)
            .unwrap()
            .is_none()
    );
}

#[test]
fn explicit_external_record_requires_exact_external_id_flags_and_nonself_identity() {
    let p = profile(42);
    let other = profile(43);
    let other_wire = other
        .reply(&request(43, 0, 4), ExternalResolution::Unsupported)
        .unwrap()
        .unwrap();
    let mut record = UserData::decode(frame(&other_wire).body, body_limits()).unwrap();
    record.status_flags = Some(0);
    let external = record.user_info.as_ref().unwrap().external_id.unwrap();
    let q = request(0, external, 99);
    let output = p
        .reply(&q, ExternalResolution::CurrentRecord(&record))
        .unwrap()
        .unwrap();
    let f = frame(&output);
    assert_eq!((f.fields.category, f.fields.correlation), (1, 99));
    assert_eq!(f.body, record.encode(body_limits()).unwrap());
    assert!(f.metadata.is_empty());
    assert_eq!(
        p.reply(
            &request(0, external + 1, 99),
            ExternalResolution::CurrentRecord(&record)
        ),
        Err(Error::ProfileConfig)
    );
    record.status_flags = Some(2);
    assert_eq!(
        p.reply(&q, ExternalResolution::CurrentRecord(&record)),
        Err(Error::ProfileConfig)
    );
    record.status_flags = Some(0);
    record.user_info.as_mut().unwrap().blaze_id = Some(42);
    assert_eq!(
        p.reply(&q, ExternalResolution::CurrentRecord(&record)),
        Err(Error::ProfileConfig)
    );
    record.user_info.as_mut().unwrap().blaze_id = Some(43);
    record.extended_data = None;
    assert_eq!(
        p.reply(&q, ExternalResolution::CurrentRecord(&record)),
        Err(Error::ProfileConfig)
    );
}

#[test]
fn unsupported_selector_combinations_are_not_implicitly_looked_up() {
    let p = profile(42);
    for (id, external) in [(0, 0), (42, 999), (-1, 0), (43, 0)] {
        assert!(
            p.reply(&request(id, external, 7), ExternalResolution::Absent)
                .unwrap()
                .is_none()
        );
    }
    for change in 0..7 {
        let mut q = query(42, 0);
        match change {
            0 => q.account_id = Some(9),
            1 => q.account_locale = Some(1),
            2 => q.name = Some(b"name"),
            3 => q.persona_namespace = Some(b"namespace"),
            4 => q.origin_persona_id = Some(7),
            5 => q.pid_id = Some(7),
            _ => q.external_blob = Some(Blob(b"blob")),
        }
        let w = wire(12, 0, 7, &[], &q.encode(body_limits()).unwrap());
        assert!(p.reply(&w, ExternalResolution::Absent).unwrap().is_none());
    }
}

#[test]
fn canonical_unknown_type_duplicate_and_frame_guards_reject_without_response() {
    let p = profile(42);
    let q = request(42, 0, 7);
    let f = frame(&q);
    let mut unknown = f.body.to_vec();
    unknown.extend(hex("eeeeee0001"));
    let mut duplicate = f.body.to_vec();
    duplicate.extend(hex("a64000002a"));
    let mut wrong_type = f.body.to_vec();
    wrong_type[..4].copy_from_slice(&[0x86, 0x99, 0, 1]);
    let mut reversed = nfs_heat2::decode(f.body, body_limits())
        .unwrap()
        .fields()
        .map(|f| f.unwrap().as_bytes().to_vec())
        .collect::<Vec<_>>();
    reversed.reverse();
    for body in [unknown, duplicate, wrong_type, reversed.concat()] {
        assert!(
            p.reply(&wire(12, 0, 7, &[], &body), ExternalResolution::Unsupported)
                .is_err()
        );
    }
    let mut concatenated = q.clone();
    concatenated.extend(&q);
    let mut wrong_route = q.clone();
    wrong_route[9] = 13;
    for bad in [
        q[..q.len() - 1].to_vec(),
        concatenated,
        wire(12, 0, 7, &[0], f.body),
        wire(12, 1, 7, &[], f.body),
        wrong_route,
        vec![0; user_lookup::MAX_BODY_BYTES + 145],
    ] {
        assert!(p.reply(&bad, ExternalResolution::Unsupported).is_err());
    }
}

#[test]
fn source_notifications_must_match_session_and_be_complete_and_canonical() {
    let (a, b) = current(42);
    let (_, other) = current(43);
    assert!(matches!(
        Profile::from_current(&a, &other),
        Err(Error::ProfileConfig)
    ));
    for bad in [
        vec![],
        wire(2, 1, 0, &[], frame(&a).body),
        wire(2, 2, 1, &[], frame(&a).body),
    ] {
        assert!(Profile::from_current(&bad, &b).is_err());
    }
    let mut m = UserSessionExtendedDataUpdate::decode(frame(&b).body, body_limits()).unwrap();
    m.subscribed = Some(false);
    assert!(
        Profile::from_current(&a, &wire(1, 2, 0, &[], &m.encode(body_limits()).unwrap())).is_err()
    );
    m.subscribed = Some(true);
    m.extended_data.as_mut().unwrap().qos_data = None;
    assert!(
        Profile::from_current(&a, &wire(1, 2, 0, &[], &m.encode(body_limits()).unwrap())).is_err()
    );
    let mut initial = NotifyUserAddedInitial::decode(frame(&a).body, body_limits()).unwrap();
    initial.user_info.as_mut().unwrap().name = None;
    assert!(
        Profile::from_current(
            &wire(2, 2, 0, &[], &initial.encode(body_limits()).unwrap()),
            &b
        )
        .is_err()
    );
}

#[test]
fn oversized_current_fields_and_collections_are_rejected() {
    let (a, b) = current(42);
    let long = vec![b'x'; 1024];
    let mut m = UserSessionExtendedDataUpdate::decode(frame(&b).body, body_limits()).unwrap();
    m.extended_data.as_mut().unwrap().country = Some(&long);
    let body = m.encode(nfs_heat2::Limits::default()).unwrap();
    assert!(Profile::from_current(&a, &wire(1, 2, 0, &[], &body)).is_err());
    m.extended_data.as_mut().unwrap().country = Some(b"");
    m.extended_data.as_mut().unwrap().latency_list = Some(LatencyList(vec![0; 129]));
    let body = m.encode(nfs_heat2::Limits::default()).unwrap();
    assert!(Profile::from_current(&a, &wire(1, 2, 0, &[], &body)).is_err());
}

#[test]
fn repeats_and_correlation_boundaries_are_deterministic_and_profiles_are_isolated() {
    let a = profile(42);
    let b = profile(43);
    for correlation in [0, 1, 0x00ff_ffff] {
        let q = request(42, 0, correlation);
        let one = a
            .reply(&q, ExternalResolution::Unsupported)
            .unwrap()
            .unwrap();
        assert_eq!(
            a.reply(&q, ExternalResolution::Unsupported)
                .unwrap()
                .unwrap(),
            one
        );
        assert_eq!(frame(&one).fields.correlation, correlation);
        assert!(
            b.reply(&q, ExternalResolution::Unsupported)
                .unwrap()
                .is_none()
        );
    }
    std::thread::scope(|s| {
        let a = s.spawn(|| {
            a.reply(&request(42, 0, 7), ExternalResolution::Unsupported)
                .unwrap()
                .unwrap()
        });
        let b = s.spawn(|| {
            b.reply(&request(43, 0, 7), ExternalResolution::Unsupported)
                .unwrap()
                .unwrap()
        });
        assert_ne!(a.join().unwrap(), b.join().unwrap());
    });
}

#[test]
fn explicit_single_account_directory_policy_does_not_resolve_own_external_id() {
    use user_lookup::single_account_external_resolution as resolve;
    assert!(matches!(
        resolve(&request(0, 44, 1), 43),
        ExternalResolution::Absent
    ));
    for query in [request(0, 43, 1), request(42, 0, 1), vec![0; 5]] {
        assert!(matches!(
            resolve(&query, 43),
            ExternalResolution::Unsupported
        ));
    }
}

#[test]
fn every_partial_frame_and_invalid_header_preserves_session_state() {
    let mut p = profile(42);
    let query = request(42, 0, 1);
    let hardware = wire(
        8,
        0,
        1,
        &[],
        &UpdateHardwareFlagsRequest {
            hardware_flags: Some(1),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap(),
    );
    let before = p.reply(&query, ExternalResolution::Unsupported).unwrap();
    for end in 0..query.len() {
        assert!(
            p.reply(&query[..end], ExternalResolution::Unsupported)
                .is_err()
        );
    }
    for end in 0..hardware.len() {
        assert!(p.update_hardware(&hardware[..end]).is_err());
    }
    for base in [&query, &hardware] {
        let original = frame(base);
        for change in 0..3 {
            let mut fields = original.fields;
            match change {
                0 => fields.slot = 1,
                1 => fields.reserved = [1, 0],
                _ => fields.category = 2,
            }
            let bad = nfs_fire2::encode(
                Frame {
                    fields,
                    metadata: &[],
                    body: original.body,
                },
                frame_limits(),
            )
            .unwrap();
            if base == &query {
                assert!(p.reply(&bad, ExternalResolution::Unsupported).is_err());
            } else {
                assert!(p.update_hardware(&bad).is_err());
            }
        }
    }
    assert_eq!(
        p.reply(&query, ExternalResolution::Unsupported).unwrap(),
        before
    );
}
