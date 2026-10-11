// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::autolog::*;
use nfs_services::local_social::{Current, Error, body_limits, frame_limits};
fn wire(command: u16, category: u8, corr: u32, body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2050,
                routing_b: command,
                category,
                correlation: corr,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        Default::default(),
    )
    .unwrap()
}
fn random(corr: u32) -> Vec<u8> {
    wire(76, 0, corr, &[0xb7, 0x0b, 0x21, 0, 50])
}
fn recent() -> Vec<u8> {
    wire(
        70,
        0,
        8,
        &[
            0x9b, 0x0d, 0x39, 0, 0, 0xb6, 0x1e, 0x30, 0, 0, 0xd3, 0x9c, 0x25, 0, 0,
        ],
    )
}
fn frame(b: &[u8]) -> Frame<'_> {
    let d = nfs_fire2::decode(b, frame_limits()).unwrap().unwrap();
    assert_eq!(d.consumed, b.len());
    d.frame
}
#[test]
fn explicit_empty_local_population_has_independent_empty_list_bytes() {
    let p = Current::new(42, &[], true).unwrap();
    let b = p.reply(&random(9)).unwrap().unwrap();
    assert_eq!(frame(&b).body, [0x8a, 0xca, 0x64, 4, 0, 0]);
    assert_eq!(frame(&b).fields.correlation, 9);
}

#[test]
fn empty_recommendations_require_explicit_current_account_state() {
    let q = wire(26, 0, 321, &[0x8a, 0xca, 0x64, 0, 42]);
    let old = Current::new(42, &[], true).unwrap();
    assert!(old.reply(&q).unwrap().is_none());
    let p = old.with_empty_friend_recommendations();
    let other = Current::new(43, &[], true)
        .unwrap()
        .with_empty_friend_recommendations();
    let r = p.reply(&q).unwrap().unwrap();
    assert_eq!(r, wire(26, 1, 321, &[]));
    assert_eq!(p.reply(&q).unwrap(), Some(r));
    assert!(other.reply(&q).unwrap().is_none());
    for body in [
        vec![],
        vec![0x8a, 0xca, 0x64, 0, 0],
        vec![0x8a, 0xca, 0x64, 0, 43],
        vec![0x8a, 0xca, 0x64, 0, 42, 0xff, 0xff, 0xff, 0, 1],
    ] {
        assert!(p.reply(&wire(26, 0, 322, &body)).unwrap().is_none());
    }
    let joined = [q.clone(), q.clone()].concat();
    assert!(p.reply(&joined).is_err());
    for split in 0..q.len() {
        assert!(p.reply(&q[..split]).is_err());
    }
}
#[test]
fn local_populations_are_isolated_owned_and_repeatable() {
    let mut ids = vec![43, 44];
    let a = Current::new(42, &ids, true).unwrap();
    ids[0] = 99;
    let b = Current::new(70, &[71], true).unwrap();
    let q = random(1);
    let aw = a.reply(&q).unwrap().unwrap();
    let bw = b.reply(&q).unwrap().unwrap();
    assert_eq!(
        RandomPlayersResponse::decode(frame(&aw).body, body_limits())
            .unwrap()
            .blaze_ids
            .unwrap()
            .0,
        vec![43, 44]
    );
    assert_eq!(
        RandomPlayersResponse::decode(frame(&bw).body, body_limits())
            .unwrap()
            .blaze_ids
            .unwrap()
            .0,
        vec![71]
    );
    assert_eq!(a.reply(&q).unwrap().unwrap(), aw);
    std::thread::scope(|threads| {
        let first = threads.spawn(|| a.reply(&q).unwrap().unwrap());
        let second = threads.spawn(|| b.reply(&q).unwrap().unwrap());
        assert_eq!(first.join().unwrap(), aw);
        assert_eq!(second.join().unwrap(), bw);
    });
}
#[test]
fn player_snapshot_rejects_self_duplicates_invalid_ids_and_over_limit() {
    for ids in [
        vec![42],
        vec![0],
        vec![-1],
        vec![43, 43],
        (100..151).collect(),
    ] {
        assert!(matches!(Current::new(42, &ids, true), Err(Error::Context)));
    }
    assert!(Current::new(0, &[], true).is_err());
    let ids: Vec<_> = (100..150).collect();
    let p = Current::new(42, &ids, true).unwrap();
    let b = p.reply(&random(3)).unwrap().unwrap();
    assert_eq!(
        RandomPlayersResponse::decode(frame(&b).body, body_limits())
            .unwrap()
            .blaze_ids
            .unwrap()
            .0
            .len(),
        50
    );
}
#[test]
fn empty_history_is_explicit_and_only_observed_query_is_supported() {
    let p = Current::new(42, &[], true).unwrap();
    let q = recent();
    let b = p.reply(&q).unwrap().unwrap();
    assert_eq!(b, wire(70, 1, 8, &[]));
    let nonempty = Current::new(42, &[], false).unwrap();
    assert!(nonempty.reply(&q).unwrap().is_none());
    let mut q = q;
    let end = q.len() - 1;
    q[end] = 1;
    assert!(p.reply(&q).unwrap().is_none());
}
#[test]
fn unsupported_variants_unknowns_and_bad_profiles_receive_no_success() {
    let p = Current::new(42, &[], true).unwrap();
    for q in [
        wire(75, 0, 1, &[0x8a, 0xca, 0x64, 0, 42]),
        wire(76, 0, 1, &[0xb7, 0x0b, 0x21, 0, 49]),
        wire(78, 0, 1, &[]),
        wire(76, 0, 1, &[]),
        wire(76, 0, 1, &[0xb7, 0x0b, 0x21, 0, 50, 0xff, 0xff, 0xff, 0, 0]),
    ] {
        assert!(p.reply(&q).unwrap().is_none());
    }
}
#[test]
fn framed_input_requires_one_complete_bounded_request() {
    let p = Current::new(42, &[], true).unwrap();
    let q = random(1);
    for cut in 0..q.len() {
        assert!(p.reply(&q[..cut]).is_err());
    }
    assert!(p.reply(&[q.as_slice(), q.as_slice()].concat()).is_err());
    for i in [6, 7, 13, 14, 15] {
        let mut b = q.clone();
        b[i] ^= 1;
        assert!(p.reply(&b).is_err());
    }
    let mut b = q;
    b[5] = 1;
    assert!(p.reply(&b).is_err());
    assert!(p.reply(&vec![0; 4113]).is_err());
}

#[test]
fn unavailable_history_never_becomes_an_empty_success_and_queries_are_bounded() {
    let state = Current::new(42, &[i64::MAX], false).unwrap();
    assert!(state.reply(&recent()).unwrap().is_none());
    let result = state.reply(&random(0x00ff_ffff)).unwrap().unwrap();
    let decoded = RandomPlayersResponse::decode(frame(&result).body, body_limits()).unwrap();
    assert_eq!(decoded.blaze_ids.unwrap().0, vec![i64::MAX]);
    for cut in 0..recent().len() {
        assert!(state.reply(&recent()[..cut]).is_err());
    }
    assert!(Current::new(-1, &[], true).is_err());
    for route in [(2050, 26), (2050, 70), (2050, 76)] {
        assert!(nfs_services::local_social::owns(route.0, route.1));
    }
    assert!(!nfs_services::local_social::owns(2050, 75));
}
