// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::items::DefinitionClass;
use serde_json::json;

fn fixture() -> (Value, ItemCatalog) {
    let guid = "12".repeat(16);
    let classes = ItemCatalog::new([([0x12; 16], DefinitionClass::RaceVehicleItemData)]).unwrap();
    (
        json!({"format":FORMAT,"version":VERSION,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "parts":{"customization":[{"guid":guid,"class_name":"RaceVehicleItemData","tuning":{},"chassis":{"family":"other"}}],
        "nos":[],"wheels":[],"appearance":[],"mesh":[{"guid":guid,"class_name":"RaceVehicleItemData","kind":"ignore"}]}}),
        classes,
    )
}

#[test]
fn versioned_parts_validate_identity_classes_and_duplicate_rows() {
    let (value, classes) = fixture();
    assert!(
        Catalog::from_json(&value, &classes)
            .unwrap()
            .matches_classes(&classes)
    );
    for key in ["format", "version", "build_sha256"] {
        let mut changed = value.clone();
        changed[key] = json!("other");
        assert_eq!(
            Catalog::from_json(&changed, &classes),
            Err(ParseError::Invalid)
        );
    }
    let mut extra = value.clone();
    extra["extra"] = json!(true);
    assert_eq!(
        Catalog::from_json(&extra, &classes),
        Err(ParseError::Invalid)
    );
    let wrong = ItemCatalog::new([([0x12; 16], DefinitionClass::CurrencyItemData)]).unwrap();
    assert_eq!(Catalog::from_json(&value, &wrong), Err(ParseError::Invalid));
    for section in ["customization", "mesh"] {
        let mut duplicate = value.clone();
        let row = duplicate["parts"][section][0].clone();
        duplicate["parts"][section]
            .as_array_mut()
            .unwrap()
            .push(row);
        assert_eq!(
            Catalog::from_json(&duplicate, &classes),
            Err(ParseError::Invalid)
        );
    }
    let mut oversized = value.clone();
    oversized["parts"]["nos"] = json!(vec![json!({}); nfs_world::items::MAX_DEFINITIONS + 1]);
    assert_eq!(
        Catalog::from_json(&oversized, &classes),
        Err(ParseError::Limit)
    );
}
