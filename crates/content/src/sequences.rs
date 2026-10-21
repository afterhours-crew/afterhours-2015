// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    Invalid,
    Limit,
    Io,
}
mod extract;
mod rules;
pub(crate) use extract::build;
#[cfg(test)]
#[path = "sequences/extract_tests.rs"]
mod extract_tests;
use nfs_world::{
    arrival::Arrivals,
    content::bindings::name_key,
    replication::entity::creation::{Asset, Catalog, Content},
    sequences::{Component, Definition, Layout, Sequences},
};
pub use rules::Rules;
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};

#[derive(Clone, Debug)]
pub struct SequenceContent {
    pub definition: Definition,
    pub content: Content,
    /// Optional arrive sequence after garage exit (`nfs_world::arrival`).
    pub arrive: Option<(Definition, Content)>,
    pub spline: Option<nfs_world::replication::spline::Definition>,
    pub returning: Option<ReturnSequence>,
    pub crash: Option<CrashSequence>,
}
#[derive(Clone, Debug)]
pub struct CrashSequence {
    pub definition: nfs_world::crash::Definition,
    pub content: Content,
    pub duration_ms: u64,
}
#[derive(Clone, Debug)]
pub struct ReturnSequence {
    pub definition: Definition,
    pub content: Content,
    pub spline: nfs_world::replication::spline::Definition,
    pub duration_ms: u64,
}
impl SequenceContent {
    pub fn load(path: &Path) -> Result<Self, ParseError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ParseError::Io)?
            .take(16385)
            .read_to_end(&mut bytes)
            .map_err(|_| ParseError::Io)?;
        if bytes.len() > 16384 {
            return Err(ParseError::Limit);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| ParseError::Invalid)?)
    }
    pub fn from_json(v: &Value) -> Result<Self, ParseError> {
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
        if v.get("returning").is_some() {
            expected.push("returning");
        }
        if v.get("crash").is_some() {
            expected.push("crash");
        }
        keys(v, &expected)?;
        if v["format"] != "nfs-waiting-sequence"
            || !matches!(v["version"].as_u64(), Some(1 | 2))
            || v["build_sha256"]
                != "92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df"
            || v["blueprint"] != "Gameplay/WaitingForGarageSequence"
        {
            return Err(ParseError::Invalid);
        }
        let (definition, content) = sequence(v, Layout::Waiting)?;
        Sequences::new(definition, content.clone()).map_err(|_| ParseError::Invalid)?;
        let mut spline = None;
        let arrive = match v.get("arrive") {
            None => None,
            Some(a) => {
                let mut arrive_keys = vec![
                    "blueprint",
                    "asset",
                    "catalog",
                    "manager",
                    "channel",
                    "embedded_bus",
                ];
                if a.get("spline").is_some() {
                    arrive_keys.push("spline");
                }
                keys(a, &arrive_keys)?;
                if let Some(value) = a.get("spline") {
                    if v["version"] == 1 {
                        keys(value, &["asset", "catalog", "base_value", "flag"])?;
                        // Legacy documents stored a captured goal counter as content.
                        // Validate it for compatibility, but allocate from owned state.
                        if uint::<u8>(&value["base_value"])? >= 64 {
                            return Err(ParseError::Invalid);
                        }
                    } else {
                        keys(value, &["asset", "catalog", "flag"])?;
                    }
                    let (asset, catalog) = asset_catalog(value)?;
                    let definition = nfs_world::replication::spline::Definition {
                        asset,
                        catalog,
                        flag: value["flag"].as_bool().ok_or(ParseError::Invalid)?,
                    };
                    definition.validate().map_err(|_| ParseError::Invalid)?;
                    spline = Some(definition);
                }
                if a["blueprint"] != "Gameplay/Tests/GarageExitSequence" {
                    return Err(ParseError::Invalid);
                }
                let parsed = sequence(a, Layout::GarageExit)?;
                Arrivals::new(parsed.0, parsed.1.clone()).map_err(|_| ParseError::Invalid)?;
                Some(parsed)
            }
        };
        let returning = v
            .get("returning")
            .map(|r| {
                keys(
                    r,
                    &[
                        "blueprint",
                        "asset",
                        "catalog",
                        "manager",
                        "channel",
                        "embedded_bus",
                        "spline",
                        "duration_ms",
                        "presentation",
                    ],
                )?;
                if r["blueprint"] != "Gameplay/Tests/GarageEntrySequence" {
                    return Err(ParseError::Invalid);
                }
                let (definition, content) = sequence(r, Layout::GarageEntry)?;
                let duration_ms = uint(&r["duration_ms"])?;
                let s = &r["spline"];
                keys(s, &["asset", "catalog", "flag"])?;
                let (asset, catalog) = asset_catalog(s)?;
                let spline = nfs_world::replication::spline::Definition {
                    asset,
                    catalog,
                    flag: s["flag"].as_bool().ok_or(ParseError::Invalid)?,
                };
                Arrivals::returning(definition, content.clone(), duration_ms)
                    .and_then(|a| a.with_spline(spline.clone()))
                    .map_err(|_| ParseError::Invalid)?;
                Ok(ReturnSequence {
                    definition,
                    content,
                    spline,
                    duration_ms,
                })
            })
            .transpose()?;
        let crash = v
            .get("crash")
            .map(|c| {
                keys(
                    c,
                    &[
                        "blueprint",
                        "asset",
                        "catalog",
                        "manager",
                        "embedded_bus",
                        "duration_ms",
                    ],
                )?;
                if c["blueprint"] != "Gameplay/Sequences/CrashedSequence" {
                    return Err(ParseError::Invalid);
                }
                let (asset, catalog) = asset_catalog(c)?;
                let content =
                    Content::new(vec![catalog], vec![(asset, nfs_world::crash::profile())])
                        .map_err(|_| ParseError::Invalid)?;
                let definition = nfs_world::crash::Definition {
                    asset,
                    manager: component(&c["manager"])?,
                    embedded_bus: uint(&c["embedded_bus"])?,
                };
                let duration_ms = uint(&c["duration_ms"])?;
                nfs_world::crash::Crashes::new(definition, content.clone(), duration_ms)
                    .map_err(|_| ParseError::Invalid)?;
                Ok(CrashSequence {
                    definition,
                    content,
                    duration_ms,
                })
            })
            .transpose()?;
        Ok(Self {
            definition,
            content,
            arrive,
            spline,
            returning,
            crash,
        })
    }
}
/// Asset, catalog, manager, channel and bus of a sequence definition.
fn asset_catalog(v: &Value) -> Result<(Asset, Catalog), ParseError> {
    keys(&v["asset"], &["bundle", "type_id", "local_index"])?;
    let asset = Asset {
        bundle: uint(&v["asset"]["bundle"])?,
        type_id: uint(&v["asset"]["type_id"])?,
        local_index: uint(&v["asset"]["local_index"])?,
    };
    let entries = v["catalog"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 4096)
        .ok_or(ParseError::Invalid)?;
    let entries = entries
        .iter()
        .map(|e| {
            let a = e
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or(ParseError::Invalid)?;
            Ok((uint(&a[0])?, uint(&a[1])?))
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    Ok((
        asset,
        Catalog::new(asset.bundle, &entries).map_err(|_| ParseError::Invalid)?,
    ))
}
fn sequence(v: &Value, layout: Layout) -> Result<(Definition, Content), ParseError> {
    let (asset, catalog) = asset_catalog(v)?;
    let content = Content::new(vec![catalog], vec![(asset, layout.profile())])
        .map_err(|_| ParseError::Invalid)?;
    let definition = Definition {
        presentation: (layout == Layout::GarageEntry)
            .then(|| component(&v["presentation"]))
            .transpose()?,
        layout,
        asset,
        manager: component(&v["manager"])?,
        channel: component(&v["channel"])?,
        embedded_bus: uint(&v["embedded_bus"])?,
    };
    Ok((definition, content))
}
fn keys(v: &Value, expected: &[&str]) -> Result<(), ParseError> {
    let map = v.as_object().ok_or(ParseError::Invalid)?;
    if map.len() != expected.len() || !expected.iter().all(|k| map.contains_key(*k)) {
        return Err(ParseError::Invalid);
    }
    Ok(())
}
fn uint<T: TryFrom<u64>>(v: &Value) -> Result<T, ParseError> {
    v.as_u64()
        .and_then(|n| T::try_from(n).ok())
        .ok_or(ParseError::Invalid)
}
fn component(v: &Value) -> Result<Component, ParseError> {
    keys(v, &["scene", "component_index"])?;
    let scene = v["scene"]
        .as_str()
        .filter(|s| s.is_ascii() && s.len() <= 256)
        .ok_or(ParseError::Invalid)?;
    let index = uint::<usize>(&v["component_index"])?;
    if !(1..512).contains(&index) {
        return Err(ParseError::Invalid);
    }
    Ok(Component {
        scene_key: name_key(scene.as_bytes()).map_err(|_| ParseError::Invalid)?,
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
            ("/version", json!(3)),
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
        assert!(plain.arrive.is_none());
        v["arrive"] = json!({"blueprint":"Gameplay/Tests/GarageExitSequence","asset":{"bundle":16,"type_id":3424,"local_index":9},"catalog":[[3404,1],[3424,11]],
            "manager":{"scene":"test/gameplay","component_index":3},"channel":{"scene":"test/garage","component_index":23},"embedded_bus":2});
        let c = SequenceContent::from_json(&v).unwrap();
        let (definition, _) = c.arrive.as_ref().unwrap();
        assert_eq!(
            (definition.asset.bundle, definition.asset.local_index),
            (16, 9)
        );
        assert!(
            Arrivals::new(
                c.arrive.as_ref().unwrap().0,
                c.arrive.as_ref().unwrap().1.clone()
            )
            .is_ok()
        );
        for (pointer, bad) in [
            ("/arrive/asset/local_index", json!(11)),
            ("/arrive/embedded_bus", json!(0)),
            ("/arrive/channel/component_index", json!(0)),
        ] {
            let mut invalid = v.clone();
            *invalid.pointer_mut(pointer).unwrap() = bad;
            assert!(SequenceContent::from_json(&invalid).is_err(), "{pointer}");
        }
        v["arrive"]["spline"] = json!({"asset":{"bundle":23,"type_id":5100,"local_index":1},"catalog":[[5100,3]],"base_value":37,"flag":false});
        assert!(SequenceContent::from_json(&v).unwrap().spline.is_some());
        for (pointer, bad) in [
            ("/arrive/spline/asset/local_index", json!(3)),
            ("/arrive/spline/base_value", json!(64)),
            ("/arrive/spline/flag", json!(0)),
        ] {
            let mut invalid = v.clone();
            *invalid.pointer_mut(pointer).unwrap() = bad;
            assert!(SequenceContent::from_json(&invalid).is_err());
        }
        let mut foreign = v.clone();
        foreign["arrive"]["spline"]["actor"] = json!(27);
        assert!(SequenceContent::from_json(&foreign).is_err());
        let legacy = SequenceContent::from_json(&v).unwrap();
        let mut current = v.clone();
        current["version"] = json!(2);
        assert!(SequenceContent::from_json(&current).is_err());
        current["arrive"]["spline"]
            .as_object_mut()
            .unwrap()
            .remove("base_value");
        assert_eq!(
            SequenceContent::from_json(&current).unwrap().spline,
            legacy.spline
        );
        let mut changed_legacy = v.clone();
        changed_legacy["arrive"]["spline"]["base_value"] = json!(0);
        assert_eq!(
            SequenceContent::from_json(&changed_legacy).unwrap().spline,
            legacy.spline
        );
        for name in ["allocation_serial", "selector", "serial"] {
            let mut invalid = current.clone();
            invalid["arrive"]["spline"][name] = json!(1);
            assert!(SequenceContent::from_json(&invalid).is_err());
        }
        let mut fixed = v.clone();
        fixed["arrive"]["fixed_channel"] = json!({"reference": 48, "selector": 8});
        assert!(SequenceContent::from_json(&fixed).is_err());
        let mut invalid = v;
        invalid["arrive"]["blueprint"] = json!("extra");
        assert!(SequenceContent::from_json(&invalid).is_err());
    }
}
