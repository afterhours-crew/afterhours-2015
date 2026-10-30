// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::replication::{
    state::World,
    sublevel::{Content, Kind, Profile, ordinary},
};
use nfs_protocol::world::rpc::Serial;

fn bound(count: usize) -> (Activation, Record, Catalog) {
    let mut kinds = vec![
        Kind::Noop,
        Kind::RpcBool,
        Kind::Rpc,
        Kind::RpcFourReferences,
    ];
    kinds.extend(std::iter::repeat_n(Kind::Rpc, count));
    let content = Content::new(vec![(123, Profile::new(false, &kinds).unwrap())]).unwrap();
    let creation = ordinary::creation(
        5,
        123,
        None,
        Serial::new(17).unwrap(),
        &ordinary::unpopulated(content.profile(123).unwrap()).unwrap(),
        &content,
    )
    .unwrap();
    let mut world = World::default();
    world.register_levels(&[5]).unwrap();
    let record = world.spawn_scene(creation, &content).unwrap();
    let site = |serializer| Site {
        scene_key: 123,
        serializer,
    };
    let catalog = Catalog {
        volumes: (4..4 + count).map(site).collect(),
        presence: site(1),
        streamed: site(2),
        location: site(3),
    };
    (
        Activation::bind(std::slice::from_ref(&record), &catalog).unwrap(),
        record,
        catalog,
    )
}

#[test]
fn early_location_preserves_activation_order_and_requires_ready_owner() {
    let (mut a, r, _) = bound(2);
    let location = client(r.id, 7, 2, 0, None, 0);
    let before = a.clone();
    assert_eq!(
        a.receive(location.span(), |_| true),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        a.receive_ready(location.span(), |_| false, |_| true),
        Err(Error::UnknownObject)
    );
    assert_eq!(a, before);
    assert_eq!(
        a.receive_ready(location.span(), |_| true, |_| true)
            .unwrap(),
        Some(Report::Location {
            participant: 7,
            first: true
        })
    );
    assert_eq!(
        a.receive_ready(location.span(), |_| true, |_| true)
            .unwrap(),
        Some(Report::Location {
            participant: 7,
            first: false
        })
    );
    assert!(a.participants().next().is_none());
    let crossing = client(r.id, 7, 3, 2, Some(1.), 0);
    assert_eq!(
        a.receive_ready(crossing.span(), |_| true, |_| true),
        Err(Error::UnknownObject)
    );
    assert!(a.set(7, true, 3, |_| true).unwrap().is_empty());
    assert!(a.located_before_activation.contains(&7));
    assert_eq!(a.set(7, true, 4, |_| true).unwrap().len(), 4);
    assert!(a.located_before_activation.is_empty());
    assert_eq!(
        a.receive(location.span(), |_| true).unwrap(),
        Some(Report::Location {
            participant: 7,
            first: false
        })
    );
    // The leave now carries the streamed-leave call as well (capacity 4).
    assert!(a.set(7, false, 3, |_| true).unwrap().is_empty());
    assert_eq!(a.set(7, false, 4, |_| true).unwrap().len(), 4);
    assert_eq!(
        a.receive(location.span(), |_| true).unwrap(),
        Some(Report::Retired { participant: 7 })
    );
}

#[test]
fn pending_locations_are_bounded_and_detach_clears_unpublished_state() {
    let (mut a, r, _) = bound(1);
    for id in 1..=MAX_PARTICIPANTS as u16 {
        a.receive_ready(client(r.id, id, 2, 0, None, 0).span(), |_| true, |_| true)
            .unwrap();
    }
    let before = a.clone();
    assert_eq!(
        a.receive_ready(client(r.id, 129, 2, 0, None, 0).span(), |_| true, |_| true),
        Err(Error::Bound)
    );
    assert_eq!(a, before);
    assert!(a.set(7, false, 0, |_| true).unwrap().is_empty());
    assert!(!a.located_before_activation.contains(&7));
    a.receive_ready(client(r.id, 129, 2, 0, None, 0).span(), |_| true, |_| true)
        .unwrap();
    let other = bound(1).0;
    assert!(other.located_before_activation.is_empty());
}
fn client(
    scene: u16,
    participant: u16,
    selector: u16,
    method: u32,
    clock: Option<f64>,
    token: u32,
) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(selector.into(), 9).put(method.into(), 32);
    if let Some(clock) = clock {
        payload.put(clock.to_bits(), 64).put(token.into(), 32);
    }
    payload.put(73, 7); // Alignment bits are intentionally nonzero.
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(scene.into(), 13)
        .put(participant.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}
#[test]
fn large_batch_is_atomic_repeat_safe_and_shared_presence_tracks_occupancy() {
    let (mut a, _, _) = bound(160);
    let original = a.clone();
    assert!(a.set(7, true, 161, |_| true).unwrap().is_empty());
    assert_eq!(a, original);
    assert_eq!(a.set(7, true, 162, |_| false), Err(Error::UnknownObject));
    assert_eq!(a, original);
    let first = a.set(7, true, 162, |_| true).unwrap();
    assert_eq!(first.len(), 162);
    assert_eq!(first[0].call, Call::StreamedParticipant);
    assert_eq!(first[1].call, Call::Presence(true));
    assert!(first[2..].iter().all(|v| v.participant == Some(7)
        && v.call
            == Call::Subscribe {
                token: 0,
                inside: false
            }));
    assert!(a.set(7, true, 162, |_| true).unwrap().is_empty());
    let second = a.set(8, true, 161, |_| true).unwrap();
    assert_eq!(second.len(), 161);
    assert!(!second.iter().any(|v| matches!(v.call, Call::Presence(_))));
    let before = a.clone();
    assert!(a.set(7, false, 160, |_| true).unwrap().is_empty());
    assert_eq!(a, before);
    let leave = a.set(7, false, 161, |_| true).unwrap();
    assert_eq!(leave.len(), 161);
    assert_eq!(leave[0].call, Call::StreamedParticipantLeave);
    assert_eq!(leave[0].participant, Some(7));
    assert!(leave[1..].iter().all(|v| v.call == Call::Unsubscribe));
    assert!(a.set(8, false, 161, |_| true).unwrap().is_empty());
    let last = a.set(8, false, 162, |_| true).unwrap();
    assert_eq!(last[0].call, Call::StreamedParticipantLeave);
    assert_eq!(last[1].call, Call::Presence(false));
    assert_eq!(a.members, original.members);
    assert_eq!(a.retired, [7, 8].into_iter().collect());
    assert_eq!(a.set(7, true, 162, |_| true).unwrap(), first);
    assert_eq!(a.retired, [8].into_iter().collect());
}
#[test]
fn generated_wire_has_typed_references_payload_and_zero_padding() {
    let (mut a, record, _) = bound(1);
    let calls = a.set(71, true, 3, |_| true).unwrap();
    for (i, call) in calls.iter().enumerate() {
        let wire = call.encode().unwrap();
        let e = Envelope::decode(
            wire.span(),
            Limits {
                max_input_bits: 512,
                max_references: 2,
                max_payload_bytes: 16,
            },
        )
        .unwrap();
        let route = e.route(RouteProfile::ClientReceive).unwrap();
        assert_eq!(
            e.references(),
            if i == 1 {
                vec![record.id]
            } else {
                vec![record.id, 71]
            }
        );
        assert_eq!(route.serial(), Serial::new(17));
        assert_eq!(route.method_index(), if i == 0 { 3 } else { 0 });
        assert_eq!(route.arguments().len(), if i == 2 { 37 } else { 5 });
        assert!(
            (0..route.arguments().len()).all(|bit| route.arguments().read_u32(bit, 1) == Ok(0))
        );
        assert!(e.remaining().is_empty());
    }
    let encoded = crate::session::replication_frame(
        &crate::replication::section::Section {
            float_bits: None,
            flag: false,
            deleted: vec![],
            setup: None,
            records: vec![],
        },
        &calls
            .into_iter()
            .map(crate::participants::HostRpc::WorldActivation)
            .collect::<Vec<_>>(),
        0,
    )
    .unwrap();
    let parsed =
        crate::frame::parse_prefix(encoded.span(), crate::frame::Direction::FromHost).unwrap();
    assert_eq!(parsed.messages.unwrap().groups[0].messages.len(), 3);
}
#[test]
fn reports_are_owned_monotonic_isolated_and_never_invent_host_replies() {
    let (mut a, r, _) = bound(2);
    a.set(7, true, 10, |_| true).unwrap();
    a.set(8, true, 10, |_| true).unwrap();
    let enter = client(r.id, 7, 3, 2, Some(12.5), 0);
    let before = a.clone();
    assert_eq!(
        a.receive(enter.span(), |_| false),
        Err(Error::UnknownObject)
    );
    assert_eq!(a, before);
    assert!(matches!(
        a.receive(enter.span(), |p| p == 7).unwrap(),
        Some(Report::Crossing {
            inside: true,
            changed: true,
            token: 0,
            ..
        })
    ));
    assert!(matches!(
        a.receive(enter.span(), |_| true).unwrap(),
        Some(Report::Crossing { changed: false, .. })
    ));
    assert!(matches!(
        a.receive(client(r.id, 7, 3, 3, Some(13.), 0).span(), |_| true)
            .unwrap(),
        Some(Report::Crossing {
            inside: false,
            changed: true,
            ..
        })
    ));
    assert!(matches!(
        a.receive(enter.span(), |_| true).unwrap(),
        Some(Report::Crossing { changed: false, .. })
    ));
    assert!(matches!(
        a.receive(client(r.id, 8, 3, 2, Some(12.5), 0).span(), |_| true)
            .unwrap(),
        Some(Report::Crossing { changed: true, .. })
    ));
    assert_eq!(a.members[&7].observations[&(r.id, 3)], (13., false));
    assert_eq!(a.members[&8].observations[&(r.id, 3)], (12.5, true));
    let location = client(r.id, 7, 2, 0, None, 0);
    assert_eq!(
        a.receive(location.span(), |_| true).unwrap(),
        Some(Report::Location {
            participant: 7,
            first: true
        })
    );
    assert_eq!(
        a.receive(location.span(), |_| true).unwrap(),
        Some(Report::Location {
            participant: 7,
            first: false
        })
    );
    a.set(7, false, 10, |_| true).unwrap();
    assert_eq!(
        a.receive(enter.span(), |_| true).unwrap(),
        Some(Report::Retired { participant: 7 })
    );
    assert_eq!(
        a.receive(enter.span(), |_| false),
        Err(Error::UnknownObject)
    );
    let other = bound(2).0;
    assert!(other.members.is_empty());
    assert_eq!(a.members.len(), 1);
}

#[test]
fn crossing_codec_preserves_generation_clock_order_and_nonzero_padding() {
    let crossing = client(101, 71, 19, 2, Some(123.75), 0x89ab_cdef);
    assert_eq!(
        Crossing::decode(crossing.span()).unwrap(),
        Some(Crossing {
            object: 101,
            selector: 19,
            participant: 71,
            token: 0x89ab_cdef,
            seconds: 123.75,
            inside: true,
        })
    );
    let leaving = client(101, 71, 19, 3, Some(5.), u32::MAX);
    assert!(!Crossing::decode(leaving.span()).unwrap().unwrap().inside);
    let location = client(101, 71, 19, 0, None, 0);
    assert_eq!(Crossing::decode(location.span()).unwrap(), None);
    // The native mode-filter handler ignores this clock. The ordinary world
    // activation receiver separately imposes its finite/monotonic policy.
    let nan = client(101, 71, 19, 2, Some(f64::NAN), 7);
    assert!(
        Crossing::decode(nan.span())
            .unwrap()
            .unwrap()
            .seconds
            .is_nan()
    );
    let mut trailing = crossing.clone();
    trailing.put(1, 1);
    assert_eq!(Crossing::decode(trailing.span()), Err(Error::Shape));
    let truncated =
        nfs_protocol::world::BitSpan::new(crossing.bytes(), 0, crossing.span().len() - 1).unwrap();
    assert_eq!(Crossing::decode(truncated), Err(Error::Shape));
}
#[test]
fn malformed_reports_and_unbound_routes_do_not_mutate_state() {
    let (mut a, r, _) = bound(1);
    a.set(7, true, 10, |_| true).unwrap();
    let before = a.clone();
    for (seconds, token) in [(f64::NAN, 0), (f64::INFINITY, 0), (-1., 0), (1., 1)] {
        assert!(
            a.receive(client(r.id, 7, 3, 2, Some(seconds), token).span(), |_| true)
                .is_err()
        );
        assert_eq!(a, before);
    }
    let wire = client(r.id, 7, 3, 2, Some(1.), 0);
    for end in 0..wire.len() {
        assert!(
            a.receive(wire.span().slice(0, end).unwrap(), |_| true)
                .is_err()
        );
        assert_eq!(a, before);
    }
    for (scene, selector, method) in [(r.id + 1, 3, 2), (r.id, 4, 2), (r.id, 3, 9)] {
        assert_eq!(
            a.receive(
                client(scene, 7, selector, method, Some(1.), 0).span(),
                |_| true
            )
            .unwrap(),
            None
        );
        assert_eq!(a, before);
    }
    let mut trailing = wire;
    trailing.put(0, 1);
    assert!(a.receive(trailing.span(), |_| true).is_err());
    assert_eq!(a, before);
}
#[test]
fn binding_and_member_limits_fail_before_changes() {
    let (mut a, r, mut c) = bound(1);
    c.volumes.push(c.volumes[0]);
    assert_eq!(
        Activation::bind(std::slice::from_ref(&r), &c),
        Err(Error::DuplicateObject)
    );
    c.volumes.pop();
    c.volumes[0].serializer = 1;
    assert!(Activation::bind(std::slice::from_ref(&r), &c).is_err());
    c.volumes[0].scene_key = 222;
    assert_eq!(Activation::bind(&[r], &c), Err(Error::UnknownObject));
    for p in 1..=MAX_PARTICIPANTS as u16 {
        a.set(p, true, 10, |_| true).unwrap();
    }
    let before = a.clone();
    assert_eq!(a.set(129, true, 10, |_| true), Err(Error::Bound));
    assert_eq!(a, before);
}

#[test]
fn retired_participants_report_inertly_until_reactivated_and_retirement_is_bounded() {
    let (mut a, r, _) = bound(2);
    a.set(7, true, 10, |_| true).unwrap();
    let enter = client(r.id, 7, 3, 2, Some(1.), 0);
    assert!(matches!(
        a.receive(enter.span(), |_| true).unwrap(),
        Some(Report::Crossing { .. })
    ));
    // The host retires membership when it queues the unsubscription; the
    // client keeps reporting until that notification arrives (E838).
    a.set(7, false, 10, |_| true).unwrap();
    let before = a.clone();
    let leave = client(r.id, 7, 3, 3, Some(2.), 0);
    let location = client(r.id, 7, 2, 0, None, 0);
    for report in [&leave, &location, &enter] {
        assert_eq!(
            a.receive_ready(report.span(), |_| true, |_| false).unwrap(),
            Some(Report::Retired { participant: 7 })
        );
    }
    assert_eq!(a, before);
    // Ownership and shape still decide; never-activated participants are unknown.
    assert_eq!(
        a.receive(leave.span(), |_| false),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        a.receive(client(r.id, 7, 3, 3, Some(-1.), 0).span(), |_| true),
        Err(Error::Shape)
    );
    assert_eq!(
        a.receive(client(r.id, 8, 3, 3, Some(2.), 0).span(), |_| true),
        Err(Error::UnknownObject)
    );
    assert_eq!(a, before);
    // Reactivation clears retirement and restores live reports.
    a.set(7, true, 10, |_| true).unwrap();
    assert!(!a.retired.contains(&7));
    assert!(matches!(
        a.receive(enter.span(), |_| true).unwrap(),
        Some(Report::Crossing { changed: true, .. })
    ));
    // Retirement follows ownership and is bounded like membership.
    a.set(7, false, 10, |_| true).unwrap();
    a.set(9, true, 10, |_| true).unwrap();
    a.set(9, false, 10, |id| id == 9).unwrap();
    assert_eq!(a.retired, [9].into_iter().collect());
    for id in 10..10 + MAX_PARTICIPANTS as u16 {
        a.set(id, true, 10, |_| true).unwrap();
        a.set(id, false, 10, |_| true).unwrap();
    }
    assert_eq!(a.retired.len(), MAX_PARTICIPANTS);
    assert!(!a.retired.contains(&9));
    assert!(a.retired.contains(&10));
    assert!(a.retired.contains(&(9 + MAX_PARTICIPANTS as u16)));
}
