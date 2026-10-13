// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_world::replication::Initial as RecordInitial;
use serde_json::json;

#[test]
fn optional_float_and_variant_roles_preserve_absence_and_bits() {
    let mut value = super::tests::content_json();
    value["content"]["profiles"][0]["serializers"] =
        json!(["root", "rpc_reference", "float_property", "rpc_variant"]);
    value["spawn"]["entities"][0]["roles"][3] = json!({"kind":"variant","tag":null,"value":null});
    for bits in [None, Some(0), Some(0x8000_0000u32), Some(0x7fc0_1234)] {
        value["spawn"]["entities"][0]["roles"][2] = json!({"kind":"float","bits":bits});
        let logic = GarageLogic::from_json(&value).unwrap();
        let (mut players, participant) = super::tests::world();
        let mut ghosts = BTreeMap::new();
        let out = logic
            .produce(Which::Spawn, &mut players, participant, &mut ghosts)
            .unwrap();
        let record = out.records.iter().find(|r| r.id == ghosts[&277]).unwrap();
        let Some(RecordInitial::Entity { fields, .. }) = &record.initial else {
            panic!()
        };
        assert_eq!(fields[2], Initial::FloatProperty(bits));
        assert!(matches!(fields[3], Initial::RpcVariant { value: None, .. }));
    }
    for bad in [json!(-1), json!(4294967296u64), json!(1.5), json!("0")] {
        value["spawn"]["entities"][0]["roles"][2]["bits"] = bad;
        assert!(GarageLogic::from_json(&value).is_err());
    }
    value["spawn"]["entities"][0]["roles"][2]["bits"] = Json::Null;
    value["spawn"]["entities"][0]["roles"][3]["value"] = json!(1);
    assert!(GarageLogic::from_json(&value).is_err());
}
