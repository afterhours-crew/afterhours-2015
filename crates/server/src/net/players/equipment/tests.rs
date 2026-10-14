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
    let car = items.items.get_mut(&1).unwrap();
    car.children.push(6);
    car.defaults.push(6);
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
    let profile = Service::new(repo.clone(), catalog)
        .with_persistent(tables)
        .with_purchase_offers(nfs_services::purchase::Offers::new([[7; 16]]).unwrap())
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
