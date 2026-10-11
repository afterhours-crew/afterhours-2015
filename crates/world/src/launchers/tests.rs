// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use sublevel::{Content, Kind, Profile, Rpc};
fn bound() -> Launchers {
    let content = Content::new(vec![(
        7,
        Profile::new(false, &[Kind::Noop, Kind::Rpc]).unwrap(),
    )])
    .unwrap();
    let catalog = Catalog::new(vec![(7, vec![1])], &content).unwrap();
    let creation = sublevel::Creation {
        prefix: sublevel::Prefix {
            level_id: 1,
            content_key: 7,
            blueprint: None,
            word_a: 0,
            word_b: 0,
            constructor_word: None,
        },
        body: sublevel::Body {
            initial: Some(vec![
                sublevel::Initial::Empty,
                sublevel::Initial::Rpc(Rpc {
                    selector: 172,
                    serial: Serial::new(5).unwrap(),
                }),
            ]),
            updates: vec![Some(sublevel::Update::Noop); 2],
        },
    };
    let record = Record::from_scene(82, creation, &content).unwrap();
    let mut model = Launchers::default();
    model.bind(&[record], &catalog).unwrap();
    model
}
const REQUEST: [u8; 20] = [
    0, 0, 0, 0, 0, 0, 0, 0, 2, 0x02, 0x90, 0x02, 0x40, 0xca, 0xc0, 0, 0, 0, 0x20, 0,
];
const REPLY: [u8; 19] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 0x02, 0x90, 0x1d, 0x58, 0x02, 0x80, 0, 0, 0, 0,
];
fn request() -> BitSpan<'static> {
    BitSpan::new(&REQUEST, 0, 155).unwrap()
}

#[test]
fn entity_roles_are_atomic_and_reject_another_owned_player() {
    use crate::replication::entity::{self, creation};
    let asset = creation::Asset {
        bundle: 1,
        type_id: 3404,
        local_index: 0,
    };
    let content = creation::Content::new(
        vec![creation::Catalog::new(1, &[(3404, 1)]).unwrap()],
        vec![(
            asset,
            entity::Profile::new(&[entity::Kind::Root, entity::Kind::Rpc]).unwrap(),
        )],
    )
    .unwrap();
    let record = Record::from_entity(
        82,
        creation::Creation {
            prefix: creation::Prefix {
                parent: None,
                blueprint: 1,
                sub_id: 1,
                owner: None,
                asset,
            },
            body: entity::Body {
                initial: Some(vec![
                    entity::Initial::Root {
                        value: 1,
                        rpc: entity::Rpc {
                            selector: 0,
                            serial: Serial::new(5).unwrap(),
                        },
                        reference: 7,
                    },
                    entity::Initial::Rpc(entity::Rpc {
                        selector: 172,
                        serial: Serial::new(5).unwrap(),
                    }),
                ]),
                updates: vec![Some(entity::Update::Noop); 2],
            },
        },
        &content,
    )
    .unwrap();
    let mut model = Launchers::default();
    let before = model.clone();
    assert_eq!(
        model.bind_entity(&record, &[1, 1], 9),
        Err(replication::Error::DuplicateObject)
    );
    assert_eq!(model, before);
    assert_eq!(
        model.bind_entity(&record, &[1, 0], 9),
        Err(replication::Error::TypeMismatch)
    );
    assert_eq!(model, before);
    model.bind_entity(&record, &[1], 10).unwrap();
    assert_eq!(
        model.receive(request(), |_| true),
        Err(replication::Error::UnknownObject)
    );
    assert_eq!(model.enabled_count(), 0);
    let mut other = Launchers::default();
    other.bind_entity(&record, &[1], 9).unwrap();
    assert!(matches!(
        other.receive(request(), |_| true).unwrap(),
        Outcome::Enabled(_)
    ));
    assert_eq!(model.enabled_count(), 0);
    let before = other.clone();
    assert!(other.bind_entity(&record, &[1], 9).is_err());
    assert_eq!(other, before);
}
#[test]
fn grounded_pair_is_idempotent_and_per_player_per_world() {
    let mut a = bound();
    let b = bound();
    let Outcome::Enabled(reply) = a.receive(request(), |id| id == 9).unwrap() else {
        panic!()
    };
    let encoded = reply.encode().unwrap();
    assert_eq!((encoded.len(), encoded.bytes()), (150, REPLY.as_slice()));
    assert_eq!(
        a.receive(request(), |id| id == 9),
        Ok(Outcome::Repeated(reply))
    );
    assert_eq!(a.enabled_count(), 1);
    assert_eq!(b.enabled_count(), 0);
    assert_eq!(
        a.receive(request(), |_| false),
        Err(replication::Error::UnknownObject)
    );
    let envelope = Envelope::decode(
        encoded.span(),
        Limits {
            max_input_bits: 200,
            max_references: 1,
            max_payload_bytes: 7,
        },
    )
    .unwrap();
    let route = envelope.route(RouteProfile::ClientReceive).unwrap();
    assert_eq!(route.method_index(), 0);
    assert_eq!(route.serial(), Serial::new(5));
    assert_eq!(route.arguments().read_u32(0, 5), Ok(0));
}
#[test]
fn malformed_unknown_and_full_states_never_mutate() {
    let mut model = bound();
    let original = model.clone();
    for bits in 0..155 {
        assert!(
            model
                .receive(BitSpan::new(&REQUEST, 0, bits).unwrap(), |_| true)
                .is_err()
        );
    }
    let mut bad = REQUEST;
    bad[0] = 1;
    assert!(
        model
            .receive(BitSpan::new(&bad, 0, 155).unwrap(), |_| true)
            .is_err()
    );
    assert_eq!(model, original);
    let mut padded = REQUEST;
    padded[19] = 0xe0;
    assert!(matches!(
        model.receive(BitSpan::new(&padded, 0, 155).unwrap(), |_| true),
        Ok(Outcome::Enabled(_))
    ));
    model = original.clone();
    let mut unknown = REQUEST;
    unknown[18] = 0x30;
    assert_eq!(
        model.receive(BitSpan::new(&unknown, 0, 155).unwrap(), |_| true),
        Ok(Outcome::Unsupported)
    );
    assert_eq!(model, original);
    for n in 0..MAX_ENABLED {
        model
            .enabled
            .insert(((n / 512 + 1) as u16, (n % 512) as u16, 10));
    }
    assert_eq!(
        model.receive(request(), |_| true),
        Err(replication::Error::Bound)
    );
    assert_eq!(model.enabled_count(), MAX_ENABLED);
}
#[test]
fn static_roles_and_endpoint_bounds_are_strict() {
    let content = Content::new(vec![(
        7,
        Profile::new(false, &[Kind::Noop, Kind::Rpc, Kind::BoolProperty]).unwrap(),
    )])
    .unwrap();
    for entries in [
        vec![(7, vec![1, 1])],
        vec![(7, vec![0])],
        vec![(7, vec![2])],
        vec![(8, vec![1])],
        vec![(7, vec![1]), (7, vec![1])],
    ] {
        assert!(Catalog::new(entries, &content).is_err());
    }
    for (scene, selector) in [(0, 0), (8192, 0), (1, 512)] {
        assert!(
            Enabled {
                scene,
                selector,
                serial: Serial::new(0).unwrap()
            }
            .encode()
            .is_err()
        );
    }
}
