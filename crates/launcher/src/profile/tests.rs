// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Synthetic account saves and content; no game, server or installation.

use super::*;
use nfs_services::persistent::required_tables;
use nfs_storage::{
    Batch, DefinitionGuid, GarageSlots, InventoryRepository, ItemRecord, Op, Timestamp,
    account::{Document, Repository},
    records::{Repository as _, SqliteRecords, UnixMicros},
    tables::{Row, Table, Value},
};
use serde_json::json;

const CAR: &str = "322ee1d18db783faf3017e18d69cfcc7";
/// A second car model without a vehicle profile.
const CAR2: &str = "5d0c3bdcd2b1a0e3ec9e5e0a1c6f7a10";
const PART: &str = "0090c333fea7512769c2daf1ab07d108";
const LIVERY: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const UNKNOWN: &str = "ffffffffffffffffffffffffffffffff";
const SAVED_AT: u64 = 1_760_794_047_000;
const BUILD: &str = nfs_services::SUPPORTED_BUILD_SHA256;

fn id(n: u8) -> String {
    format!("{n:02x}").repeat(16)
}
fn account(n: u8) -> AccountId {
    parse_account(&id(n)).unwrap()
}
fn accounts(data: &Path) -> PathBuf {
    let directory = data.join("artifacts").join("account");
    std::fs::create_dir_all(&directory).unwrap();
    directory
}
fn record(item: u64, definition: &str) -> ItemRecord {
    ItemRecord {
        id: ItemId::new(item).unwrap(),
        definition: DefinitionGuid(content::guid(definition).unwrap()),
        owner: None,
        state: 2,
        buy_price: 0,
        sell_price: 0,
        default_items: vec![],
        sub_items: vec![],
        derived: vec![],
    }
}
fn identity_document(account: AccountId, name: &str) -> Document {
    Document {
        revision: 1,
        bytes: json!({"version": 1, "storage_account": account.hex(), "persona": 1001,
            "account": 2002, "name": name})
        .to_string()
        .into_bytes(),
    }
}
fn row(cells: &[(&str, i32)]) -> Table {
    Table {
        rows: BTreeMap::from([(
            0,
            cells
                .iter()
                .map(|(column, value)| (key(column), Value::Int(*value)))
                .collect::<Row>(),
        )]),
    }
}
fn table(name: &str, table: Table) -> Op {
    Op::SetTable(key(name), table)
}

/// Every domain, three items (a car, a part and one the content does not
/// know), sparse REP and stats rows, three collectibles and a livery.
fn complete_save(data: &Path, n: u8) -> PathBuf {
    let directory = accounts(data);
    let a = account(n);
    let mut documents = vec![(Kind::Identity, identity_document(a, "Tester"))];
    for kind in &Kind::ALL[1..] {
        documents.push((
            *kind,
            Document {
                revision: 1,
                bytes: b"{}".to_vec(),
            },
        ));
    }
    let mut ops = vec![
        Op::Insert(record(7, CAR)),
        Op::Insert(record(8, PART)),
        Op::Insert(record(9, UNKNOWN)),
        Op::SetGarage(GarageSlots([
            ItemId::new(7),
            None,
            ItemId::new(9),
            None,
            None,
        ])),
        Balance(12_345).store(),
        // StyleScore is deliberately absent: it reads as the schema default.
        table(
            "RepValuesTable",
            row(&[
                ("RepScore", 15),
                ("RepLevel", 2),
                ("BuildScore", 1),
                ("CrewScore", 2),
                ("OutlawScore", 3),
                ("SpeedScore", 4),
            ]),
        ),
        table(
            "GameplayStatsTable",
            row(&[("DistanceDriven", 1_284_021), ("TotalTimePlayed", 151_200)]),
        ),
        table("ProgressionStats", row(&[("IntroCompleted", 1)])),
        table(
            "CollectiblesTable",
            Table {
                rows: (1..=3)
                    .map(|k| (k, Row::from([(key("C0"), Value::Int(1))])))
                    .collect(),
            },
        ),
    ];
    for (name, secondary, _) in required_tables() {
        let present = ops
            .iter()
            .any(|op| matches!(op, Op::SetTable(k, _) if *k == key(name)));
        if !present && *name != "GarageItemsTable" {
            let rows = if secondary.is_none() {
                BTreeMap::from([(0, Row::new())])
            } else {
                BTreeMap::new()
            };
            ops.push(table(name, Table { rows }));
        }
    }
    let batch = Batch {
        id: 1,
        expected_generation: 0,
        ops,
    };
    Repository::open_owned_directory(&directory)
        .unwrap()
        .initialize_fresh(a, &documents, &batch, Timestamp(SAVED_AT))
        .unwrap();
    let repository = SqliteRepository::open_owned_directory(&directory).unwrap();
    SqliteRecords(repository.clone())
        .put(
            a,
            "wrap",
            "application/octet-stream",
            &[1; 300],
            UnixMicros(1),
        )
        .unwrap();
    repository.path(a)
}

fn persistent_content() -> serde_json::Value {
    let tables: Vec<_> = required_tables()
        .iter()
        .map(|(name, secondary, count)| {
            let named = STORE_TABLES
                .iter()
                .find(|(table, _)| table == name)
                .map(|(_, columns)| columns.to_vec());
            let columns: Vec<_> = if *name == "GarageItemsTable" {
                [
                    "PrimaryVehicleItem",
                    "Vehicle1",
                    "Vehicle2",
                    "Vehicle3",
                    "Vehicle4",
                ]
                .iter()
                .map(|c| json!({"name": c, "key": key(c), "value_type": "string", "default": "0"}))
                .collect()
            } else {
                let names = named.unwrap_or_default();
                (0..*count)
                    .map(|i| {
                        let column = names.get(i).map_or(format!("C{i}"), |c| (*c).to_owned());
                        json!({"name": column, "key": key(&column), "value_type": "int", "default": "0"})
                    })
                    .collect()
            };
            json!({"name": name, "key": key(name), "secondary_key": secondary,
                "using_stores": secondary.is_none(), "write_whole_rows": false, "columns": columns})
        })
        .collect();
    json!({"version": 1, "build_sha256": BUILD,
        "source": {"path": "synthetic", "sha256": "0".repeat(64)}, "tables": tables})
}

/// A prepared content cache in the data directory.
fn install_content(data: &Path) -> PathBuf {
    let directory = data.join("artifacts").join("content").join("e".repeat(64));
    std::fs::create_dir_all(&directory).unwrap();
    let write = |name: &str, value: serde_json::Value| {
        std::fs::write(directory.join(name), value.to_string()).unwrap();
    };
    write(
        "reputation-thresholds.json",
        json!({"format": "nfs-reputation-thresholds", "version": 1, "build_sha256": BUILD,
            "thresholds": [0, 10, 25]}),
    );
    write("persistent-content.json", persistent_content());
    write(
        "item-content.json",
        json!({"format": "nfs-item-definitions", "version": 1, "build_sha256": BUILD,
            "definitions": [{"guid": CAR, "class": "RaceVehicleItemData"},
                {"guid": CAR2, "class": "RaceVehicleItemData"},
                {"guid": PART, "class": "AirFilterItemData"},
                {"guid": LIVERY, "class": "LiveryCustomizationItemData"}]}),
    );
    std::fs::write(
        directory.join("vehicle-profiles.json"),
        content::tests::vehicle_file(json!([{
            "blueprint": "Vehicles/player/Car_Ford_MustangGT_2015/Car_Ford_MustangGT_2015",
            "definitions": [CAR]
        }])),
    )
    .unwrap();
    directory
}

fn select(data: &Path, n: u8) {
    records::write_new_json(
        &data.join("account.json"),
        &records::AccountRecord::post_intro(id(n)),
    )
    .unwrap();
}

fn read(data: &Path, n: u8) -> Profile {
    let saves = Saves::open(data).unwrap();
    let content = content(data, None, Need::Full).unwrap();
    let profile = saves.profile(&id(n), &content).unwrap();
    crate::contract::testing::validate("profile", &profile);
    profile
}

#[test]
fn a_complete_save_reports_raw_values_decoded_domains_and_content_names() {
    let data = crate::testing::scratch("profile-complete");
    let path = complete_save(&data, 1);
    let cache = install_content(&data);
    select(&data, 1);
    let before = std::fs::read(&path).unwrap();
    let p = read(&data, 1);
    assert_eq!(p.warnings, Vec::<String>::new());
    assert!(p.active);
    assert_eq!(
        p.content,
        Some(Source {
            directory: cache,
            chosen_by: "only-cache"
        })
    );
    let driver = p.identity.clone().unwrap();
    assert_eq!(
        (driver.display_name.as_str(), driver.persona_id.as_str()),
        ("Tester", "1001")
    );
    assert_eq!(driver.account_id, id(1));
    assert_eq!(
        p.bank,
        Some(Bank {
            balance: 12_345,
            formatted: "$12,345".into()
        })
    );
    let rep = p.reputation.clone().unwrap();
    let stored = |value| {
        Some(Cell {
            value,
            stored: true,
        })
    };
    assert_eq!((rep.score, rep.stored_level), (stored(15), stored(2)));
    assert_eq!(rep.categories.speed, stored(4));
    assert_eq!(
        rep.categories.style,
        Some(Cell {
            value: 0,
            stored: false
        })
    );
    assert_eq!(
        (
            rep.level,
            rep.current_threshold,
            rep.next_threshold,
            rep.maximum_level
        ),
        (Some(2), Some(10), Some(25), Some(3))
    );
    let tables = p.tables.clone().unwrap();
    assert_eq!(tables.len(), STORE_TABLES.len());
    let stats = &tables["GameplayStatsTable"];
    assert_eq!(stats.0.len(), 9);
    assert_eq!(stats.get("DistanceDriven"), stored(1_284_021));
    assert_eq!(
        stats.get("TopSpeed"),
        Some(Cell {
            value: 0,
            stored: false
        })
    );
    assert_eq!(tables["PrestigeMedalTable"].0.len(), 6);
    assert_eq!(tables["SpeedListStatsTable"].0.len(), 10);
    assert_eq!(tables["ProgressionStats"].get("IntroCompleted"), stored(1));
    let garage = p.garage.clone().unwrap();
    assert_eq!(garage.slots.len(), 5);
    let car = garage.slots[0].item.clone().unwrap();
    assert!(garage.slots[0].primary && !garage.slots[2].primary);
    assert_eq!((car.id.as_str(), car.definition.as_str()), ("7", CAR));
    assert_eq!(car.class, Some("RaceVehicleItemData"));
    assert_eq!(car.vehicle.unwrap().label, "Ford MustangGT 2015");
    assert_eq!(garage.primary_vehicle, garage.slots[0].item);
    assert!(garage.slots[1].item.is_none());
    let unknown = garage.slots[2].item.clone().unwrap();
    assert_eq!((unknown.class, unknown.vehicle), (None, None));
    // Only the car is a car; the unknown item still occupies its slot.
    let cars = p.cars.clone().unwrap();
    assert_eq!(cars.len(), 1);
    assert_eq!(
        (cars[0].id.as_str(), cars[0].location, cars[0].slot),
        ("7", CarLocation::Active, Some(0))
    );
    assert_eq!(
        p.car_summary,
        Some(CarSummary {
            garage_used: 2,
            garage_slots: 5,
            warehouse_used: 0,
            warehouse_limit: 100,
            total_sale_value: 0,
        })
    );
    assert_eq!(
        p.counts,
        Counts {
            items: 3,
            items_by_class: Some(BTreeMap::from([
                ("AirFilterItemData", 1),
                ("RaceVehicleItemData", 1)
            ])),
            unresolved_items: Some(1),
            livery_records: 1,
            livery_bytes: 300,
            collectibles: Some(3),
            activities: Some(0),
            points_of_interest: Some(0),
        }
    );
    assert_eq!(p.save.schema_version, nfs_storage::sqlite::VERSION);
    assert_eq!((p.save.generation, p.save.updated_at_ms), (1, SAVED_AT));
    assert_eq!(
        p.save.updated_at_utc.as_deref(),
        Some("2025-10-18T13:27:27Z")
    );
    assert_eq!(p.save.file_bytes, before.len() as u64);
    assert_eq!(p.save.domains.len(), 5);
    assert_eq!(p.save.domains["identity"].revision, 1);
    // JSON keeps raw integers, stored names and string identifiers.
    let value = serde_json::to_value(&p).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(
        value["tables"]["GameplayStatsTable"]["DistanceDriven"],
        json!({"value": 1_284_021, "stored": true})
    );
    assert_eq!(value["garage"]["slots"][0]["item"]["id"], "7");
    assert_eq!(value["identity"]["persona_id"], "1001");
    assert_eq!(value["bank"]["balance"], 12_345);
    // Reading changed nothing in the save.
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::remove_dir_all(data).unwrap();
}

/// An owned record with the given owner, installed children and stock defaults.
fn part(item: u64, definition: &str, owner: Option<u64>, sub: &[u64], defaults: &[u64]) -> Op {
    let ids = |list: &[u64]| list.iter().map(|&i| ItemId::new(i).unwrap()).collect();
    Op::Insert(ItemRecord {
        owner: owner.map(|o| ItemId::new(o).unwrap()),
        sub_items: ids(sub),
        default_items: ids(defaults),
        ..record(item, definition)
    })
}

fn car(item: u64, definition: &str, sell_price: u32, sub: &[u64], defaults: &[u64]) -> Op {
    let Op::Insert(record) = part(item, definition, None, sub, defaults) else {
        unreachable!()
    };
    Op::Insert(ItemRecord {
        sell_price,
        ..record
    })
}

/// Two garage cars and two warehouse cars. Car 10 is active with a stock
/// part tree (11 > 12), a replaced stock part (13), a purchased spare (14),
/// an installed upgrade (15) and an installed library wrap (16). Item 40 is
/// an unowned car offer and 70 a car-class record that car 60 owns, so a
/// spare of 60 rather than a car.
fn fleet_save(data: &Path, n: u8) {
    let directory = accounts(data);
    let a = account(n);
    let mut ops = vec![
        car(10, CAR, 5_000, &[11, 15, 16], &[11, 13]),
        part(11, PART, Some(10), &[12], &[12]),
        // An installed-link cycle back to 11 must not loop.
        part(12, PART, Some(11), &[11], &[]),
        part(13, PART, Some(10), &[], &[]),
        part(14, PART, Some(10), &[], &[]),
        part(15, PART, Some(10), &[], &[]),
        part(16, LIVERY, None, &[], &[]),
        car(20, CAR2, 7_000, &[], &[]),
        car(30, CAR, 1_000, &[31], &[31]),
        part(31, PART, Some(30), &[], &[]),
        car(60, CAR2, 0, &[], &[]),
        part(70, CAR, Some(60), &[], &[]),
        Op::SetGarage(GarageSlots([
            ItemId::new(10),
            None,
            None,
            ItemId::new(20),
            None,
        ])),
    ];
    let Op::Insert(offer) = car(40, CAR, 9_000, &[], &[]) else {
        unreachable!()
    };
    ops.push(Op::Insert(ItemRecord {
        state: nfs_world_core::items::PURCHASABLE,
        ..offer
    }));
    let mut documents = vec![(Kind::Identity, identity_document(a, "Collector"))];
    for kind in &Kind::ALL[1..] {
        documents.push((
            *kind,
            Document {
                revision: 1,
                bytes: b"{}".to_vec(),
            },
        ));
    }
    Repository::open_owned_directory(&directory)
        .unwrap()
        .initialize_fresh(
            a,
            &documents,
            &Batch {
                id: 1,
                expected_generation: 0,
                ops,
            },
            Timestamp(SAVED_AT),
        )
        .unwrap();
}

#[test]
fn cars_cover_the_garage_and_the_warehouse_with_parts_spares_and_prices() {
    let data = crate::testing::scratch("profile-cars");
    fleet_save(&data, 3);
    install_content(&data);
    let p = read(&data, 3);
    let cars = p.cars.clone().unwrap();
    let summary: Vec<_> = cars
        .iter()
        .map(|c| {
            (
                c.id.as_str(),
                c.location,
                c.slot,
                c.installed_parts,
                c.spare_parts,
                c.item_records,
                c.sale_price,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("10", CarLocation::Active, Some(0), 4, 2, 6, 5_000),
            ("20", CarLocation::Garage, Some(3), 0, 0, 1, 7_000),
            ("30", CarLocation::Warehouse, None, 1, 0, 2, 1_000),
            ("60", CarLocation::Warehouse, None, 0, 1, 2, 0),
        ]
    );
    assert_eq!(cars[0].display_name.as_deref(), Some("Ford MustangGT 2015"));
    assert_eq!(cars[0].definition, CAR);
    assert_eq!(
        cars[1].display_name, None,
        "no vehicle profile, no guessed name"
    );
    assert_eq!(
        p.car_summary,
        Some(CarSummary {
            garage_used: 2,
            garage_slots: 5,
            warehouse_used: 2,
            warehouse_limit: nfs_services::garage::WAREHOUSE_CAPACITY,
            total_sale_value: 13_000,
        })
    );
    let value = serde_json::to_value(&p).unwrap();
    assert_eq!(value["cars"][1]["location"], "garage");
    assert_eq!(value["cars"][2]["slot"], serde_json::Value::Null);
    assert_eq!(value["car_summary"]["warehouse_limit"], 100);

    // Without item content nothing can be called a car.
    std::fs::remove_dir_all(data.join("artifacts").join("content")).unwrap();
    let p = read(&data, 3);
    assert!(p.cars.is_none() && p.car_summary.is_none());
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn missing_and_partial_domains_are_null_with_warnings() {
    let data = crate::testing::scratch("profile-partial");
    let directory = accounts(&data);
    let a = account(2);
    // Only settings: no identity, tables, garage, bank or items yet.
    Repository::open_owned_directory(&directory)
        .unwrap()
        .initialize(
            a,
            &[(
                Kind::Settings,
                Document {
                    revision: 4,
                    bytes: b"{}".to_vec(),
                },
            )],
        )
        .unwrap();
    install_content(&data);
    let p = read(&data, 2);
    assert!(!p.active);
    assert!(p.identity.is_none() && p.reputation.is_none() && p.tables.is_none());
    assert!(p.garage.is_none());
    assert_eq!(p.bank.unwrap().balance, 0, "no bank record is zero cash");
    assert_eq!(p.counts.items, 0);
    assert_eq!(p.counts.collectibles, None);
    assert_eq!(
        p.save.domains.keys().copied().collect::<Vec<_>>(),
        ["settings"]
    );
    assert_eq!(p.save.domains["settings"].revision, 4);
    assert_eq!(p.save.updated_at_utc, None, "never written by a batch");
    assert_eq!(p.warnings.len(), 3, "{:?}", p.warnings);
    // An identity that does not decode, or names another account, is not shown.
    let repository = Repository::open_owned_directory(&directory).unwrap();
    repository
        .compare_exchange(a, Kind::Identity, None, b"not json")
        .unwrap();
    assert!(read(&data, 2).identity.is_none());
    repository
        .compare_exchange(
            a,
            Kind::Identity,
            Some(1),
            &identity_document(account(3), "Other").bytes,
        )
        .unwrap();
    let p = read(&data, 2);
    assert!(p.identity.is_none());
    assert!(p.warnings.iter().any(|w| w.contains("another account")));
    // Without any prepared content the save still reads.
    std::fs::remove_dir_all(data.join("artifacts").join("content")).unwrap();
    complete_save(&data, 1);
    let p = read(&data, 1);
    assert_eq!(p.identity.unwrap().display_name, "Tester");
    assert!(p.reputation.is_none() && p.tables.is_none() && p.content.is_none());
    assert_eq!(p.counts.items_by_class, None);
    assert_eq!(p.counts.collectibles, Some(3));
    assert!(
        p.garage.unwrap().slots[0]
            .item
            .as_ref()
            .unwrap()
            .vehicle
            .is_none()
    );
    assert!(p.warnings[0].contains("No prepared content"));
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn absent_saves_invalid_ids_and_records_are_refused() {
    let data = crate::testing::scratch("profile-absent");
    let saves = Saves::open(&data.join("never-used")).unwrap();
    assert_eq!(saves.active(), None);
    let none = Content::default();
    assert_eq!(
        saves.profile(&id(1), &none).unwrap_err().exit,
        crate::exit::Exit::NoSession
    );
    assert!(saves.list(&none).unwrap().accounts.is_empty());
    accounts(&data);
    let saves = Saves::open(&data).unwrap();
    assert_eq!(
        saves.profile(&id(1), &none).unwrap_err().exit,
        crate::exit::Exit::NoSession
    );
    for invalid in ["", "AB", &"A".repeat(32), &"0".repeat(32), &"g".repeat(32)] {
        assert_eq!(
            saves.profile(invalid, &none).unwrap_err().exit,
            crate::exit::Exit::Refused,
            "{invalid}"
        );
    }
    std::fs::write(data.join("account.json"), b"{\"version\":1}").unwrap();
    assert!(Saves::open(&data).is_err());
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn listing_reports_every_save_isolates_damaged_ones_and_is_bounded() {
    let data = crate::testing::scratch("profile-list");
    install_content(&data);
    complete_save(&data, 1);
    complete_save(&data, 3);
    select(&data, 3);
    let directory = accounts(&data);
    std::fs::write(directory.join(format!("{}.sqlite", id(2))), [0x5a; 4096]).unwrap();
    for ignored in [
        "notes.txt".to_owned(),
        format!("{}.sqlite-wal", id(4)),
        format!("{}.sqlite", "A".repeat(32)),
    ] {
        std::fs::write(directory.join(ignored), b"x").unwrap();
    }
    let saves = Saves::open(&data).unwrap();
    let listing = saves
        .list(&content(&data, None, Need::Summary).unwrap())
        .unwrap();
    crate::contract::testing::validate("profile-list", &listing);
    assert!(!listing.truncated);
    let ids: Vec<_> = listing
        .accounts
        .iter()
        .map(|e| e.account_id.clone())
        .collect();
    assert_eq!(ids, [id(1), id(2), id(3)]);
    let first = &listing.accounts[0];
    assert_eq!(first.display_name.as_deref(), Some("Tester"));
    assert_eq!(
        (first.level, first.stored_level, first.score),
        (Some(2), Some(2), Some(15))
    );
    assert_eq!(first.bank.as_ref().unwrap().formatted, "$12,345");
    assert_eq!(first.updated_at_ms, Some(SAVED_AT));
    assert!(!first.active && listing.accounts[2].active);
    let damaged = &listing.accounts[1];
    assert!(damaged.error.is_some() && damaged.display_name.is_none());
    for n in 10..10 + MAX_ACCOUNTS as u8 {
        std::fs::write(directory.join(format!("{}.sqlite", id(n))), b"").unwrap();
    }
    let listing = saves.list(&Content::default()).unwrap();
    crate::contract::testing::validate("profile-list", &listing);
    assert!(listing.truncated);
    assert_eq!(listing.accounts.len(), MAX_ACCOUNTS);
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn a_server_committing_concurrently_never_blocks_or_tears_a_profile() {
    let data = crate::testing::scratch("profile-concurrent");
    let path = complete_save(&data, 1);
    install_content(&data);
    let a = account(1);
    let writer = SqliteRepository::open_owned_directory(path.parent().unwrap()).unwrap();
    // An open write transaction is invisible to the profile and does not block it.
    let raw = rusqlite_connection(&path);
    raw.execute_batch("BEGIN IMMEDIATE; UPDATE meta SET updated_at=updated_at+1;")
        .unwrap();
    let started = std::time::Instant::now();
    assert_eq!(read(&data, 1).save.updated_at_ms, SAVED_AT);
    assert!(started.elapsed() < Duration::from_secs(2));
    raw.execute_batch("ROLLBACK;").unwrap();
    drop(raw);
    let thread = std::thread::spawn(move || {
        for generation in 1..60u64 {
            writer
                .apply(
                    a,
                    &Batch {
                        id: 1000 + generation,
                        expected_generation: generation,
                        ops: vec![Balance(generation as u32 + 1).store()],
                    },
                    Timestamp(SAVED_AT + generation),
                )
                .unwrap();
        }
    });
    let saves = Saves::open(&data).unwrap();
    let summary = content(&data, None, Need::Summary).unwrap();
    let mut reads = 0;
    while !thread.is_finished() || reads == 0 {
        let p = saves.profile(&id(1), &summary).unwrap();
        // Bank and generation always come from the same commit.
        if p.save.generation > 1 {
            assert_eq!(u64::from(p.bank.unwrap().balance), p.save.generation);
        }
        reads += 1;
    }
    thread.join().unwrap();
    assert_eq!(saves.profile(&id(1), &summary).unwrap().save.generation, 60);
    std::fs::remove_dir_all(data).unwrap();
}

/// A second writer on the save, as another server process would hold it.
fn rusqlite_connection(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

#[test]
fn numbers_and_times_format_only_within_their_bounds() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1_000), "1,000");
    assert_eq!(grouped(-1_234_567), "-1,234,567");
    assert_eq!(grouped(i64::MIN), "-9,223,372,036,854,775,808");
    assert_eq!(utc(0), None);
    assert_eq!(utc(1), Some("1970-01-01T00:00:00Z".into()));
    assert_eq!(utc(MAX_FORMATTED_MS), Some("9999-12-31T23:59:59Z".into()));
    assert_eq!(utc(MAX_FORMATTED_MS + 1), None);
    assert_eq!(utc(u64::MAX), None);
}

#[cfg(unix)]
#[test]
fn a_linked_account_directory_is_refused() {
    let data = crate::testing::scratch("profile-link");
    let elsewhere = crate::testing::scratch("profile-link-target");
    std::fs::create_dir_all(data.join("artifacts")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, data.join("artifacts").join("account")).unwrap();
    assert!(Saves::open(&data).is_err());
    std::fs::remove_dir_all(data).unwrap();
    std::fs::remove_dir_all(elsewhere).unwrap();
}
