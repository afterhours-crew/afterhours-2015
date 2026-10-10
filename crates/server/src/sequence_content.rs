// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;
use nfs_world::{
    arrival::Arrivals,
    content::bindings::name_key,
    replication::entity::creation::{Asset, Catalog, Content},
    sequences::{self, Component, Definition, Sequences},
};
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};

#[derive(Clone, Debug)]
pub struct SequenceContent {
    pub definition: Definition,
    pub content: Content,
    /// Optional arrive sequence after garage exit (`nfs_world::arrival`).
    pub arrive: Option<(Definition, Content)>,
}
impl SequenceContent {
    pub fn load(path: &Path) -> Result<Self, Failure> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| Failure::Output)?
            .take(16385)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Output)?;
        if bytes.len() > 16384 {
            return Err(Failure::BodyLimit);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| Failure::ProfileConfig)?)
    }
    pub fn from_json(v: &Value) -> Result<Self, Failure> {
        let mut expected = vec![
            "format",
            "version",
            "build_sha256",
            "blueprint",
            "asset",
            "catalog",
            "manager",
            "channel",
            "embedded_bus",
        ];
        if v.get("arrive").is_some() {
            expected.push("arrive");
        }
        keys(v, &expected)?;
        if v["format"] != "nfs-waiting-sequence"
            || v["version"] != 1
            || v["build_sha256"]
                != "92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df"
            || v["blueprint"] != "Gameplay/WaitingForGarageSequence"
        {
            return Err(Failure::ProfileConfig);
        }
        let (definition, content) = sequence(v)?;
        Sequences::new(definition, content.clone()).map_err(|_| Failure::ProfileConfig)?;
        let arrive = match v.get("arrive") {
            None => None,
            Some(a) => {
                keys(
                    a,
                    &["asset", "catalog", "manager", "channel", "embedded_bus"],
                )?;
                let parsed = sequence(a)?;
                Arrivals::new(parsed.0, parsed.1.clone()).map_err(|_| Failure::ProfileConfig)?;
                Some(parsed)
            }
        };
        Ok(Self {
            definition,
            content,
            arrive,
        })
    }
    pub fn instance(&self) -> Result<Sequences, nfs_world::replication::Error> {
        Sequences::new(self.definition, self.content.clone())
    }
    pub fn arrivals(&self) -> Option<Result<Arrivals, nfs_world::replication::Error>> {
        self.arrive
            .as_ref()
            .map(|(definition, content)| Arrivals::new(*definition, content.clone()))
    }
}
/// Asset, catalog, manager, channel and bus of a sequence definition.
fn sequence(v: &Value) -> Result<(Definition, Content), Failure> {
    keys(&v["asset"], &["bundle", "type_id", "local_index"])?;
    let asset = Asset {
        bundle: uint(&v["asset"]["bundle"])?,
        type_id: uint(&v["asset"]["type_id"])?,
        local_index: uint(&v["asset"]["local_index"])?,
    };
    let entries = v["catalog"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 4096)
        .ok_or(Failure::ProfileConfig)?;
    let entries = entries
        .iter()
        .map(|e| {
            let a = e
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or(Failure::ProfileConfig)?;
            Ok((uint(&a[0])?, uint(&a[1])?))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    let content = Content::new(
        vec![Catalog::new(asset.bundle, &entries).map_err(|_| Failure::ProfileConfig)?],
        vec![(asset, sequences::profile())],
    )
    .map_err(|_| Failure::ProfileConfig)?;
    let definition = Definition {
        asset,
        manager: component(&v["manager"])?,
        channel: component(&v["channel"])?,
        embedded_bus: uint(&v["embedded_bus"])?,
    };
    Ok((definition, content))
}
fn keys(v: &Value, expected: &[&str]) -> Result<(), Failure> {
    let map = v.as_object().ok_or(Failure::ProfileConfig)?;
    if map.len() != expected.len() || !expected.iter().all(|k| map.contains_key(*k)) {
        return Err(Failure::ProfileConfig);
    }
    Ok(())
}
fn uint<T: TryFrom<u64>>(v: &Value) -> Result<T, Failure> {
    v.as_u64()
        .and_then(|n| T::try_from(n).ok())
        .ok_or(Failure::ProfileConfig)
}
fn component(v: &Value) -> Result<Component, Failure> {
    keys(v, &["scene", "component_index"])?;
    let scene = v["scene"]
        .as_str()
        .filter(|s| s.is_ascii() && s.len() <= 256)
        .ok_or(Failure::ProfileConfig)?;
    let index = uint::<usize>(&v["component_index"])?;
    if !(1..512).contains(&index) {
        return Err(Failure::ProfileConfig);
    }
    Ok(Component {
        scene_key: name_key(scene.as_bytes()).map_err(|_| Failure::ProfileConfig)?,
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn static_boundary_rejects_unknown_fields_invalid_catalog_and_runtime_values() {
        let v = json!({"format":"nfs-waiting-sequence","version":1,"build_sha256":"92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df",
            "blueprint":"Gameplay/WaitingForGarageSequence","asset":{"bundle":9,"type_id":3424,"local_index":0},"catalog":[[3392,1],[3404,2],[3424,2]],
            "manager":{"scene":"test/gameplay","component_index":3},"channel":{"scene":"test/garage","component_index":23},"embedded_bus":2});
        let c = SequenceContent::from_json(&v).unwrap();
        assert_eq!(
            c.definition.manager.scene_key,
            name_key(b"test/gameplay").unwrap()
        );
        for (pointer, bad) in [
            ("/asset/local_index", json!(2)),
            ("/catalog/1/0", json!(3392)),
            ("/catalog/0/1", json!(65535)),
            ("/channel/component_index", json!(512)),
            ("/manager/component_index", json!(0)),
            ("/embedded_bus", json!(0)),
            ("/version", json!(2)),
        ] {
            let mut invalid = v.clone();
            *invalid.pointer_mut(pointer).unwrap() = bad;
            assert!(SequenceContent::from_json(&invalid).is_err(), "{pointer}");
        }
        let mut invalid = v.clone();
        invalid["participant"] = json!(260);
        assert!(SequenceContent::from_json(&invalid).is_err());
        let mut invalid = v;
        invalid["manager"]["selector"] = json!(2);
        assert!(SequenceContent::from_json(&invalid).is_err());
    }
    #[test]
    fn the_arrive_sequence_is_optional_and_validated_like_the_waiting_one() {
        let mut v = json!({"format":"nfs-waiting-sequence","version":1,"build_sha256":"92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df",
            "blueprint":"Gameplay/WaitingForGarageSequence","asset":{"bundle":9,"type_id":3424,"local_index":0},"catalog":[[3392,1],[3404,2],[3424,2]],
            "manager":{"scene":"test/gameplay","component_index":3},"channel":{"scene":"test/garage","component_index":23},"embedded_bus":2});
        let plain = SequenceContent::from_json(&v).unwrap();
        assert!(plain.arrive.is_none() && plain.arrivals().is_none());
        v["arrive"] = json!({"asset":{"bundle":16,"type_id":3424,"local_index":9},"catalog":[[3404,1],[3424,11]],
            "manager":{"scene":"test/gameplay","component_index":3},"channel":{"scene":"test/garage","component_index":23},"embedded_bus":2});
        let c = SequenceContent::from_json(&v).unwrap();
        let (definition, _) = c.arrive.as_ref().unwrap();
        assert_eq!(
            (definition.asset.bundle, definition.asset.local_index),
            (16, 9)
        );
        assert!(c.arrivals().unwrap().is_ok());
        for (pointer, bad) in [
            ("/arrive/asset/local_index", json!(11)),
            ("/arrive/embedded_bus", json!(0)),
            ("/arrive/channel/component_index", json!(0)),
        ] {
            let mut invalid = v.clone();
            *invalid.pointer_mut(pointer).unwrap() = bad;
            assert!(SequenceContent::from_json(&invalid).is_err(), "{pointer}");
        }
        let mut invalid = v;
        invalid["arrive"]["blueprint"] = json!("extra");
        assert!(SequenceContent::from_json(&invalid).is_err());
    }
}
