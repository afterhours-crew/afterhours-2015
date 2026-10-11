// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::world::rpc::Serial;
fn binding(selector: u16) -> Binding {
    Binding {
        endpoint: Endpoint {
            scene: 9,
            selector,
            serial: Serial::new(7).unwrap(),
        },
        participant: 20,
        vehicle: 30 + selector,
    }
}
#[test]
fn both_wire_directions_check_lengths_routes_references_and_ignore_padding() {
    let b = binding(4);
    let host = b.encode().unwrap();
    let ack = b.expected().encode().unwrap();
    assert_eq!((host.len(), ack.len()), (176, 168));
    assert_eq!(Binding::decode(host.span()), Ok(b));
    assert_eq!(Loaded::decode(ack.span()), Ok(b.expected()));
    for n in 0..host.len() {
        assert!(Binding::decode(BitSpan::new(host.bytes(), 0, n).unwrap()).is_err());
    }
    for n in 0..ack.len() {
        assert!(Loaded::decode(BitSpan::new(ack.bytes(), 0, n).unwrap()).is_err());
    }
    let mut h = host.bytes().to_vec();
    h[21] |= 31;
    assert_eq!(Binding::decode(BitSpan::new(&h, 0, 176).unwrap()), Ok(b));
    h[21] |= 32;
    assert!(Binding::decode(BitSpan::new(&h, 0, 176).unwrap()).is_err());
    let mut a = ack.bytes().to_vec();
    a[20] |= 127;
    assert_eq!(
        Loaded::decode(BitSpan::new(&a, 0, 168).unwrap()),
        Ok(b.expected())
    );
    a[20] &= 127;
    assert!(Loaded::decode(BitSpan::new(&a, 0, 168).unwrap()).is_err());
    for raw in [&mut h, &mut a] {
        raw.push(0);
    }
    assert!(Binding::decode(BitSpan::new(&h, 0, 177).unwrap()).is_err());
    assert!(Loaded::decode(BitSpan::new(&a, 0, 169).unwrap()).is_err());
    for id in [0, 8192, u16::MAX] {
        assert!(Binding { vehicle: id, ..b }.encode().is_err());
    }
}
#[test]
fn exact_pending_handshakes_are_owned_idempotent_bounded_and_isolated() {
    let mut p = Pending::default();
    assert!(p.acknowledge(binding(0).expected(), |_| true).is_err());
    for n in 0..5 {
        assert_eq!(p.offer(binding(n)), Ok(Some(binding(n))));
    }
    assert_eq!(p.offer(binding(5)), Err(Error::Bound));
    assert!(!p.all_loaded(20, 5));
    let before = p.clone();
    assert_eq!(
        p.acknowledge(binding(0).expected(), |_| false),
        Err(Error::UnknownObject)
    );
    assert!(
        p.acknowledge(
            Loaded {
                vehicle: 99,
                ..binding(0).expected()
            },
            |_| true
        )
        .is_err()
    );
    assert_eq!(p, before);
    for n in [4, 0, 3, 1, 2] {
        assert_eq!(
            p.acknowledge(binding(n).expected(), |id| id == 20),
            Ok(true)
        );
        assert_eq!(p.acknowledge(binding(n).expected(), |_| true), Ok(false));
        assert_eq!(p.offer(binding(n)), Ok(None));
    }
    assert!(p.all_loaded(20, 5));
    assert!(!p.all_loaded(20, 4));
    assert!(!Pending::default().all_loaded(20, 1));
    assert!(
        p.offer(Binding {
            vehicle: 99,
            ..binding(0)
        })
        .is_err()
    );
    let other = Binding {
        participant: 21,
        ..binding(0)
    };
    p.offer(other).unwrap();
    assert!(!p.all_loaded(21, 1));
    assert!(p.all_loaded(20, 5));
}

#[test]
fn participant_capacity_and_duplicate_vehicle_checks_do_not_partly_mutate() {
    let mut p = Pending::default();
    for id in 1..=128 {
        p.offer(Binding {
            participant: id,
            vehicle: id + 200,
            ..binding(0)
        })
        .unwrap();
    }
    let old = p.clone();
    assert_eq!(
        p.offer(Binding {
            participant: 129,
            ..binding(0)
        }),
        Err(Error::Bound)
    );
    assert_eq!(
        p.offer(Binding {
            participant: 1,
            vehicle: 201,
            ..binding(1)
        }),
        Err(Error::DuplicateObject)
    );
    assert_eq!(p, old);
}
