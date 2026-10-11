// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::{
    content::{Registration, Registrations, SubLevelNames},
    replication::players,
};
use serde_json::json;

pub(crate) fn fixture() -> (Population, Players, Content, Layout, Arc<Collection>, u16) {
    let (value, classes, items) = crate::vehicle_content::tests::fixture();
    let content = Content::from_json(&value, &classes).unwrap();
    let slots = (0..5).map(|i| {
        let mut bits=[0u32;16];
        for j in [0,5,10] {bits[j]=1f32.to_bits()}
        bits[12]=(i as f32*8.).to_bits();
        json!({"ordinal":i,"component_index":i+1,"item_component_index":i+8,"transform_bits":bits})
    }).collect::<Vec<_>>();
    let layout = Layout::from_json(
        &json!({"format":"nfs-garage-population","version":1,"build_sha256":crate::content::BUILD,
        "blueprint":"garage/test","item_blueprint":"test/gameplay","main":slots}),
    )
    .unwrap();
    let messages = [
        Message::Names(SubLevelNames {
            word: 1,
            entries: vec![(7, b"wheel/test".to_vec())],
        }),
        Message::Registrations(Registrations {
            header: 6,
            entries: vec![Registration {
                handle: 7,
                next: 24,
                region: 0,
                word: 53,
                byte: 0,
                flag: false,
                enum3: 3,
                word2: 0,
                bit: true,
                enum2: 1,
                word3: 0,
                text: vec![],
            }],
        }),
    ];
    let population = Population::new(&messages).unwrap();
    let mut players = Players::default();
    let scene_content = sublevel::Content::new(vec![
        (
            layout.item_scene_key,
            sublevel::gameplay::profile().unwrap(),
        ),
        (
            layout.scene_key,
            sublevel::Profile::new(
                false,
                &[
                    sublevel::Kind::Noop,
                    sublevel::Kind::Rpc,
                    sublevel::Kind::Rpc,
                    sublevel::Kind::Rpc,
                    sublevel::Kind::Rpc,
                    sublevel::Kind::Rpc,
                ],
            )
            .unwrap(),
        ),
    ])
    .unwrap();
    let scenes = [layout.item_scene_key, layout.scene_key]
        .into_iter()
        .enumerate()
        .map(|(i, key)| {
            sublevel::ordinary::creation(
                i as u16 + 1,
                key,
                None,
                Serial::new(0).unwrap(),
                &sublevel::ordinary::unpopulated(scene_content.profile(key).unwrap()).unwrap(),
                &scene_content,
            )
            .unwrap()
        })
        .collect();
    players
        .create_scenes(&[1, 2, 24], scenes, &scene_content)
        .unwrap();
    let participant = join(&mut players, 3, 0x100000002);
    (
        population,
        players,
        content,
        layout,
        Arc::new(items),
        participant,
    )
}
fn join(players: &mut Players, connection: u8, persona: u64) -> u16 {
    let player = players
        .create(
            connection,
            persona,
            players::Request {
                name: b"Driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players
        .join(connection, persona, player)
        .unwrap()
        .unwrap()
        .id;
    players
        .create_actor(connection, persona, participant)
        .unwrap();
    participant
}
fn owner(participant: u16) -> Owner {
    Owner {
        connection: 3,
        persona: 0x100000002,
        participant,
    }
}
fn inventory(items: Arc<Collection>) -> Inventory {
    Inventory {
        slots: Slots::new([None, None, Some(1), None, None]).unwrap(),
        items,
    }
}

#[test]
fn current_inventory_creates_owned_scene_vehicle_and_matching_offer_once() {
    let (mut population, mut players, content, layout, items, p) = fixture();
    let before = players.objects().len();
    let output = population
        .spawn(
            &mut players,
            owner(p),
            inventory(items.clone()),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    assert_eq!(output.records.len(), 2);
    assert_eq!(players.objects().len(), before + 2);
    assert_eq!(output.messages.len(), 2);
    assert_eq!(output.bindings.len(), 1);
    let binding = output.bindings[0];
    assert_eq!(binding.vehicle, output.records[1].id);
    assert_eq!(binding.endpoint.selector, 2);
    assert_eq!(
        players.owned_vehicle(3, 0x100000002, p, 1),
        Some(binding.vehicle)
    );
    assert_eq!(players.owned_vehicle(7, 0x100000002, p, 1), None);
    let Some(Initial::Vehicle { prefix, creation }) = &output.records[1].initial else {
        panic!("vehicle")
    };
    assert_eq!(prefix.asset.bundle, 51);
    assert_eq!(
        prefix.parent.as_ref().unwrap().reference,
        Some(output.records[0].id)
    );
    assert_eq!(creation.connection_id, 3);
    let nfs_world::replication::Update::Vehicle { fields, .. } = &output.records[1].update else {
        panic!("vehicle update")
    };
    let appearance = fields
        .iter()
        .flatten()
        .find_map(|u| match u {
            vehicle::Update::Appearance(v) => Some(v.as_ref()),
            _ => None,
        })
        .unwrap();
    assert_eq!(appearance.paint.as_ref().unwrap().resource.words, [2, 1]);
    assert!(!population.all_loaded(p));
    assert!(
        population
            .acknowledge(&players, owner(p), binding.expected())
            .unwrap()
    );
    assert!(
        !population
            .acknowledge(&players, owner(p), binding.expected())
            .unwrap()
    );
    assert!(population.all_loaded(p));
    let repeated = population
        .spawn(
            &mut players,
            owner(p),
            inventory(items),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    assert!(
        repeated.records.is_empty() && repeated.messages.is_empty() && repeated.bindings.is_empty()
    );
}

#[test]
fn late_missing_endpoint_rolls_back_world_ids_authority_and_registration() {
    let (mut population, mut players, content, mut layout, items, p) = fixture();
    let before = players.objects().snapshot();
    let registry = population.registrations.clone();
    let authority = population.authority.clone();
    let key = layout.scene_key;
    layout.scene_key = 0;
    assert!(
        population
            .spawn(
                &mut players,
                owner(p),
                inventory(items.clone()),
                &content,
                &layout,
                [0.; 3]
            )
            .is_err()
    );
    assert_eq!(players.objects().snapshot(), before);
    assert_eq!(population.registrations, registry);
    assert_eq!(population.authority, authority);
    assert_eq!(population.next_group, 1);
    assert!(population.spawned.is_empty());
    layout.scene_key = key;
    assert_eq!(
        population
            .spawn(
                &mut players,
                owner(p),
                inventory(items),
                &content,
                &layout,
                [0.; 3]
            )
            .unwrap()
            .records[0]
            .id,
        before.len() as u16 + 1
    );
}

#[test]
fn foreign_repeats_stale_ack_and_changed_inventory_do_not_mutate_the_owner() {
    let (mut population, mut players, content, layout, items, p) = fixture();
    let other = join(&mut players, 7, 99);
    let output = population
        .spawn(
            &mut players,
            owner(p),
            inventory(items.clone()),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let before = players.objects().snapshot();
    assert!(
        population
            .spawn(
                &mut players,
                Owner {
                    connection: 7,
                    persona: 99,
                    participant: p
                },
                inventory(items.clone()),
                &content,
                &layout,
                [0.; 3]
            )
            .is_err()
    );
    let mut ack = output.bindings[0].expected();
    ack.vehicle -= 1;
    assert!(population.acknowledge(&players, owner(p), ack).is_err());
    assert!(
        population
            .acknowledge(&players, owner(other), output.bindings[0].expected())
            .is_err()
    );
    let mut changed = (*items).clone();
    changed.items.get_mut(&1).unwrap().sell_price += 1;
    assert!(
        population
            .spawn(
                &mut players,
                owner(p),
                inventory(Arc::new(changed)),
                &content,
                &layout,
                [0.; 3]
            )
            .is_err()
    );
    assert_eq!(players.objects().snapshot(), before);
    let second = population
        .spawn(
            &mut players,
            Owner {
                connection: 7,
                persona: 99,
                participant: other,
            },
            inventory(items),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    assert_eq!(second.records.len(), 1);
    assert!(second.messages.is_empty());
    assert_ne!(second.bindings[0].vehicle, output.bindings[0].vehicle);
    assert!(!population.all_loaded(other));
}

fn world_layout(layout: &Layout) -> Layout {
    let slots = (0..5).map(|i| {
        let mut bits=[0u32;16];
        for j in [0,5,10] {bits[j]=1f32.to_bits()}
        bits[12]=(i as f32*8.).to_bits();
        json!({"ordinal":i,"component_index":i+1,"item_component_index":i+8,"transform_bits":bits})
    }).collect::<Vec<_>>();
    let mut spawn = [0u32; 16];
    for j in [0, 5, 10] {
        spawn[j] = 1f32.to_bits();
    }
    spawn[12] = 959.5f32.to_bits();
    spawn[13] = (-26.75f32).to_bits();
    spawn[14] = (-506.5f32).to_bits();
    let world = Layout::from_json(
        &json!({"format":"nfs-garage-population","version":1,"build_sha256":crate::content::BUILD,
        "blueprint":"garage/test","item_blueprint":"test/gameplay","main":slots,
        "world_spawn":{"transform_bits":spawn}}),
    )
    .unwrap();
    assert_eq!(world.scene_key, layout.scene_key);
    assert_eq!(
        world.world_spawn().unwrap().locator(),
        [959.5, -26.75, -506.5]
    );
    world
}

#[test]
fn garage_exit_swaps_the_garage_car_for_a_driveable_world_car() {
    let (mut population, mut players, content, layout, items, p) = fixture();
    let layout = world_layout(&layout);
    let garage = population
        .spawn(
            &mut players,
            owner(p),
            inventory(items.clone()),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let (scene, garage_car) = (garage.records[0].id, garage.records[1].id);
    // The client simulates its garage car, then its world car (E101 grants).
    assert_eq!(
        players.owned_vehicles(3, 0x100000002),
        [garage_car].into_iter().collect()
    );
    assert!(players.owned_vehicles(7, 0x100000002).is_empty());
    let objects = players.objects().len();
    let world = population
        .spawn_world(
            &mut players,
            owner(p),
            &inventory(items.clone()),
            &content,
            &layout,
        )
        .unwrap();
    assert_eq!(world.deleted, vec![garage_car]);
    assert!(world.messages.is_empty());
    assert_eq!(world.records.len(), 1);
    assert_eq!(players.objects().len(), objects);
    let car = &world.records[0];
    assert_ne!(car.id, garage_car);
    assert_eq!(players.owned_vehicle(3, 0x100000002, p, 1), Some(car.id));
    assert_eq!(
        players.owned_vehicles(3, 0x100000002),
        [car.id].into_iter().collect()
    );
    let (
        Some(Initial::Vehicle { prefix, creation }),
        Some(Initial::Vehicle {
            prefix: garage_prefix,
            creation: garage_creation,
        }),
    ) = (&car.initial, &garage.records[1].initial)
    else {
        panic!("vehicles")
    };
    assert_eq!(prefix.sub_id, WORLD_SUB_ID);
    assert_eq!(garage_prefix.sub_id, 1);
    assert_eq!(prefix.parent.as_ref().unwrap().reference, Some(scene));
    assert_eq!(prefix.blueprint, garage_prefix.blueprint);
    assert_eq!(prefix.asset, garage_prefix.asset);
    assert_eq!(creation.connection_id, garage_creation.connection_id);
    let (Some(vehicle::Initial::Root(root)), Some(vehicle::Initial::Root(garage_root))) =
        (creation.fields.first(), garage_creation.fields.first())
    else {
        panic!("roots")
    };
    assert!(!root.fine_flag && garage_root.fine_flag);
    assert_eq!(
        root.position,
        vehicle::Vector::from_position([959.5, -26.75, -506.5], [0.; 3], 5).unwrap()
    );
    let nfs_world::replication::Update::Vehicle { fields, .. } = &car.update else {
        panic!("vehicle update")
    };
    let controls = fields
        .iter()
        .flatten()
        .find_map(|u| match u {
            vehicle::Update::Chassis(c) => c.physics.as_ref().map(|p| p.controls.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!((controls.value3, controls.value6), (3, 63));
    assert!(controls.flag && controls.wheel_pairs.is_some());
}

#[test]
fn world_spawn_requires_configuration_a_garage_car_and_the_owner() {
    let (mut population, mut players, content, layout, items, p) = fixture();
    let before = (population.clone(), players.clone());
    assert_eq!(
        population
            .spawn_world(
                &mut players,
                owner(p),
                &inventory(items.clone()),
                &content,
                &layout
            )
            .err(),
        Some(Error::Unsupported)
    );
    let layout = world_layout(&layout);
    // No garage car yet: nothing to swap, nothing changes.
    assert_eq!(
        population
            .spawn_world(
                &mut players,
                owner(p),
                &inventory(items.clone()),
                &content,
                &layout
            )
            .err(),
        Some(Error::UnknownObject)
    );
    assert_eq!(players.objects().len(), before.1.objects().len());
    population
        .spawn(
            &mut players,
            owner(p),
            inventory(items.clone()),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let saved = players.objects().len();
    let foreign = Owner {
        connection: 7,
        persona: 0x100000002,
        participant: p,
    };
    assert!(
        population
            .spawn_world(&mut players, foreign, &inventory(items), &content, &layout)
            .is_err()
    );
    assert_eq!(players.objects().len(), saved);
}
