// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::net::items::{Exchanges, Service};
use crate::vehicle_content::population::{Owner, tests as population_tests};
use nfs_services::{inventory, persistent};
use nfs_storage::{
    AccountId, Batch, GarageSlots, InventoryRepository, ItemId, MemoryRepository, Op, Timestamp,
};
use nfs_world::{
    bits::BitWriter,
    items::{
        Definition, DefinitionClass, DefinitionFlags, Derived, InventoryCatalog, Item, OWNED,
        OwnershipLevel,
    },
};
use serde_json::json;

mod appearance;
mod driving;
mod purchases;

fn tables() -> persistent::Catalog {
    let rows:Vec<_>=persistent::required_tables().iter().map(|&(name,secondary,count)|{
        let columns:Vec<_>=(0..count).map(|i| {
            let garage=name=="GarageItemsTable";
            let column=if garage {["PrimaryVehicleItem","Vehicle1","Vehicle2","Vehicle3","Vehicle4"][i].into()} else {format!("Column{i}")};
            json!({"name":column,"key":persistent::key(&column),"value_type":if garage{"string"}else{"int"},"default":"0"})
        }).collect();
        json!({"name":name,"key":persistent::key(name),"secondary_key":secondary,"using_stores":secondary.is_none(),"write_whole_rows":false,"columns":columns})
    }).collect();
    persistent::Catalog::from_json(&json!({"version":1,"build_sha256":crate::content::BUILD,"source":{"path":"synthetic","sha256":"00".repeat(32)},"tables":rows})).unwrap()
}

// Constructed player/car/part ids and data, independent of any game capture.
fn fixture() -> (
    PlayerListener,
    Exchanges,
    Arc<MemoryRepository>,
    AccountId,
    u16,
    u16,
    u16,
) {
    let (mut population, mut players, _, layout, _, _) = population_tests::fixture();
    let (mut document, classes, mut items) = crate::vehicle_content::tests::fixture();
    let mut definitions: Vec<_> = items
        .items
        .values()
        .map(|i| {
            (
                i.definition,
                Definition {
                    class: classes.class(&i.definition).unwrap(),
                    buy_price: 0,
                    sell_price: 0,
                    quantity: 1,
                    ownership_level: OwnershipLevel::Owned,
                    flags: DefinitionFlags::default(),
                    sub_items: vec![],
                    additional_items: vec![],
                    default_derived: Some(i.derived.clone()),
                },
            )
        })
        .collect();
    for id in [6u8, 7] {
        let definition = [id; 16];
        definitions.push((
            definition,
            Definition {
                class: DefinitionClass::AirFilterItemData,
                buy_price: 30,
                sell_price: 10,
                quantity: 1,
                ownership_level: OwnershipLevel::Owned,
                flags: DefinitionFlags {
                    requires_owner: true,
                    ..Default::default()
                },
                sub_items: vec![],
                additional_items: vec![],
                default_derived: Some(Derived::Empty),
            },
        ));
        items.items.insert(
            u64::from(id),
            Item {
                id: u64::from(id),
                definition,
                owner: 1,
                defaults: vec![],
                children: vec![],
                state: OWNED,
                buy_price: 30,
                sell_price: 10,
                derived: Derived::Empty,
            },
        );
        let guid = format!("{id:02x}").repeat(16);
        document["parts"]["customization"].as_array_mut().unwrap().push(json!({"guid":guid,"class_name":"AirFilterItemData","tuning":{},"chassis":{"family":"performance","performance_value":if id==6{0.0}else{0.25}}}));
        document["parts"]["mesh"]
            .as_array_mut()
            .unwrap()
            .push(json!({"guid":guid,"class_name":"AirFilterItemData","kind":"ignore"}));
    }
    let persistent = Derived::PersistantTuning {
        abs_setting: 5,
        stability_control_setting: 5,
        traction_control_setting: 5,
        air_pressure_front_setting: 5,
        air_pressure_rear_setting: 5,
    };
    definitions.push((
        [8; 16],
        Definition {
            class: DefinitionClass::PersistantTuningItemData,
            default_derived: Some(persistent.clone()),
            ..definitions
                .iter()
                .find(|(guid, _)| *guid == [6; 16])
                .unwrap()
                .1
                .clone()
        },
    ));
    items.items.insert(
        8,
        Item {
            id: 8,
            definition: [8; 16],
            derived: persistent,
            ..items.items[&6].clone()
        },
    );
    document["parts"]["customization"].as_array_mut().unwrap().push(json!({"guid":"08".repeat(16),"class_name":"PersistantTuningItemData",
        "tuning":{"asset_present":true,"transmission_values":[10,0]},"chassis":{"family":"performance","performance_value":0.0,"list_a":0}}));
    document["parts"]["mesh"].as_array_mut().unwrap().push(
        json!({"guid":"08".repeat(16),"class_name":"PersistantTuningItemData","kind":"ignore"}),
    );
    // An owned alternative front rim, with a different mesh bundle and paint.
    let mut rim = items.items[&2].clone();
    rim.id = 9;
    rim.definition = [9; 16];
    let Derived::Rims { primary_paint, .. } = &mut rim.derived else {
        panic!()
    };
    *primary_paint = [0.25f32.to_bits(); 3];
    items.items.insert(9, rim.clone());
    let mut definition = definitions
        .iter()
        .find(|(g, _)| *g == [2; 16])
        .unwrap()
        .1
        .clone();
    definition.default_derived = Some(rim.derived);
    definitions.push(([9; 16], definition));
    for family in ["customization", "wheels", "appearance", "mesh"] {
        let mut entry = document["parts"][family]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["guid"] == "02".repeat(16))
            .unwrap()
            .clone();
        entry["guid"] = json!("09".repeat(16));
        if family == "customization" {
            entry["chassis"]["rim_values"] = json!([0.45, 0.5, 0.55]);
        }
        if family == "mesh" {
            entry["guids"] = json!(["22".repeat(16)]);
        }
        if family == "wheels" {
            entry["diameter"] = json!(0.75);
            entry["sizes"] = json!([0.45, 0.5, 0.55]);
        }
        document["parts"][family]
            .as_array_mut()
            .unwrap()
            .push(entry);
    }
    document["vehicles"][0]["meshes"][0]["variants"]
        .as_array_mut()
        .unwrap()
        .push(json!({"guid":"22".repeat(16),"bundle":"wheel/upgrade"}));
    let mut tire = items.items[&4].clone();
    tire.id = 10;
    tire.definition = [10; 16];
    items.items.insert(10, tire);
    let tire_definition = definitions
        .iter()
        .find(|(g, _)| *g == [4; 16])
        .unwrap()
        .1
        .clone();
    definitions.push(([10; 16], tire_definition));
    for family in ["customization", "wheels", "mesh"] {
        let mut entry = document["parts"][family]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["guid"] == "04".repeat(16))
            .unwrap()
            .clone();
        entry["guid"] = json!("0a".repeat(16));
        document["parts"][family]
            .as_array_mut()
            .unwrap()
            .push(entry);
    }
    let car = items.items.get_mut(&1).unwrap();
    car.children.push(6);
    car.children.push(8);
    car.defaults.push(6);
    car.defaults.push(8);
    car.defaults.extend([2, 3, 4, 5]);
    let livery = Item {
        id: 11,
        definition: [11; 16],
        owner: 0,
        defaults: vec![],
        children: vec![],
        state: OWNED,
        buy_price: 0,
        sell_price: 0,
        derived: Derived::LiveryCustomization {
            byte_vault_data_id: 0,
            library_preset_id: u32::MAX,
            dynamic_flags: 1,
        },
    };
    definitions.push((
        [11; 16],
        Definition {
            class: DefinitionClass::LiveryCustomizationItemData,
            flags: DefinitionFlags::default(),
            default_derived: Some(livery.derived.clone()),
            ..definitions[0].1.clone()
        },
    ));
    items.items.insert(11, livery);
    document["parts"]["customization"]
        .as_array_mut()
        .unwrap()
        .push(json!({
        "guid":"0b".repeat(16),"class_name":"LiveryCustomizationItemData",
        "tuning":{},"chassis":{"family":"other"}}));
    document["parts"]["mesh"]
        .as_array_mut()
        .unwrap()
        .push(json!({
        "guid":"0b".repeat(16),"class_name":"LiveryCustomizationItemData","kind":"ignore"}));
    document["parts"]["appearance"].as_array_mut().unwrap().push(json!({
        "guid":"0b".repeat(16),"class_name":"LiveryCustomizationItemData","data_id":"0","version":-1}));
    items.roots = items.items.keys().copied().collect();
    let catalog = Arc::new(InventoryCatalog::new(definitions, vec![]).unwrap());
    let vehicles =
        crate::vehicle_content::Content::from_json(&document, catalog.bindings()).unwrap();
    let persona = 303;
    let player = players
        .create(
            HOST_SELECTOR as u8,
            persona,
            Request {
                name: b"Upgrade".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players
        .join(HOST_SELECTOR as u8, persona, player)
        .unwrap()
        .unwrap()
        .id;
    players
        .create_actor(HOST_SELECTOR as u8, persona, participant)
        .unwrap();
    let inventory = population_tests::inventory(Arc::new(items));
    let spawned = population
        .spawn(
            &mut players,
            Owner {
                connection: HOST_SELECTOR as u8,
                persona,
                participant,
            },
            inventory.clone(),
            &vehicles,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let car = spawned.bindings[0].vehicle;
    let account = AccountId::from_owned_config([8; 16]).unwrap();
    let repo = Arc::new(MemoryRepository::default());
    repo.open(account).unwrap();
    let mut ops: Vec<_> = inventory::records(&inventory.items, catalog.bindings())
        .unwrap()
        .into_iter()
        .map(Op::Insert)
        .collect();
    ops.push(Op::SetGarage(GarageSlots(
        inventory.slots.values().map(|i| i.and_then(ItemId::new)),
    )));
    ops.push(nfs_services::bank::Balance(70).store());
    repo.apply(
        account,
        &Batch {
            id: 1,
            expected_generation: 0,
            ops,
        },
        Timestamp(1),
    )
    .unwrap();
    let tables = Arc::new(tables());
    let (_, loaded) = tables
        .ensure_loaded(
            repo.as_ref(),
            account,
            repo.snapshot(account).unwrap(),
            Timestamp(2),
        )
        .unwrap();
    let scene = players.objects().scene(layout.scene_key).unwrap();
    let mut listener = PlayerListener::new(persona);
    listener.players = players;
    listener.inventory = Some(inventory);
    listener.persistent = Some(Arc::new(loaded));
    listener.population = Some(population);
    listener.roles = Some(crate::scene_roles::SceneRoles {
        level: b"test".to_vec(),
        gameplay: layout.item_scene_key,
        startup: 20,
        garage: layout.scene_key,
        progression: 21,
        traffic: [0; 10],
        customization_timer: replication::entity::creation::Asset {
            bundle: 1,
            type_id: 1,
            local_index: 0,
        },
        streaming_gate: replication::entity::creation::Asset {
            bundle: 1,
            type_id: 2,
            local_index: 0,
        },
        spawn_points: None,
        free_roam: None,
        bank: None,
    });
    listener.vehicles = Some(Arc::new(GarageContent { vehicles, layout }));
    let offers = nfs_services::purchase::Offers::new([[7; 16], [9; 16], [10; 16]])
        .unwrap()
        .with_vehicle_scopes(&catalog, [([1; 16], vec![[7; 16], [9; 16], [10; 16]])])
        .unwrap()
        .with_part_policy(
            &catalog,
            [
                ([2; 16], 0),
                ([3; 16], 1),
                ([4; 16], 0),
                ([5; 16], 1),
                ([9; 16], 0),
                ([10; 16], 0),
            ],
            [([7; 16], 0), ([9; 16], 0), ([10; 16], 0)],
        )
        .unwrap();
    let profile = Service::new(repo.clone(), catalog)
        .with_persistent(tables)
        .with_purchase_offers(offers)
        .bind(account);
    (
        listener,
        Exchanges::new(Some(profile), 1),
        repo,
        account,
        scene,
        player,
        car,
    )
}

fn wire(scene: u16, player: u16, car: Option<u16>, method: u32, item: u64) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(2, 9).put(u64::from(method), 32);
    if car.is_some() {
        payload.put(item as u32 as u64, 32).put(item >> 32, 32);
    }
    payload.put(0, 7);
    let mut wire = BitWriter::new();
    wire.put(0, 32)
        .put(0, 32)
        .put(if car.is_some() { 3 } else { 2 }, 8)
        .put(u64::from(scene), 13)
        .put(u64::from(player), 13);
    if let Some(car) = car {
        wire.put(u64::from(car), 13);
    }
    wire.put((payload.len() / 8) as u64, 9)
        .put_span(payload.span());
    wire
}
fn accept(listener: &mut PlayerListener, wire: &BitWriter) -> Result<bool, replication::Error> {
    let mut staged = listener.equipment.clone();
    let handled = listener.stage_equipment(&listener.players, wire.span(), &mut staged)?;
    listener.equipment = staged;
    Ok(handled)
}

#[tokio::test]
async fn equip_commit_updates_repository_live_car_and_return_inventory_only_once() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap());
    assert_eq!(repo.snapshot(account).unwrap(), before);
    assert!(listener.equipment.committed.is_empty());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.working());
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    assert_eq!(update.player_index, u32::from(HOST_SELECTOR - 1));
    assert_eq!(update.items.roots, [1]);
    assert!(update.items.items[&1].children.contains(&7));
    assert!(!update.items.items[&1].children.contains(&6));
    let saved = repo.snapshot(account).unwrap();
    assert_eq!(saved.generation, before.generation + 1);
    assert_eq!(saved.tables, before.tables);
    let saved_car = &saved.items[&ItemId::new(1).unwrap()];
    assert!(saved_car.sub_items.contains(&ItemId::new(7).unwrap()));
    assert!(!saved_car.sub_items.contains(&ItemId::new(6).unwrap()));
    assert_eq!(listener.pending.len(), 1);
    assert_eq!(listener.pending[0].records[0].id, car);
    assert!(listener.pending_rpcs.is_empty()); // no fabricated success RPC
    let inventory = listener.inventory.as_ref().unwrap().clone();
    listener
        .population
        .as_mut()
        .unwrap()
        .refresh_inventory(
            &listener.players,
            HOST_SELECTOR as u8,
            listener.persona,
            &inventory,
            &inventory,
        )
        .unwrap();
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(!jobs.working());
    assert_eq!(repo.snapshot(account).unwrap(), saved);
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 5, Timestamp(5));
    assert!(jobs.completed(&mut listener).await);
    assert_eq!(repo.snapshot(account).unwrap(), saved);
    assert_eq!(listener.pending.len(), 1);
}

#[test]
fn invalid_ownership_and_world_car_do_not_stage_or_commit() {
    let (mut listener, _, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    for request in [
        wire(scene, player + 1000, Some(car), 0, 7),
        wire(scene, player, Some(car + 1000), 0, 7),
        wire(scene, player, Some(car), 0, 1),
    ] {
        assert!(accept(&mut listener, &request).is_err());
    }
    listener.world_cars.insert(car, (0, false));
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).is_err());
    assert!(listener.equipment.edits.is_empty());
    assert!(listener.equipment.committed.is_empty());
    assert_eq!(repo.snapshot(account).unwrap(), before);
}

fn dyno_wire(scene: u16, player: u16, car: u16, stats: nfs_world::items::DynoStats) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(2, 9)
        .put(3, 32)
        .put_span(stats.encode().span())
        .align();
    let mut wire = BitWriter::new();
    wire.put(0, 32)
        .put(0, 32)
        .put(3, 8)
        .put(scene.into(), 13)
        .put(player.into(), 13)
        .put(car.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    wire
}

fn handling_wire(scene: u16, player: u16, car: u16, changes: &[(u8, u8)]) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(2, 9).put(8, 32).put(changes.len() as u64, 8);
    for &(category, value) in changes {
        payload.put(category.into(), 8).put(value.into(), 8);
    }
    payload.put(0, 7);
    let mut body = BitWriter::new();
    body.put(0, 64)
        .put(3, 8)
        .put(scene.into(), 13)
        .put(player.into(), 13)
        .put(car.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}

#[tokio::test]
async fn multiple_categories_are_retained_until_one_commit() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    for item in [7, 8, 8] {
        assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, item)).unwrap());
    }
    assert_eq!(listener.equipment.edits[&player].selections, vec![7, 8]);
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    assert!(update.items.items[&1].children.contains(&7));
    assert!(update.items.items[&1].children.contains(&8));
    assert!(!update.items.items[&1].children.contains(&6));
    let after = repo.snapshot(account).unwrap();
    assert_eq!(after.generation, before.generation + 1);
    assert_eq!(after.tables, before.tables);
}

#[tokio::test]
async fn invalid_equipment_presentation_fails_before_any_inventory_write() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    listener.equipment.committed.front_mut().unwrap().owner.name = vec![b'x'; 1025];
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(!jobs.completed(&mut listener).await);
    assert_eq!(repo.snapshot(account).unwrap(), before);
    assert!(listener.pending.is_empty());
    assert!(listener.pending_content.is_empty());
}

#[tokio::test]
async fn visual_rim_edit_refreshes_mesh_and_appearance_without_replacing_the_rear() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    let physics = listener
        .players
        .vehicle_physics(HOST_SELECTOR as u8, listener.persona, car)
        .unwrap();
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 9)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.completed(&mut listener).await);
    let notification = crate::net::items::tests::deliver_notification(&mut jobs);
    assert!(notification.items.items[&1].children.contains(&9));
    assert!(notification.items.items[&1].children.contains(&3));
    assert!(!notification.items.items[&1].children.contains(&2));
    assert!(notification.items.items[&1].defaults.contains(&2));
    assert_eq!(listener.pending_content.len(), 2);
    assert!(
        listener
            .population
            .as_ref()
            .unwrap()
            .registrations()
            .mesh_level("wheel/upgrade")
            .is_ok()
    );
    let replication::Update::Vehicle { fields, .. } = &listener.pending[0].records[0].update else {
        panic!()
    };
    assert!(
        fields
            .iter()
            .any(|f| matches!(f, Some(replication::vehicle::Update::Appearance(_))))
    );
    assert!(
        fields
            .iter()
            .any(|f| matches!(f, Some(replication::vehicle::Update::Mesh(_))))
    );
    assert_eq!(
        listener
            .players
            .vehicle_physics(HOST_SELECTOR as u8, listener.persona, car)
            .unwrap(),
        physics
    );
    let saved = repo.snapshot(account).unwrap();
    assert_eq!(saved.tables, before.tables);
    assert_eq!(saved.generation, before.generation + 1);
}

#[tokio::test]
async fn handling_cancel_and_combined_save_refresh_parts_notification_and_chassis() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    let change = handling_wire(scene, player, car, &[(26, 6), (30, 10)]);
    assert!(
        accept(
            &mut listener,
            &handling_wire(scene, player + 1000, car, &[(26, 6)])
        )
        .is_err()
    );
    assert!(accept(&mut listener, &change).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(!jobs.working());
    assert_eq!(repo.snapshot(account).unwrap(), before);
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap());
    assert!(accept(&mut listener, &change).unwrap());
    let newer = handling_wire(scene, player, car, &[(26, 7)]);
    assert!(accept(&mut listener, &newer).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(jobs.completed(&mut listener).await);
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    let saved = repo.snapshot(account).unwrap();
    assert_eq!(saved.generation, before.generation + 1);
    assert_eq!(
        &saved.items[&ItemId::new(8).unwrap()].derived[12..16],
        &7u32.to_le_bytes()
    );
    assert_eq!(
        &saved.items[&ItemId::new(1).unwrap()].derived[64..68],
        &4u32.to_le_bytes()
    );
    assert_eq!(
        update.items.items[&8],
        listener.inventory.as_ref().unwrap().items.items[&8]
    );
    let settings = listener
        .vehicles
        .as_ref()
        .unwrap()
        .vehicles
        .customization(&update.items, 1)
        .unwrap();
    assert_eq!(settings.tuning[26], 7);
    assert_eq!(settings.tuning[31], 10);
    assert_eq!(listener.pending.len(), 1);
    assert_eq!(saved.tables, before.tables);
}

#[tokio::test]
async fn dyno_is_staged_with_equip_notified_after_commit_and_cancelled_with_a_new_edit() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    let stats = nfs_world::items::DynoStats::new([180, 400, 500, 120], [4.5, 10., 12.5]).unwrap();
    assert!(accept(&mut listener, &dyno_wire(scene, player + 1000, car, stats)).is_err());
    assert!(accept(&mut listener, &dyno_wire(scene, player, car + 1000, stats)).is_err());
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap());
    assert!(accept(&mut listener, &dyno_wire(scene, player, car, stats)).unwrap());
    assert_eq!(repo.snapshot(account).unwrap(), before);
    assert!(accept(&mut listener, &wire(scene, player, None, 4, 0)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(!jobs.working());
    assert_eq!(repo.snapshot(account).unwrap(), before);
    assert!(accept(&mut listener, &wire(scene, player, Some(car), 0, 7)).unwrap());
    assert!(accept(&mut listener, &dyno_wire(scene, player, car, stats)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 4, Timestamp(4));
    assert!(jobs.completed(&mut listener).await);
    let saved = repo.snapshot(account).unwrap();
    assert_eq!(saved.generation, before.generation + 1);
    let derived = Derived::decode(
        nfs_world::items::Layout::RaceVehicle,
        &saved.items[&ItemId::new(1).unwrap()].derived,
    )
    .unwrap();
    let mut expected = listener.inventory.as_ref().unwrap().items.items[&1]
        .derived
        .clone();
    stats.apply(&mut expected).unwrap();
    assert_eq!(derived, expected);
    // A new edit remains queued until the previous durable notification is delivered.
    assert!(accept(&mut listener, &dyno_wire(scene, player, car, stats)).unwrap());
    assert!(accept(&mut listener, &wire(scene, player, None, 5, 0)).unwrap());
    jobs.start(&mut listener, 5, Timestamp(5));
    assert!(!jobs.working());
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    assert_eq!(update.items.items[&1].derived, derived);
    assert!(update.items.items[&1].children.contains(&7));
    jobs.start(&mut listener, 6, Timestamp(6));
    assert!(jobs.completed(&mut listener).await);
    assert_eq!(repo.snapshot(account).unwrap(), saved);
    crate::net::items::tests::deliver_notification(&mut jobs);
}

#[tokio::test]
async fn chunked_dyno_between_equip_and_commit_is_persisted_atomically() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let before = repo.snapshot(account).unwrap();
    let stats = nfs_world::items::DynoStats::new([180, 400, 500, 120], [4.5, 10., 12.5]).unwrap();
    let equip = wire(scene, player, Some(car), 0, 7);
    let dyno = dyno_wire(scene, player, car, stats);
    let commit = wire(scene, player, None, 5, 0);
    assert!(dyno.bytes().len() > frame::CHUNK_BYTES);
    let chunks = frame::chunks(47, dyno.bytes()).unwrap();
    let mut messages = vec![frame::Message::Opaque {
        index: 47,
        body: equip.span(),
    }];
    for chunk in chunks {
        messages.push(chunk);
    }
    messages.push(frame::Message::Opaque {
        index: 47,
        body: commit.span(),
    });
    let frame = frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(0),
                messages,
            }],
        )
        .build()
        .unwrap();
    assert!(listener.frame(Queued {
        advertised: None,
        frame: 1,
        data: frame.span()
    }));
    assert_eq!(listener.equipment.committed.len(), 1);
    assert_eq!(listener.equipment.committed[0].dyno, Some(stats));
    // A retry in a different application frame does not create a second commit.
    assert!(listener.frame(Queued {
        advertised: None,
        frame: 2,
        data: frame.span()
    }));
    assert_eq!(listener.equipment.committed.len(), 1);
    jobs.start(&mut listener, 3, Timestamp(3));
    assert!(jobs.completed(&mut listener).await);
    assert_eq!(
        repo.snapshot(account).unwrap().generation,
        before.generation + 1
    );
    let update = crate::net::items::tests::deliver_notification(&mut jobs);
    let mut expected = before.items[&ItemId::new(1).unwrap()].derived.clone();
    let mut derived = Derived::decode(nfs_world::items::Layout::RaceVehicle, &expected).unwrap();
    stats.apply(&mut derived).unwrap();
    expected = derived.encode().unwrap();
    assert_eq!(
        repo.snapshot(account).unwrap().items[&ItemId::new(1).unwrap()].derived,
        expected
    );
    assert_eq!(update.items.items[&1].derived, derived);
    assert!(update.items.items[&1].children.contains(&7));
}
