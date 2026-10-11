// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    participants,
    replication::{
        Error, Initial, Record, Update,
        sublevel::{self, root},
    },
};

fn scenes(gameplay: u32, startup: u32) -> Vec<Record> {
    let rpc = |selector| sublevel::Rpc {
        selector,
        serial: Serial::new(3).unwrap(),
    };
    let mut game = vec![sublevel::Initial::Empty; 13];
    game[1] = sublevel::Initial::RpcReferences {
        rpc: rpc(1),
        value: vec![],
    };
    for (index, field) in game.iter_mut().enumerate().take(13).skip(8) {
        *field = sublevel::Initial::Rpc(rpc(index as u16));
    }
    let mut start = vec![sublevel::Initial::Empty; 115];
    for index in [6, 11, 7, 8, 25, 21, 18, 19, 114, 95, 90, 89, 86, 87] {
        start[index] = sublevel::Initial::RpcReferences {
            rpc: rpc(index as u16),
            value: vec![],
        };
    }
    [(gameplay, game), (startup, start)]
        .into_iter()
        .enumerate()
        .map(|(i, (key, fields))| {
            let kinds = fields
                .iter()
                .map(|field| match field {
                    sublevel::Initial::Empty => sublevel::Kind::Noop,
                    sublevel::Initial::Rpc(_) => sublevel::Kind::Rpc,
                    sublevel::Initial::RpcReferences { .. } => sublevel::Kind::RpcReferences,
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
            let profile = sublevel::Profile::new(false, &kinds).unwrap();
            let updates = vec![None; fields.len()];
            Record {
                id: 1 + i as u16,
                initial: Some(Initial::SubLevel {
                    prefix: sublevel::Prefix {
                        level_id: 10 + i as u16,
                        content_key: key,
                        blueprint: None,
                        word_a: 0,
                        word_b: 0,
                        constructor_word: None,
                    },
                    fields,
                }),
                update: Update::SubLevel {
                    profile,
                    fields: updates,
                },
            }
        })
        .collect()
}

#[test]
fn role_keys_are_explicit_per_world_and_foreign_or_ambiguous_roles_fail() {
    let records = scenes(101, 102);
    let bindings = participants::Bindings::from_records(&records, 101, 102)
        .unwrap()
        .unwrap();
    assert_eq!((bindings.manager.scene, bindings.inventory.scene), (1, 2));
    assert!(
        participants::Bindings::from_records(&records, 201, 202)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        participants::Bindings::from_records(&records, 201, 102),
        Err(Error::UnknownObject)
    );
    for (a, b) in [(0, 102), (1, 102), (101, 1), (101, 101)] {
        assert_eq!(
            participants::Bindings::from_records(&records, a, b),
            Err(Error::Shape)
        );
    }
    let mut duplicate = records.clone();
    duplicate.push(records[0].clone());
    assert_eq!(
        participants::Bindings::from_records(&duplicate, 101, 102),
        Err(Error::DuplicateObject)
    );
    let mut lifecycle = participants::Lifecycle::default();
    assert_eq!(
        lifecycle.bind(&duplicate, 101, 102),
        Err(Error::DuplicateObject)
    );
    assert!(!lifecycle.available());
    lifecycle.bind(&records, 101, 102).unwrap();
    assert!(lifecycle.available());
}

#[test]
fn traffic_slots_use_configuration_and_reject_reserved_or_duplicate_keys() {
    let keys = [11, 12, 13, 14, 15, 16, 17, 18, 19, 20];
    let content = root::traffic_content(&keys).unwrap();
    assert_eq!(content.profiles().count(), 11);
    for slot in 0..10 {
        let creation = root::traffic_creation(&keys, slot, 100 + slot as u16).unwrap();
        assert_eq!(creation.prefix.content_key, keys[slot]);
        assert_eq!(creation.prefix.level_id, 100 + slot as u16);
    }
    for bad in [0, 1, keys[0]] {
        let mut changed = keys;
        changed[9] = bad;
        assert_eq!(root::traffic_content(&changed), Err(Error::Shape));
    }
    assert_eq!(root::traffic_creation(&keys, 10, 1), Err(Error::Bound));
}
