// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::{
    content::{Registration, Registrations, SubLevelNames},
    replication::players,
};
use serde_json::json;

#[test]
fn garage_transactions_replace_only_changed_slots_and_retire_old_acks() {
    let (mut population, mut players, content, layout, items, p) = fixture();
    let one = Inventory {
        slots: Slots::new([Some(1), None, None, None, None]).unwrap(),
        items: items.clone(),
    };
    let first = population
        .spawn(
            &mut players,
            owner(p),
            one.clone(),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let first_ghost = first.bindings[0].vehicle;
    for binding in &first.bindings {
        population
            .acknowledge(&players, owner(p), binding.expected())
            .unwrap();
    }
    let foreign = join(&mut players, 4, 202);
    let foreign_owner = Owner {
        connection: 4,
        persona: 202,
        participant: foreign,
    };
    let foreign_spawn = population
        .spawn(
            &mut players,
            foreign_owner,
            one.clone(),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let mut bought = (*items).clone();
    for item in items.items.values() {
        let mut item = item.clone();
        item.id += 100;
        if item.owner != 0 {
            item.owner += 100;
        }
        for id in item.children.iter_mut().chain(item.defaults.iter_mut()) {
            *id += 100;
        }
        bought.items.insert(item.id, item);
    }
    bought.roots.extend(items.roots.iter().map(|id| id + 100));
    let two = Inventory {
        slots: Slots::new([Some(1), Some(101), None, None, None]).unwrap(),
        items: Arc::new(bought),
    };
    let added = population
        .reconcile(&mut players, owner(p), &one, two.clone(), &content, &layout)
        .unwrap();
    assert!(added.deleted.is_empty());
    assert_eq!(added.bindings.len(), 1);
    assert_eq!(
        players.owned_vehicle(3, 0x100000002, p, 1),
        Some(first_ghost)
    );
    assert!(!population.all_loaded(p));
    let snapshot = players.objects().snapshot();
    let swapped = Inventory {
        slots: Slots::new([Some(101), Some(1), None, None, None]).unwrap(),
        items: two.items.clone(),
    };
    assert!(
        population
            .reconcile(
                &mut players,
                owner(p),
                &two,
                swapped.clone(),
                &content,
                &layout
            )
            .is_err()
    );
    assert_eq!(snapshot, players.objects().snapshot());
    for binding in &added.bindings {
        population
            .acknowledge(&players, owner(p), binding.expected())
            .unwrap();
    }
    let switched = population
        .reconcile(
            &mut players,
            owner(p),
            &two,
            swapped.clone(),
            &content,
            &layout,
        )
        .unwrap();
    assert_eq!(switched.deleted.len(), 2);
    assert_eq!(switched.bindings.len(), 2);
    for binding in first.bindings.iter().chain(&added.bindings) {
        assert_eq!(
            population.acknowledge(&players, owner(p), binding.expected()),
            Ok(false)
        );
    }
    assert!(!population.all_loaded(p));
    for binding in &switched.bindings {
        population
            .acknowledge(&players, owner(p), binding.expected())
            .unwrap();
    }
    let sold = Inventory {
        slots: Slots::new([Some(101), None, None, None, None]).unwrap(),
        items: Arc::new(Collection {
            roots: two
                .items
                .roots
                .iter()
                .copied()
                .filter(|id| *id >= 100)
                .collect(),
            items: two
                .items
                .items
                .iter()
                .filter(|(id, _)| **id >= 100)
                .map(|(&id, item)| (id, item.clone()))
                .collect(),
        }),
    };
    let sale = population
        .reconcile(&mut players, owner(p), &swapped, sold, &content, &layout)
        .unwrap();
    assert_eq!(sale.deleted.len(), 1);
    assert!(sale.bindings.is_empty());
    assert!(population.all_loaded(p));
    assert_eq!(population.groups.len(), 2);
    assert!(
        players
            .objects()
            .get(foreign_spawn.bindings[0].vehicle)
            .is_some()
    );
    assert_eq!(population.spawned[&foreign].items, one.items);
    assert_eq!(
        population.acknowledge(&players, owner(p), foreign_spawn.bindings[0].expected()),
        Err(Error::UnknownObject)
    );
}

#[test]
fn purchased_inventory_refresh_keeps_other_players_and_allows_garage_return() {
    let (mut population, mut players, content, layout, items, participant) = fixture();
    let layout = world_layout(&layout);
    let mut initial = (*items).clone();
    let mut grant = initial.items[&2].clone();
    grant.id = 20;
    grant.owner = 0;
    grant.state = nfs_world::items::PURCHASABLE;
    initial.items.insert(20, grant);
    initial.roots.push(20);
    let original = inventory(Arc::new(initial));
    let first = population
        .spawn(
            &mut players,
            owner(participant),
            original.clone(),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    for binding in first.bindings {
        population
            .acknowledge(&players, owner(participant), binding.expected())
            .unwrap();
    }
    let other = join(&mut players, 4, 202);
    let other_owner = || Owner {
        connection: 4,
        persona: 202,
        participant: other,
    };
    population
        .spawn(
            &mut players,
            other_owner(),
            original.clone(),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let mut bought = (*original.items).clone();
    let car_owner = bought.items[&2].owner;
    let promoted = bought.items.get_mut(&20).unwrap();
    promoted.owner = car_owner;
    promoted.state = nfs_world::items::OWNED;
    let changed = Inventory {
        slots: original.slots,
        items: Arc::new(bought),
    };
    population
        .refresh_inventory(&players, 3, 0x100000002, &original, &changed)
        .unwrap();
    assert_eq!(population.spawned[&other].items, original.items);
    assert_eq!(
        population.refresh_inventory(&players, 3, 0x100000002, &original, &changed),
        Err(Error::Shape)
    );
    assert_eq!(population.spawned[&participant].items, changed.items);
    population
        .spawn_world(
            &mut players,
            owner(participant),
            &changed,
            &content,
            &layout,
        )
        .unwrap();
    let returning = population
        .prepare_return(&mut players, owner(participant), changed, &content, &layout)
        .unwrap();
    assert_eq!(returning.bindings.len(), 1);
}

#[test]
fn repeated_return_prepares_a_second_lifetime_and_swaps_only_after_detach() {
    let (mut population, mut players, content, layout, items, p) = fixture();
    let layout = world_layout(&layout);
    let inventory = inventory(items);
    players.enable_player_manager().unwrap();
    let initial = population
        .spawn(
            &mut players,
            owner(p),
            inventory.clone(),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    for b in &initial.bindings {
        population
            .acknowledge(&players, owner(p), b.expected())
            .unwrap();
    }
    let stable_count = players.objects().len();
    for _ in 0..3 {
        let world = population
            .spawn_world(&mut players, owner(p), &inventory, &content, &layout)
            .unwrap();
        let car = world.records[0].id;
        players
            .bind_player_vehicle(3, 0x100000002, p, Some(car))
            .unwrap();
        let prepared = population
            .prepare_return(&mut players, owner(p), inventory.clone(), &content, &layout)
            .unwrap();
        assert_eq!(prepared.bindings.len(), 1);
        let binding = prepared.bindings[0];
        let garage = binding.vehicle;
        assert_ne!(car, garage);
        assert_eq!(players.owned_vehicle(3, 0x100000002, p, 1), Some(car));
        assert_eq!(players.staged_vehicle(3, 0x100000002, p, 1), Some(garage));
        assert!(
            population
                .signal_owner(&players, 3, 0x100000002, garage)
                .is_some()
        );
        assert_eq!(players.objects().len(), stable_count + 1);
        assert!(!population.all_loaded(p));
        assert!(
            population
                .prepare_return(&mut players, owner(p), inventory.clone(), &content, &layout)
                .unwrap()
                .records
                .is_empty()
        );
        let before = players.objects().snapshot();
        assert!(
            population
                .finish_return(&mut players, owner(p), &inventory, &layout)
                .is_err()
        );
        assert_eq!(players.objects().snapshot(), before);
        population
            .acknowledge(&players, owner(p), binding.expected())
            .unwrap();
        players
            .bind_player_vehicle(3, 0x100000002, p, None)
            .unwrap();
        assert_eq!(
            population
                .finish_return(&mut players, owner(p), &inventory, &layout)
                .unwrap(),
            car
        );
        assert!(players.objects().get(car).is_none());
        assert_eq!(players.owned_vehicle(3, 0x100000002, p, 1), Some(garage));
        assert!(players.staged_vehicle(3, 0x100000002, p, 1).is_none());
        assert!(population.all_loaded(p));
        assert_eq!(players.objects().len(), stable_count);
        assert_eq!(population.groups.len(), initial.bindings.len());
    }
}

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
        "blueprint":"garage/test","item_blueprint":"test/gameplay","garage_component_index":6,"main":slots}),
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
pub(crate) fn inventory(items: Arc<Collection>) -> Inventory {
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

pub(crate) fn world_layout(layout: &Layout) -> Layout {
    exit_layout(layout, "world_spawn")
}
fn exit_layout(layout: &Layout, pose_key: &str) -> Layout {
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
        pose_key:{"transform_bits":spawn}}),
    )
    .unwrap();
    assert_eq!(world.scene_key, layout.scene_key);
    assert_eq!(
        world
            .world_spawn()
            .or_else(|| world.exit_teleport())
            .unwrap()
            .locator(),
        [959.5, -26.75, -506.5]
    );
    world
}

#[test]
fn installed_exit_destination_swaps_at_the_owned_garage_pose_without_a_recorded_spawn() {
    let (mut population, mut players, content, layout, items, p) = fixture();
    let layout = exit_layout(&layout, "exit_teleport");
    assert!(layout.world_spawn().is_none());
    let inventory = inventory(items);
    let garage = population
        .spawn(
            &mut players,
            owner(p),
            inventory.clone(),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let garage_car = garage
        .records
        .iter()
        .find(|r| matches!(r.initial, Some(Initial::Vehicle { .. })))
        .unwrap();
    let world = population
        .spawn_world(&mut players, owner(p), &inventory, &content, &layout)
        .unwrap();
    assert_eq!(world.deleted, [garage_car.id]);
    assert!(world.messages.is_empty());
    let root = |record: &nfs_world::replication::Record| {
        let Some(Initial::Vehicle { creation, .. }) = &record.initial else {
            panic!("vehicle")
        };
        let Some(vehicle::Initial::Root(root)) = creation.fields.first() else {
            panic!("root")
        };
        root.position.clone()
    };
    assert_eq!(root(&world.records[0]), root(garage_car));
    assert_eq!(
        players.owned_vehicle(3, 0x100000002, p, 1),
        Some(world.records[0].id)
    );
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

#[test]
fn client_physics_handoff_is_owned_ordered_bounded_and_preserves_other_components() {
    use nfs_world::{
        frame::{Movement, MovementRecord},
        replication::{Update, vehicle::movement::Sample},
        vehicle_reports::Reports,
    };
    let (mut population, mut players, content, layout, items, p) = fixture();
    let layout = world_layout(&layout);
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
    let car = population
        .spawn_world(&mut players, owner(p), &inventory(items), &content, &layout)
        .unwrap()
        .records[0]
        .id;
    let cars = [car].into_iter().collect();
    let original = players.objects().get(car).unwrap().clone();
    let mut sample = Sample {
        physics: players.vehicle_physics(3, 0x100000002, car).unwrap(),
        flag: true,
    };
    sample.physics.identity_words = [(5f64.to_bits() >> 32) as u32, 0, 150];
    sample.physics.controls.value3 = 0;
    sample.physics.controls.value6 = 0;
    let (header, body) = sample.encode().unwrap();
    let movement = Movement {
        header: Some(header.span()),
        records: vec![MovementRecord {
            id: car,
            component: 0,
            data: body.span(),
        }],
    };
    let mut reports = Reports::default();
    for (c, p) in [(4, 0x100000002), (3, 1)] {
        assert_eq!(
            reports.receive(&mut players, c, p, &movement, &cars, 5000),
            Err(Error::UnknownObject)
        );
        assert_eq!(players.objects().get(car), Some(&original));
    }
    let out = reports
        .receive(&mut players, 3, 0x100000002, &movement, &cars, 5000)
        .unwrap();
    assert_eq!(out.len(), 1);
    let handoff = players.vehicle_physics(3, 0x100000002, car).unwrap();
    assert_eq!(handoff.identity_words, sample.physics.identity_words);
    assert_eq!(handoff.controls.value3, 3);
    assert!(
        reports
            .receive(&mut players, 3, 0x100000002, &movement, &cars, 5100)
            .unwrap()
            .is_empty()
    );
    sample.physics.identity_words[2] = 151;
    sample.physics.controls.value3 = 3;
    sample.physics.controls.value6 = 61;
    sample.physics.position = vehicle::Vector::from_position([30., 40., 50.], [0.; 3], 7).unwrap();
    let (header, body) = sample.encode().unwrap();
    let movement = Movement {
        header: Some(header.span()),
        records: vec![MovementRecord {
            id: car,
            component: 0,
            data: body.span(),
        }],
    };
    assert_eq!(
        reports
            .receive(&mut players, 3, 0x100000002, &movement, &cars, 5100)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        players.vehicle_physics(3, 0x100000002, car).unwrap(),
        sample.physics
    );
    let updated = players.objects().get(car).unwrap();
    assert_eq!(updated.initial, original.initial);
    let (Update::Vehicle { fields: before, .. }, Update::Vehicle { fields: after, .. }) =
        (&original.current, &updated.current)
    else {
        panic!()
    };
    for (a, b) in before.iter().zip(after) {
        if !matches!(a, Some(vehicle::Update::Chassis(_))) {
            assert_eq!(a, b);
        }
    }
    let saved = players.objects().snapshot();
    let mut bad = movement.clone();
    bad.records.push(MovementRecord {
        id: car,
        component: 0,
        data: body.span(),
    });
    assert!(
        reports
            .receive(&mut players, 3, 0x100000002, &bad, &cars, 5200)
            .is_err()
    );
    assert_eq!(players.objects().snapshot(), saved);
    assert!(
        reports
            .receive(&mut players, 3, 0x100000002, &movement, &cars, 5099)
            .is_err()
    );
    // A disconnected/replaced car no longer has authority, and retained report
    // bookkeeping is retired without accepting a late old-object update.
    players.remove_vehicle(3, 0x100000002, p, 1).unwrap();
    assert!(
        reports
            .receive(
                &mut players,
                3,
                0x100000002,
                &movement,
                &Default::default(),
                5200
            )
            .unwrap()
            .is_empty()
    );
}

#[test]
fn manager_and_participant_bind_the_owned_car_and_clear_repeatedly() {
    use nfs_world::replication::{Update, player_manager::Entry};
    let (mut population, mut players, content, layout, items, p) = fixture();
    let manager = players.enable_player_manager().unwrap().unwrap();
    assert!(players.enable_player_manager().unwrap().is_none());
    assert!(players.sync_player_manager().unwrap().is_none());
    let layout = world_layout(&layout);
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
    let car = population
        .spawn_world(
            &mut players,
            owner(p),
            &inventory(items.clone()),
            &content,
            &layout,
        )
        .unwrap()
        .records[0]
        .id;
    let before = players.objects().snapshot();
    for (connection, persona, participant, vehicle) in [
        (4, 0x100000002, p, car),
        (3, 1, p, car),
        (3, 0x100000002, p, manager.id),
    ] {
        assert!(
            players
                .bind_player_vehicle(connection, persona, participant, Some(vehicle))
                .is_err()
        );
        assert_eq!(players.objects().snapshot(), before);
    }
    let binding = players
        .bind_player_vehicle(3, 0x100000002, p, Some(car))
        .unwrap();
    assert_eq!(binding.len(), 2);
    let participant_binding = &binding[1];
    assert_eq!(participant_binding.id, p);
    assert!(matches!(&participant_binding.update, Update::Participant(v) if v.entity == Some(car)));
    for record in &binding {
        let wire = record.encode().unwrap();
        assert_eq!(
            nfs_world::replication::Record::decode(wire.span(), Some(record.update.kind()))
                .unwrap()
                .record,
            *record
        );
    }
    let bound = players.objects().snapshot();
    assert_eq!(
        players.vehicle_binding_snapshot(3, 0x100000002, p).unwrap(),
        binding
    );
    for (connection, persona, participant) in
        [(4, 0x100000002, p), (3, 1, p), (3, 0x100000002, car)]
    {
        assert!(
            players
                .vehicle_binding_snapshot(connection, persona, participant)
                .is_err()
        );
    }
    assert_eq!(players.objects().snapshot(), bound);
    assert!(players.remove_vehicle(3, 0x100000002, p, 1).is_err());
    assert_eq!(players.objects().snapshot(), bound);
    let binding = binding[0].clone();
    assert_eq!(binding.id, manager.id);
    let Update::PlayerManager(delta) = binding.update else {
        panic!()
    };
    let entry = delta.entries[2].as_ref().unwrap().as_ref().unwrap();
    assert_eq!(*entry, Entry::driving(entry.player, car));
    assert!(
        players
            .bind_player_vehicle(3, 0x100000002, p, Some(car))
            .unwrap()
            .is_empty()
    );
    assert!(players.sync_player_manager().unwrap().is_none());
    let other = join(&mut players, 7, 77);
    players.sync_player_manager().unwrap().unwrap();
    assert!(
        players
            .bind_player_vehicle(7, 77, other, Some(car))
            .is_err()
    );
    let Update::PlayerManager(current) = &players.objects().get(manager.id).unwrap().current else {
        panic!()
    };
    assert_eq!(current.entries[2], delta.entries[2]);
    let isolated = players.vehicle_binding_snapshot(3, 0x100000002, p).unwrap();
    let Update::PlayerManager(isolated) = &isolated[0].update else {
        panic!()
    };
    assert_eq!(isolated.entries[2], delta.entries[2]);
    assert_eq!(
        isolated
            .entries
            .iter()
            .filter(|entry| entry.is_some())
            .count(),
        1
    );
    assert!(
        players
            .vehicle_binding_snapshot(3, 0x100000002, other)
            .is_err()
    );
    assert!(
        current.entries[6]
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .detached
    );
    assert!(
        players
            .bind_player_vehicle(3, 0x100000002, p, None)
            .unwrap()
            .len()
            == 2
    );
    assert!(
        players
            .bind_player_vehicle(3, 0x100000002, p, None)
            .unwrap()
            .is_empty()
    );
    assert!(
        matches!(&players.objects().get(p).unwrap().current, Update::Participant(v) if v.entity == Some(0))
    );
    assert_eq!(players.remove_vehicle(3, 0x100000002, p, 1).unwrap(), car);
    assert!(
        players
            .bind_player_vehicle(3, 0x100000002, p, Some(car))
            .is_err()
    );
    let detached = players.vehicle_binding_snapshot(3, 0x100000002, p).unwrap();
    assert!(matches!(&detached[1].update, Update::Participant(v) if v.entity == Some(0)));
    let Update::PlayerManager(detached) = &detached[0].update else {
        panic!()
    };
    assert!(
        detached.entries[2]
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .detached
    );
    let mut separate = Players::default();
    assert_eq!(separate.enable_player_manager().unwrap().unwrap().id, 1);
    let Update::PlayerManager(empty) = &separate.objects().get(1).unwrap().current else {
        panic!()
    };
    assert!(empty.entries.iter().all(|s| *s == Some(None)));
}

#[test]
fn exit_handoff_keeps_car_relations_through_teleport_rollout_and_completion() {
    use nfs_world::{
        arrival::{ARRIVED, Arrivals},
        logic::{EntityRef, Message as Logic},
        replication::{
            Update,
            entity::creation::{Asset, Catalog, Content as EntityContent},
            spline,
        },
        sequences::{
            Completed, Component, Definition, Layout as SequenceLayout, Owner as SequenceOwner,
        },
    };
    let (mut population, mut players, content, layout, items, p) = fixture();
    let manager = players.enable_player_manager().unwrap().unwrap().id;
    let layout = world_layout(&layout);
    let _garage = population
        .spawn(
            &mut players,
            owner(p),
            inventory(items.clone()),
            &content,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let garage_car = players.owned_vehicle(3, 0x100000002, p, 1).unwrap();
    let world = population
        .spawn_world(&mut players, owner(p), &inventory(items), &content, &layout)
        .unwrap();
    let car = world.records[0].id;
    assert_ne!(car, garage_car);
    assert!(world.deleted.contains(&garage_car));
    assert!(players.objects().get(garage_car).is_none());
    let actor = players.owned_actor(3, 0x100000002, p).unwrap();
    let departing = players.actor_departing(3, 0x100000002, p).unwrap().unwrap();
    assert_eq!(departing.id, actor);
    assert!(matches!(&departing.update, Update::Actor(a)
        if a.flag_b == Some(true) && a.references_a.as_deref() == Some(&[])
        && a.references_b.as_deref() == Some(&[])));
    let mut binding = players
        .bind_player_vehicle(3, 0x100000002, p, Some(car))
        .unwrap();
    assert_eq!(
        binding.iter().map(|r| r.id).collect::<Vec<_>>(),
        [manager, p]
    );
    binding.extend(players.actor_vehicle_ready(3, 0x100000002, p).unwrap());
    assert!(matches!(&binding.last().unwrap().update, Update::Actor(a) if a.flag_b == Some(false)));
    let sections = nfs_world::replication::section::split_timed_records(
        binding,
        nfs_world::application::OUTBOUND_FRAME_BITS,
    )
    .unwrap();
    assert!(sections.iter().all(|s| s.frame().is_ok()));
    let relations = |players: &Players| {
        assert!(
            matches!(&players.objects().get(p).unwrap().current, Update::Participant(v) if v.entity == Some(car))
        );
        let Update::PlayerManager(m) = &players.objects().get(manager).unwrap().current else {
            panic!()
        };
        let entry = m.entries[2].as_ref().unwrap().as_ref().unwrap();
        assert_eq!(entry.primary.as_ref().unwrap().controllable, car);
        assert_eq!(entry.secondary.as_ref().unwrap().controllable, car);
    };
    relations(&players);
    // The teleport envelope and all later actor operations refer to this same car/participant.
    let teleport = nfs_world::teleport::Command {
        endpoint: nfs_world::participants::Endpoint {
            scene: players.objects().scene(layout.item_scene_key).unwrap(),
            selector: 12,
            serial: Serial::new(0).unwrap(),
        },
        participant: p,
        vehicle: car,
        position: [12., 3., 40.],
        basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
    }
    .encode()
    .unwrap();
    let envelope = nfs_protocol::world::rpc::Envelope::decode(
        nfs_protocol::world::BitSpan::new(&teleport, 0, teleport.len() * 8).unwrap(),
        nfs_protocol::world::rpc::Limits {
            max_input_bits: 4096,
            max_references: 32,
            max_payload_bytes: 256,
        },
    )
    .unwrap();
    assert_eq!(&envelope.references()[1..], &[p, car]);
    let asset = Asset {
        bundle: 24,
        type_id: 3424,
        local_index: 1,
    };
    let definition = Definition {
        presentation: None,
        layout: SequenceLayout::GarageExit,
        asset,
        manager: Component {
            scene_key: layout.item_scene_key,
            index: 3,
        },
        embedded_bus: 2,
        channel: Component {
            scene_key: layout.scene_key,
            index: 1,
        },
    };
    let entities = EntityContent::new(
        vec![Catalog::new(24, &[(3424, 2)]).unwrap()],
        vec![(asset, nfs_world::arrival::profile())],
    )
    .unwrap();
    let mut arrivals = Arrivals::new(definition, entities)
        .unwrap()
        .with_spline(spline::Definition {
            asset: Asset {
                bundle: 24,
                type_id: 5100,
                local_index: 2,
            },
            catalog: Catalog::new(24, &[(5100, 3)]).unwrap(),
            flag: false,
        })
        .unwrap();
    let owner = SequenceOwner {
        connection: 3,
        persona: 0x100000002,
        participant: p,
    };
    let rollout = arrivals.start_rollout(&mut players, owner).unwrap();
    assert_eq!(rollout.len(), 3);
    let (goal, actor, sequence) = (rollout[0].id, rollout[1].id, rollout[2].id);
    assert!(
        matches!(&rollout[1].update, Update::Actor(a) if a.references_a.as_deref() == Some(&[goal]))
    );
    assert!(players.actor_arriving(3, 0x100000002, p).unwrap().is_some());
    assert!(
        arrivals
            .start_rollout(&mut players, owner)
            .unwrap()
            .is_empty()
    );
    relations(&players);
    assert!(arrivals.poll(6_000).unwrap().is_empty());
    let stop = arrivals
        .arrived(&Logic::Reached {
            event: ARRIVED,
            target: EntityRef {
                ghost: sequence,
                entity: 1,
            },
            player: 2,
        })
        .unwrap()
        .unwrap();
    assert_eq!(stop.sequence, sequence);
    assert!(
        arrivals
            .complete(
                &players,
                owner,
                Completed {
                    sequence,
                    participant: p
                }
            )
            .unwrap()
            .is_some()
    );
    let (cleared, deleted) = arrivals.finish_rollout(&mut players, owner).unwrap();
    assert_eq!(deleted, [sequence, goal]);
    assert_eq!(cleared[0].id, actor);
    assert!(
        matches!(&cleared[0].update, Update::Actor(a) if a.flag_a == Some(false) && a.references_a.as_deref() == Some(&[]) && a.references_b.as_deref() == Some(&[]))
    );
    relations(&players);
    assert!(players.objects().get(car).is_some());
    assert!(arrivals.poll(60_000).unwrap().is_empty());
    assert_eq!(
        arrivals.finish_rollout(&mut players, owner).unwrap(),
        (vec![], vec![])
    );
}
