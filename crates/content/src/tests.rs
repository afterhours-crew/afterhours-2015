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
fn builds_outputs_the_service_loaders_accept() {
    let install = install_with(&assets());
    let prepared = install.prepare().unwrap();
    assert!(prepared.built);
    assert_eq!(
        prepared.directory,
        install.cache().join(&prepared.fingerprint.digest)
    );
    let items = ItemContent::load(&prepared.path(Kind::ItemContent)).unwrap();
    let scenes = scenes::Catalog::load(&prepared.path(Kind::SceneCatalog)).unwrap();
    assert_eq!(scenes.profiles.profiles().count(), 20);
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
    assert!(!install.prepare().unwrap().built);
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
