// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! End-to-end tests over constructed installations; no game data is used.
use super::*;
use crate::synthetic::{self, Asset, assets, guid, item, table, vehicle};
use nfs_services::item_content::ItemContent;
use nfs_services::persistent::{Catalog, required_tables};
use std::sync::atomic::{AtomicU32, Ordering};

struct TempDir(PathBuf);

#[test]
fn progress_tracks_real_extraction_cache_reuse_and_failed_work() {
    let install = install_with(&assets());
    let mut events = Vec::new();
    let first = prepare_with_progress(
        &install.game(),
        &install.cache(),
        &install.options(),
        &mut |event| events.push(event),
    )
    .unwrap();
    assert!(first.built);
    assert_eq!(events.first(), Some(&Progress::Fingerprinting));
    assert_eq!(events.last(), Some(&Progress::Ready));
    for kind in Kind::ALL {
        let begin = events
            .iter()
            .position(|e| *e == Progress::Extracting(kind))
            .unwrap();
        let bytes = std::fs::metadata(first.path(kind)).unwrap().len() as usize;
        assert_eq!(events[begin + 1], Progress::Extracted { kind, bytes });
    }
    events.clear();
    let second = prepare_with_progress(
        &install.game(),
        &install.cache(),
        &install.options(),
        &mut |event| events.push(event),
    )
    .unwrap();
    assert!(!second.built);
    assert_eq!(
        events,
        [
            Progress::Fingerprinting,
            Progress::CacheCheck,
            Progress::CacheReused,
            Progress::Ready
        ]
    );
    events.clear();
    let mut invalid = install.options();
    invalid.identity.executable_sha256 = "0".repeat(64);
    assert!(
        prepare_with_progress(&install.game(), &install.cache(), &invalid, &mut |event| {
            events.push(event)
        })
        .is_err()
    );
    assert_eq!(events, [Progress::Fingerprinting]);
}
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "nfs-content-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Install {
    dir: TempDir,
    options: Options,
    template: [u8; 64],
}

fn install_with(assets: &[Asset]) -> Install {
    let dir = TempDir::new();
    let (options, template) = synthetic::write(&dir.0.join("game"), assets).unwrap();
    Install {
        dir,
        options,
        template,
    }
}

impl Install {
    fn game(&self) -> PathBuf {
        self.dir.0.join("game")
    }
    fn cache(&self) -> PathBuf {
        self.dir.0.join("cache")
    }
    fn options(&self) -> Options {
        self.options.clone()
    }
    fn prepare(&self) -> Result<Prepared, Error> {
        prepare(&self.game(), &self.cache(), &self.options())
    }
}

#[test]
fn installed_kit_offers_exclude_unknown_unlocks_and_nonvisual_children() {
    use synthetic::Field;
    for (class, criterion, offered) in [
        ("HoodItemData", "ID_UNLOCKED_AT_REP_LEVEL", true),
        ("AirFilterItemData", "ID_UNLOCKED_AT_REP_LEVEL", false),
        ("HoodItemData", "UNKNOWN_STORY_CONDITION", false),
    ] {
        let mut content = assets();
        content[0] = item(
            1,
            class,
            vec![],
            vec![],
            vec![
                ("Quantity", Field::Int(1)),
                ("UnlocksAtRepLevel", Field::Int(0)),
                ("UnlockCriteriaDescriptionID", Field::CString(String::new())),
                ("PerformanceModifier", Field::RawPointer(0)),
                ("MeshGuids", Field::Guids(vec![])),
                ("WideCollision", Field::Bool(false)),
                ("Percent", Field::Float(0.0)),
            ],
        );
        content[1] = item(
            2,
            "BodyKitItemData",
            vec![1],
            vec![],
            vec![
                ("Quantity", Field::Int(1)),
                ("UnlocksAtRepLevel", Field::Int(0)),
                (
                    "UnlockCriteriaDescriptionID",
                    Field::CString(criterion.into()),
                ),
            ],
        );
        let install = install_with(&content);
        let prepared = install.prepare().unwrap();
        let items = ItemContent::load(&prepared.path(Kind::ItemContent)).unwrap();
        let kits = items.purchase_offers().unwrap().body_kits();
        assert_eq!(kits.contains_key(&synthetic::reference(2).1), offered);
        assert_eq!(kits.len(), usize::from(offered));
    }
}

#[test]
fn installed_fitment_restrictions_round_trip_and_reject_unknown_targets() {
    use synthetic::Field;
    for (target, valid) in [(2, true), (99, false)] {
        let mut content = assets();
        content[0] = item(
            1,
            "SpoilerItemData",
            vec![],
            vec![],
            vec![
                ("DownforceSetting", Field::Int(5)),
                ("AnimationExtent", Field::Float(0.75)),
                ("AnimationId", Field::Int(-1)),
                ("SpoilerTuning", Field::RawPointer(0)),
                ("WideCollision", Field::Bool(false)),
                ("PerformanceModifier", Field::RawPointer(0)),
                ("MeshGuids", Field::Guids(vec![])),
                (
                    "SortedRestrictions",
                    Field::Guids(vec![synthetic::reference(target).1]),
                ),
            ],
        );
        let install = install_with(&content);
        let prepared = install.prepare();
        if valid {
            let items = ItemContent::load(&prepared.unwrap().path(Kind::ItemContent)).unwrap();
            let json = items.to_json();
            assert_eq!(
                json["version"],
                nfs_services::item_content::CAR_OWNERSHIP_VERSION
            );
            assert_eq!(json["rep_rewards"], serde_json::json!([]));
            assert_eq!(
                json["fitment_restrictions"],
                serde_json::json!([{
                    "definition": nfs_frostbite::hex(&synthetic::reference(1).1),
                    "conflicts": [nfs_frostbite::hex(&synthetic::reference(2).1)],
                }])
            );
            assert_eq!(ItemContent::from_json(&json).unwrap().to_json(), json);
        } else {
            assert!(prepared.is_err());
        }
    }
}

#[test]
fn traffic_loader_rejects_broken_closure_catalog_and_geometry() {
    let install = install_with(&assets());
    let prepared = install.prepare().unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared.path(Kind::TrafficContent)).unwrap())
            .unwrap();
    for (path, bad) in [
        ("/managers/alive/scene", serde_json::json!(0)),
        ("/managers/alive/serializer", serde_json::json!(65535)),
        ("/version", serde_json::json!(2)),
        (
            "/managers/actors",
            value["managers"]["participants"].clone(),
        ),
        ("/personas/0/index", serde_json::json!(1)),
        ("/personas/0/vehicle", serde_json::json!("missing/bundle")),
        ("/personas/0/tags", serde_json::json!(["invalid-guid"])),
        ("/splines/0/catalog_count", serde_json::json!(1)),
        ("/splines/0/index", serde_json::json!(2)),
        ("/splines/0/segments/0/speed", serde_json::json!(-1)),
        ("/splines/0/segments/0/points/0", serde_json::json!([0, 1])),
        (
            "/splines/0/segments/0/points/0",
            serde_json::json!([100, 3, 4]),
        ),
        ("/splines/0/segments/0/controls", serde_json::json!([])),
        ("/splines/0/segments/0/times", serde_json::json!([0.])),
        ("/splines/0/segments/0/distances", serde_json::json!([-1.])),
        ("/density", serde_json::json!(101)),
        ("/vehicles", serde_json::json!([])),
    ] {
        let mut corrupt = value.clone();
        *corrupt.pointer_mut(path).unwrap() = bad;
        assert!(traffic::Content::from_json(&corrupt).is_err(), "{path}");
    }
    let mut duplicate = value.clone();
    duplicate["splines"]
        .as_array_mut()
        .unwrap()
        .push(value["splines"][0].clone());
    assert!(traffic::Content::from_json(&duplicate).is_err());
    duplicate["splines"][1]["index"] = serde_json::json!(0);
    assert!(traffic::Content::from_json(&duplicate).is_ok());
    duplicate["splines"][1]["catalog_count"] = serde_json::json!(3);
    assert!(traffic::Content::from_json(&duplicate).is_err());
    duplicate["splines"][1]["scene"] = serde_json::json!(123);
    assert!(traffic::Content::from_json(&duplicate).is_ok());
    let mut empty_road = value.clone();
    empty_road["splines"][0]["segments"] = serde_json::json!([]);
    let parsed = traffic::Content::from_json(&empty_road).unwrap();
    assert_eq!(parsed.splines.len(), 1);
    assert!(parsed.splines[0].segments.is_empty());
    let mut extra = value;
    extra["splines"][0]["unexpected"] = serde_json::json!(true);
    assert!(traffic::Content::from_json(&extra).is_err());
}

#[test]
fn builds_outputs_the_service_loaders_accept() {
    let install = install_with(&assets());
    let prepared = install.prepare().unwrap();
    assert!(prepared.built);
    assert_eq!(
        prepared.directory,
        install.cache().join(&prepared.fingerprint.digest)
    );
    let items = ItemContent::load(&prepared.path(Kind::ItemContent)).unwrap();
    progression::Content::load(&prepared.path(Kind::ProgressionContent)).unwrap();
    sequences::SequenceContent::load(&prepared.path(Kind::SequenceContent)).unwrap();
    let traffic = traffic::Content::load(&prepared.path(Kind::TrafficContent)).unwrap();
    let rivals = rivals::Content::load(&prepared.path(Kind::RivalContent)).unwrap();
    assert_eq!(rivals.managers, traffic.managers);
    assert_eq!(rivals.personas.len(), 1);
    assert_eq!(rivals.racers.len(), 1);
    assert_eq!(rivals.personas[0].name, "SyntheticRival");
    assert_eq!(rivals.personas[0].driving.performance, 0.2);
    assert_eq!(rivals.personas[0].driving.grip, Some(4));
    assert_eq!(rivals.personality_count, 2);
    assert_eq!(rivals.personas[0].personality_index, 1);
    assert_eq!(rivals.racers[0].position, [8., 3., -9.]);
    assert!(rivals.racers[0].regions[0].contains([0., 1., 0.]));
    assert!(!rivals.racers[0].regions[0].contains([21., 1., 0.]));
    assert_eq!(traffic.personas.len(), 1);
    assert_eq!(traffic.splines.len(), 1);
    assert_eq!(traffic.splines[0].index, 1);
    assert_eq!(traffic.splines[0].catalog_count, 2);
    assert_eq!(
        traffic.splines[0].segments[0].points,
        [[2., 3., 4.], [12., 3., 4.]]
    );
    vehicle_parts::Catalog::load(&prepared.path(Kind::VehicleParts), &items.catalog).unwrap();
    let parts: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared.path(Kind::VehicleParts)).unwrap()).unwrap();
    assert_eq!(parts["parts"]["customization"].as_array().unwrap().len(), 5);
    assert_eq!(
        parts["parts"]["appearance"][0]["data_id"],
        "18446744073709551603"
    );
    assert_eq!(parts["parts"]["appearance"][0]["version"], 2);
    let scenes = scenes::Catalog::load(&prepared.path(Kind::SceneCatalog)).unwrap();
    assert_eq!(scenes.profiles.profiles().count(), 21);
    assert!(scenes.profiles.profile(1).unwrap().is_root());
    assert_eq!(
        scenes
            .profiles
            .profile(assets::scene_key("vehicles/synthetic_bundle"))
            .unwrap()
            .kinds(),
        [nfs_world::replication::sublevel::Kind::Noop]
    );
    let json = items.to_json();
    let definitions = json["definitions"].as_array().unwrap();
    assert_eq!(definitions.len(), 5);
    let by_class = |class: &str| {
        definitions
            .iter()
            .find(|d| d["class"] == class)
            .unwrap()
            .clone()
    };
    let spoiler = by_class("SpoilerItemData");
    assert_eq!(spoiler["guid"], nfs_frostbite::hex(&guid(1 ^ 0x80)));
    assert_eq!(spoiler["buy_price"], 350);
    assert_eq!(spoiler["quantity"], 4_294_967_295u64);
    assert_eq!(spoiler["ownership_level"], 2);
    assert_eq!(spoiler["flags"]["optional"], false);
    assert_eq!(spoiler["default_derived_hex"], "05000000");
    let body_kit = by_class("BodyKitItemData");
    assert_eq!(body_kit["default_derived_hex"], "");
    assert_eq!(body_kit["sub_items"][0], spoiler["guid"]);
    let vehicle = by_class("RaceVehicleItemData");
    let mut expected = Vec::new();
    for v in [0.025f32, 0.11, 0.6, 0.76, 0.73, 1.0, 0.9] {
        expected.extend(v.to_bits().to_le_bytes());
    }
    expected.extend(b"GHOST\0\0\0");
    for v in [
        327u32,
        397,
        4.79f32.to_bits(),
        10.97f32.to_bits(),
        155,
        110,
        13.26f32.to_bits(),
        0,
    ] {
        expected.extend(v.to_le_bytes());
    }
    assert_eq!(
        vehicle["default_derived_hex"],
        nfs_frostbite::hex(&expected)
    );
    assert_eq!(vehicle["additional_items"][0], body_kit["guid"]);
    assert!(by_class("TimedDiscountItemData")["default_derived_hex"].is_null());
    assert_eq!(
        by_class("LiveryCustomizationItemData")["default_derived_hex"],
        "00000000ffffffff00000000"
    );
    assert_eq!(
        json["initial_definitions"],
        serde_json::json!([vehicle["guid"], spoiler["guid"]])
    );
    let tables = Catalog::load(&prepared.path(Kind::PersistentContent)).unwrap();
    drop(tables);
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared.path(Kind::PersistentContent)).unwrap())
            .unwrap();
    assert_eq!(
        document["tables"].as_array().unwrap().len(),
        required_tables().len()
    );
    assert_eq!(document["source"]["sha256"], prepared.fingerprint.digest);
    let stats = nfs_services::stats::Catalog::load(&prepared.path(Kind::StatDefinitions)).unwrap();
    assert_eq!(stats.len(), 2);
    let stats: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared.path(Kind::StatDefinitions)).unwrap())
            .unwrap();
    assert_eq!(
        stats["groups"][1]["stats"][0]["name"],
        "RepValuesTableColumn0"
    );
    let licenses =
        nfs_services::item_licenses::Content::load(&prepared.path(Kind::ItemLicenses)).unwrap();
    assert_eq!(licenses.item_system(), "Items/GameItemSystem");
    let catalogs: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared.path(Kind::ControlCatalogs)).unwrap())
            .unwrap();
    nfs_services::control_catalogs::Catalog::from_json(&catalogs).unwrap();
    assert_eq!(catalogs["key_scopes"][1]["name"], "SpeedWall_Synthetic");
    assert_eq!(
        catalogs["key_scopes"][1]["values"],
        serde_json::json!([[10, 11], [99, 99]])
    );
    assert_eq!(catalogs["speed_list_types"], serde_json::json!([]));
    assert_eq!(
        licenses
            .licenses()
            .iter()
            .map(|l| l.license.as_str())
            .collect::<Vec<_>>(),
        ["SyntheticA", "SyntheticB"]
    );
    assert_eq!(
        std::fs::read(prepared.path(Kind::WorldMacTemplate)).unwrap(),
        install.template
    );
}

#[test]
fn changed_construction_rules_do_not_reuse_an_installation_cache() {
    let install = install_with(&assets());
    let first = install.prepare().unwrap();
    let mut options = install.options();
    options.identity.scenes.level = "levels/absent".into();
    assert_ne!(
        first.fingerprint,
        fingerprint(&install.game(), &options).unwrap()
    );
    assert!(prepare(&install.game(), &install.cache(), &options).is_err());
    let mut changed_links = install.options();
    changed_links.identity.garage_links.primary_slot = 999;
    assert_ne!(
        first.fingerprint,
        fingerprint(&install.game(), &changed_links).unwrap()
    );
    assert!(prepare(&install.game(), &install.cache(), &changed_links).is_err());
    let mut changed_exit = install.options();
    changed_exit.identity.garage_links.exit_location = 999;
    assert_ne!(
        first.fingerprint,
        fingerprint(&install.game(), &changed_exit).unwrap()
    );
    assert!(prepare(&install.game(), &install.cache(), &changed_exit).is_err());
    let mut changed_vehicles = install.options();
    changed_vehicles.identity.vehicles.mesh_link ^= 1;
    assert_ne!(
        first.fingerprint,
        fingerprint(&install.game(), &changed_vehicles).unwrap()
    );
    assert!(prepare(&install.game(), &install.cache(), &changed_vehicles).is_err());
    for mutate in [
        |id: &mut BuildIdentity| id.traffic_spline_class_id ^= 1,
        |id: &mut BuildIdentity| id.traffic_links.roads ^= 1,
        |id: &mut BuildIdentity| id.rival_links.personality_library ^= 1,
        |id: &mut BuildIdentity| id.rival_links.personality ^= 1,
        |id: &mut BuildIdentity| id.ai_links.initial ^= 1,
        |id: &mut BuildIdentity| id.ai_links.source ^= 1,
        |id: &mut BuildIdentity| id.ai_links.destination ^= 1,
    ] {
        let mut changed = install.options();
        mutate(&mut changed.identity);
        assert_ne!(
            first.fingerprint,
            fingerprint(&install.game(), &changed).unwrap()
        );
    }
    assert!(!install.prepare().unwrap().built);
}

#[test]
fn vehicle_profiles_follow_components_links_and_installed_defaults() {
    let install = install_with(&assets());
    let prepared = install.prepare().unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared.path(Kind::VehicleProfiles)).unwrap())
            .unwrap();
    let rows = json["vehicles"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    let definition = vehicle_profiles::Definition::from_json(row).unwrap();
    assert_eq!(definition.blueprint, synthetic::vehicles::BLUEPRINT);
    let info = definition.info.as_ref().unwrap();
    assert_eq!(info.bundle, 0xf123_4567);
    assert_eq!(info.short_name.as_str(), "ID_CAR_SYNTHETIC_SHORT_NAME");
    assert_eq!(definition.asset_type, 20);
    assert_eq!(definition.asset_index, 0);
    assert_eq!(
        row["definitions"],
        serde_json::json!([nfs_frostbite::hex(&guid(0x83))])
    );
    assert_eq!(
        row["parts"],
        serde_json::json!([{"ordinal":2,"variants":2,"networkable":true}])
    );
    assert_eq!(
        row["profile"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "root",
            "chassis",
            "part",
            "triple_nibbles",
            "bool",
            "wheel",
            "wheel",
            "wheel",
            "wheel",
            "index",
            "tuning",
            "appearance",
            "four_bit",
            "tagged",
            "nibbles_guid",
            "mesh",
            "guid_float"
        ]
    );
    // Link order is rim, base; the component graph has base before rim.
    assert_eq!(row["meshes"][0]["extra"], "rim");
    assert_eq!(
        row["meshes"][0]["variants"][0]["bundle"],
        "parts/synthetic/rims"
    );
    assert_eq!(row["meshes"][1]["extra"], "none");
    assert!(row["meshes"][1]["variants"][0]["bundle"].is_null());
    assert_eq!(row["meshes"][1]["variants"].as_array().unwrap().len(), 1);
    assert_eq!(
        row["buses"],
        serde_json::json!([{"path":[],"flags":1},{"path":[0,17],"flags":0}])
    );
    assert_eq!((definition.team, definition.mode), (1, 2));
    assert_eq!(definition.resting.mass, 1000.);
    assert_eq!(definition.resting.front.spring_rate, 120.);
    assert_eq!(definition.resting.rear.spring_progression, 1.5);
    assert_eq!(definition.wheel_baseline, [0.5, 0.625]);
    assert_eq!(definition.appearance.palettes[1][2], [0.75, 0.25, 0.75]);
    assert_eq!(definition.appearance.static_wrap_counts, Some([2, 3]));
    assert_eq!(
        row["health"]["profile"]["points"],
        serde_json::json!([70, 30])
    );
}

#[test]
fn vehicle_profiles_reject_ambiguous_missing_and_cyclic_graphs() {
    use synthetic::{Field, vehicles};
    for case in 0..7 {
        let replacement = vehicles::assets_with(|fields, objects| match case {
            0 => objects[3].0 = "UnrecognizedComponentData",
            1 => objects[1].1.push(("Object", Field::RawPointer(2))),
            2 => objects[0].1[1].1 = Field::RawPointers(vec![0, 3]),
            3 => {
                let Field::Structs(_, links) = &mut fields[3].1 else {
                    panic!()
                };
                links.push(links[0].clone());
            }
            4 => fields[3].1 = Field::Structs("LinkConnection", vec![]),
            5 => objects[18].1[0].1 = Field::Int(0),
            6 => objects[1].1.push(("Excluded", Field::Bool(true))),
            _ => unreachable!(),
        });
        let mut fixture = assets();
        fixture.retain(|a| !replacement.iter().any(|b| a.name == b.name));
        fixture.extend(replacement);
        let error = install_with(&fixture).prepare().unwrap_err();
        assert!(matches!(error, Error::Content(_)), "case {case}: {error:?}");
    }
}

#[test]
fn traffic_profiles_follow_the_constructed_subset_without_player_defaults() {
    use vehicle_profiles::traffic;
    let install = install_with(&synthetic::vehicles::traffic_assets());
    let mut index = assets::AssetIndex::open(&install.game(), &install.options()).unwrap();
    let row = traffic::extract(&mut index, synthetic::vehicles::BUNDLE).unwrap();
    let d = traffic::Definition::from_json(&row).unwrap();
    assert_eq!(d.asset_type, 20);
    assert_eq!(d.asset_index, 0);
    assert_eq!(d.profile.kinds().len(), 12);
    assert_eq!(d.parts, [(2, true)].into());
    assert_eq!(d.resting.mass, 1000.);
    assert_eq!((d.team, d.mode), (1, 2));
    assert_eq!(
        row["health"]["profile"]["points"],
        serde_json::json!([70, 30])
    );
    assert!(row.get("appearance").is_none());
    assert!(row.get("definitions").is_none());
    assert!(vehicle_profiles::Definition::from_json(&row).is_err());
    for field in ["mode", "parts", "asset", "buses", "health"] {
        let mut wrong = row.clone();
        wrong[field] = serde_json::Value::Null;
        assert!(traffic::Definition::from_json(&wrong).is_err(), "{field}");
    }
    for kind in ["rpc", "wheel", "guid_float"] {
        let mut missing = row.clone();
        missing["profile"]
            .as_array_mut()
            .unwrap()
            .retain(|r| r["kind"] != kind);
        assert!(traffic::Definition::from_json(&missing).is_err());
    }
    let player = install_with(&assets());
    let mut player_index = assets::AssetIndex::open(&player.game(), &player.options()).unwrap();
    assert!(traffic::extract(&mut player_index, synthetic::vehicles::BUNDLE).is_err());
}

#[test]
fn startup_resolves_roles_by_name_and_traffic_by_construction_order() {
    let install = install_with(&assets());
    let prepared = install.prepare().unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared.path(Kind::WorldContent)).unwrap()).unwrap();
    assert_eq!(value["version"], 5);
    assert!(value.get("scene_profiles").is_none());
    assert!(value.get("launchers").is_none());
    assert_eq!(
        value["roles"]["customization_timer"],
        serde_json::json!({"bundle":3,"type_id":30,"local_index":1})
    );
    assert_eq!(
        value["roles"]["streaming_gate"],
        serde_json::json!({"bundle":1,"type_id":30,"local_index":0})
    );
    let expected: Vec<_> = (0..10)
        .rev()
        .map(|i| assets::scene_key(&format!("vehicles/runtime/{i}_bundle")))
        .collect();
    assert_eq!(value["roles"]["traffic"], serde_json::json!(expected));
    let names: Vec<_> = value["batches"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["kind"] == "names")
        .flat_map(|b| b["entries"].as_array().unwrap())
        .map(|e| {
            let bytes: Vec<_> = e[1]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_u64().unwrap() as u8)
                .collect();
            String::from_utf8(bytes).unwrap()
        })
        .collect();
    assert_eq!(names.len(), 16);
    assert_eq!(names[6], "vehicles/runtime/0_bundle");
    assert_eq!(names[15], "vehicles/runtime/9_bundle");
}

#[test]
fn startup_derives_bank_position_and_rejects_ambiguous_banks() {
    use synthetic::Field;
    for count in [0, 1, 2] {
        let replacements = synthetic::garage_layout_with(|_, _, fields, objects| {
            let first = objects.len() as u32 + 2;
            let Field::RawPointers(pointers) = &mut fields[1].1 else {
                panic!()
            };
            for i in 0..count {
                objects.push(("BankEntityData", vec![]));
                // Place the bank before every pre-existing scene child.
                pointers.insert(0, first + i);
            }
        });
        let mut fixture = assets();
        fixture.retain(|a| !replacements.iter().any(|b| b.name == a.name));
        fixture.extend(replacements);
        let mut install = install_with(&fixture);
        install.options.identity.scenes.classes.insert(
            "BankEntityData".into(),
            Some(nfs_world::replication::sublevel::Kind::Rpc),
        );
        let mut index = assets::AssetIndex::open(&install.game(), &install.options()).unwrap();
        let scenes = scenes::build(&mut index.assets, &install.options.identity.scenes).unwrap();
        let catalog =
            scenes::Catalog::from_json(&serde_json::from_slice(&scenes).unwrap()).unwrap();
        let prepared = startup::build(&mut index, &catalog);
        if count == 2 {
            assert!(matches!(prepared, Err(Error::Content(_))));
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&prepared.unwrap()).unwrap();
        if count == 0 {
            assert!(value["roles"].get("bank").is_none());
        } else {
            assert_eq!(
                value["roles"]["bank"],
                serde_json::json!({
                    "scene": assets::scene_key("levels/runtime/gameplay"), "serializer":1
                })
            );
        }
    }
}

#[test]
fn startup_rejects_missing_required_roles_and_role_assets() {
    for name in [
        "levels/runtime/garage",
        "gameplay/stats/garagestatslogic",
        "gameplay/garageloadlogic",
    ] {
        let mut incomplete = assets();
        incomplete.retain(|a| a.name != name);
        assert!(install_with(&incomplete).prepare().is_err(), "{name}");
    }
    let install = install_with(&assets());
    let mut options = install.options();
    options
        .identity
        .blueprint_class_ids
        .insert("ParticipantLogicBlueprint".into(), 65536);
    assert!(prepare(&install.game(), &install.cache(), &options).is_err());
}

#[test]
fn license_builder_rejects_missing_active_queries_and_bad_names() {
    let mut missing = assets();
    missing.retain(|a| a.name != "queries/synthetic113");
    assert!(install_with(&missing).prepare().is_err());
    let mut malformed = assets();
    let query = malformed
        .iter_mut()
        .find(|a| a.name == "queries/synthetic113")
        .unwrap();
    query.partition = synthetic::PartitionWriter::default().build(
        guid(0x71),
        "EntitlementQuery",
        guid(0xF1),
        vec![
            (
                "Platform",
                synthetic::Field::Enum("GamePlatform", "GamePlatform_Win32", 1),
            ),
            (
                "EntitlementsData",
                synthetic::Field::Structs(
                    "EntitlementData",
                    vec![vec![("License", synthetic::Field::CString("x".repeat(17)))]],
                ),
            ),
        ],
    );
    assert!(install_with(&malformed).prepare().is_err());
}

#[test]
fn reputation_follows_the_calculator_reference_and_checks_values() {
    let install = install_with(&assets());
    let prepared = install.prepare().unwrap();
    let path = prepared.path(Kind::ReputationThresholds);
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["thresholds"], serde_json::json!([0, 13, 57]));
    nfs_services::reputation::Thresholds::load(&path).unwrap();
    for (values, valid) in [
        (vec![0, 8, 51, 100], true),
        (vec![], false),
        (vec![1, 8], false),
        (vec![0, 8, 8], false),
        ((0..257).collect(), false),
    ] {
        let mut changed = assets();
        changed.retain(|a| {
            a.name != crate::reputation::CALCULATOR && a.name != "synthetic/scoring/levels"
        });
        changed.extend(synthetic::reputation(values.clone()));
        let prepared = install_with(&changed).prepare();
        assert_eq!(prepared.is_ok(), valid, "{values:?}");
    }
    let mut missing = assets();
    missing.retain(|a| a.name != "synthetic/scoring/levels");
    assert!(install_with(&missing).prepare().is_err());
    let mut wrong = assets();
    let asset = wrong
        .iter_mut()
        .find(|a| a.name == "synthetic/scoring/levels")
        .unwrap();
    asset.partition = synthetic::PartitionWriter::default().build(
        synthetic::reference(0x78).0,
        "IntScoreToIntLevelAsset",
        synthetic::reference(0x78).1,
        vec![("LevelThresholds", synthetic::Field::Float(5.0))],
    );
    assert!(install_with(&wrong).prepare().is_err());
}

#[test]
fn scope_builder_rejects_missing_modes_and_wrong_id_types() {
    let mut missing = assets();
    missing.retain(|a| a.name != "modes/synthetic116");
    assert!(install_with(&missing).prepare().is_err());
    let mut malformed = assets();
    let mode = malformed
        .iter_mut()
        .find(|a| a.name == "modes/synthetic116")
        .unwrap();
    mode.partition = synthetic::PartitionWriter::default().build(
        guid(0x74),
        "GameModeId",
        guid(0xF4),
        vec![("IdHash", synthetic::Field::Float(10.0))],
    );
    assert!(install_with(&malformed).prepare().is_err());
}

#[test]
fn second_prepare_reuses_and_a_damaged_entry_is_rebuilt() {
    let install = install_with(&assets());
    let first = install.prepare().unwrap();
    let second = install.prepare().unwrap();
    assert!(!second.built);
    assert_eq!(first.directory, second.directory);
    let template = first.path(Kind::WorldMacTemplate);
    std::fs::write(&template, b"tampered").unwrap();
    let third = install.prepare().unwrap();
    assert!(third.built);
    assert_eq!(std::fs::read(template).unwrap(), install.template);
}

#[test]
fn same_size_wrong_output_key_is_rebuilt_without_panicking() {
    let install = install_with(&assets());
    let first = install.prepare().unwrap();
    let path = first.directory.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let outputs = manifest["outputs"].as_object_mut().unwrap();
    let item = outputs.remove("item_content").unwrap();
    outputs.insert("unknown".into(), item);
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(install.prepare().unwrap().built);
    assert!(!install.prepare().unwrap().built);
}

#[test]
fn bundle_manifest_changes_cannot_reuse_a_stale_entry() {
    let install = install_with(&assets());
    let first = install.prepare().unwrap();
    let path = install.game().join("Data/Win32/synthetic.sb");
    let mut bytes = std::fs::read(&path).unwrap();
    let name = b"items/gameitemsystem";
    let offset = bytes.windows(name.len()).position(|b| b == name).unwrap();
    // Keep all offsets, sizes and catalogue digests intact.
    bytes[offset + name.len() - 1] = b'x';
    std::fs::write(path, bytes).unwrap();
    assert_ne!(
        first.fingerprint,
        fingerprint(&install.game(), &install.options()).unwrap()
    );
    assert!(matches!(install.prepare(), Err(Error::Content(_))));
    assert!(first.directory.exists());
}

#[test]
fn record_changes_cannot_reuse_a_stale_entry_but_unconsumed_bytes_can() {
    use std::io::{Seek, SeekFrom, Write};
    let install = install_with(&assets());
    let first = install.prepare().unwrap();
    let path = install.game().join("Data/cas_01.cas");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"unreferenced archive bytes").unwrap();
    drop(file);
    assert_eq!(
        first.fingerprint,
        fingerprint(&install.game(), &install.options()).unwrap()
    );
    assert!(!install.prepare().unwrap().built);
    // Alter the first record's codec without changing any index or file size.
    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(4)).unwrap();
    file.write_all(&[127]).unwrap();
    drop(file);
    assert_ne!(
        first.fingerprint,
        fingerprint(&install.game(), &install.options()).unwrap()
    );
    assert!(matches!(install.prepare(), Err(Error::Container(_))));
}

#[test]
fn an_entry_with_unexpected_files_is_not_deleted() {
    let install = install_with(&assets());
    let prepared = install.prepare().unwrap();
    std::fs::write(prepared.path(Kind::ItemContent), b"{}").unwrap();
    std::fs::write(prepared.directory.join("notes.txt"), b"keep").unwrap();
    assert!(matches!(install.prepare(), Err(Error::Cache(_))));
    assert!(prepared.directory.join("notes.txt").exists());
}

#[test]
fn a_changed_installation_gets_a_new_entry() {
    let install = install_with(&assets());
    let first = install.prepare().unwrap();
    let mut catalog = std::fs::read(install.game().join("Data/cas.cat")).unwrap();
    catalog.extend([0u8; 32]);
    std::fs::write(install.game().join("Data/cas.cat"), catalog).unwrap();
    let second = install.prepare().unwrap();
    assert!(second.built);
    assert_ne!(first.fingerprint.digest, second.fingerprint.digest);
    assert!(first.directory.exists());
}

#[test]
fn an_unsupported_executable_is_rejected_before_reading_content() {
    let install = install_with(&assets());
    let mut options = install.options();
    options.identity.executable_sha256 = "0".repeat(64);
    assert!(matches!(
        prepare(&install.game(), &install.cache(), &options),
        Err(Error::UnsupportedExecutable { .. })
    ));
    assert!(!install.cache().exists());
}

#[test]
fn a_wrong_template_digest_or_address_fails() {
    let install = install_with(&assets());
    let mut options = install.options();
    options.identity.mac_template_sha256 = "0".repeat(64);
    assert!(matches!(
        prepare(&install.game(), &install.cache(), &options),
        Err(Error::Content(_))
    ));
    let mut options = install.options();
    options.identity.mac_template_rva = 0x1000 + 0x200 - 32;
    assert!(matches!(
        prepare(&install.game(), &install.cache(), &options),
        Err(Error::Content(_))
    ));
}

#[test]
fn missing_or_inconsistent_content_fails_without_publishing() {
    let mut without_system = assets();
    without_system.retain(|a| a.name != items::ITEM_SYSTEM);
    let install = install_with(&without_system);
    assert!(matches!(install.prepare(), Err(Error::Content(_))));
    let entries: Vec<_> = std::fs::read_dir(install.cache()).unwrap().collect();
    assert!(entries.is_empty());

    let mut without_table = assets();
    without_table.retain(|a| a.name != "tables/tickettable");
    assert!(matches!(
        install_with(&without_table).prepare(),
        Err(Error::Content(_))
    ));

    let mut wrong_secondary = assets();
    let (name, secondary, count) = required_tables()[0];
    wrong_secondary.push(table(0, name, secondary.is_none(), count));
    wrong_secondary.swap_remove(
        wrong_secondary
            .iter()
            .position(|a| a.name == format!("tables/{}", name.to_lowercase()))
            .unwrap(),
    );
    assert!(matches!(
        install_with(&wrong_secondary).prepare(),
        Err(Error::Content(_))
    ));

    let mut unknown_class = assets();
    unknown_class[0] = item(1, "HovercraftItemData", vec![], vec![], vec![]);
    assert!(matches!(
        install_with(&unknown_class).prepare(),
        Err(Error::Content(_))
    ));

    let mut long_plate = assets();
    long_plate[2] = vehicle("TOOLONGPLATE");
    match install_with(&long_plate).prepare() {
        Err(Error::Content(message)) => assert!(message.contains("eight bytes"), "{message}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn dangling_item_reference_is_rejected() {
    let mut dangling = assets();
    dangling[1] = item(2, "BodyKitItemData", vec![99], vec![], vec![]);
    assert!(matches!(
        install_with(&dangling).prepare(),
        Err(Error::Content(_))
    ));
}

#[test]
fn fingerprint_lists_inputs_and_is_stable() {
    let install = install_with(&assets());
    let a = fingerprint(&install.game(), &install.options()).unwrap();
    let b = fingerprint(&install.game(), &install.options()).unwrap();
    assert_eq!(a, b);
    let paths: Vec<_> = a.inputs.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            EXECUTABLE,
            "Data/layout.toc",
            "Data/cas.cat",
            "Data/Win32/synthetic.toc",
            "Data/Win32/synthetic.sb",
            "Data/cas_01.cas"
        ]
    );
}
