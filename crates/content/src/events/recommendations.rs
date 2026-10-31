// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored narrative-to-standard event keys used by recommendation lookups.
//! This map does not generate recommendations, unlock events or inspect players.
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad, bits32},
};
use nfs_frostbite::ebx::Value;
use std::collections::BTreeMap;

const ASSET: &str = "gameplay/static/narrativetostandardeventmap";
const MAX_PAIRS: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NarrativeEventMap {
    pairs: BTreeMap<u32, u32>,
}
impl NarrativeEventMap {
    /// Convert once, as the native lookup does. Unmapped standard events keep
    /// their own key; a mapped target is not recursively converted again.
    pub fn standard_event(&self, event: u32) -> u32 {
        self.pairs.get(&event).copied().unwrap_or(event)
    }
    pub fn pairs(&self) -> &BTreeMap<u32, u32> {
        &self.pairs
    }
}

impl AssetIndex {
    pub fn narrative_event_map(&mut self) -> Result<NarrativeEventMap, Error> {
        read(&mut Graph::new(&mut self.assets), &Site::root(ASSET))
    }

    /// Resolve a details category through the checker's authored metadata
    /// collection, then normalize the selected game-mode ID. Metadata order
    /// matters: the native evaluator retains the last matching event ID.
    pub fn recommendation_event_key(
        &mut self,
        metadata: &str,
        category: u32,
    ) -> Result<u32, Error> {
        if metadata.is_empty() || metadata.len() > 512 {
            return Err(bad("recommendation metadata name bound"));
        }
        let map = self.narrative_event_map()?;
        metadata_key(
            &mut Graph::new(&mut self.assets),
            &Site::root(metadata),
            category,
            &map,
        )
    }
}

pub(crate) fn metadata_key<S: Source>(
    graph: &mut Graph<S>,
    root: &Site,
    category: u32,
    map: &NarrativeEventMap,
) -> Result<u32, Error> {
    let object = graph.object(root)?;
    if object.class != "EventMetaDataCollection" {
        return Err(bad("recommendation metadata collection class"));
    }
    let events = object
        .fields
        .get("Events")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= MAX_PAIRS)
        .ok_or_else(|| bad("recommendation metadata event bound"))?;
    let mut selected = 0;
    for event in events {
        let key = graph
            .source
            .target(
                &root.key.0,
                event
                    .as_pointer()
                    .ok_or_else(|| bad("recommendation metadata event pointer"))?,
            )?
            .ok_or_else(|| bad("recommendation metadata null event"))?;
        let site = Site {
            key,
            ..root.clone()
        };
        if graph.object(&site)?.class != "EventMetaData" {
            return Err(bad("recommendation metadata event class"));
        }
        if super::intro::results::categories(graph, &site)?.contains(&category) {
            let id = graph.target_field(&site, "Id")?;
            let mode = graph.source.object(&id)?;
            if mode.class != "GameModeId" {
                return Err(bad("recommendation metadata game-mode class"));
            }
            selected = bits32(
                mode.fields
                    .get("IdHash")
                    .ok_or_else(|| bad("recommendation metadata game-mode identity"))?,
            )?;
        }
    }
    Ok(map.standard_event(selected))
}

pub(crate) fn read<S: Source>(
    graph: &mut Graph<S>,
    root: &Site,
) -> Result<NarrativeEventMap, Error> {
    let object = graph.object(root)?;
    if object.class != "NarrativeToStandardEventMapAsset" {
        return Err(bad("narrative event map class"));
    }
    let rows = object
        .fields
        .get("Pairs")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= MAX_PAIRS)
        .ok_or_else(|| bad("narrative event map pair bound"))?;
    let mut pairs = BTreeMap::new();
    for row in rows {
        let fields = row
            .as_fields()
            .ok_or_else(|| bad("narrative event map row"))?;
        let mut key = |field| {
            let target = graph
                .source
                .target(
                    &root.key.0,
                    fields
                        .get(field)
                        .and_then(Value::as_pointer)
                        .ok_or_else(|| bad("narrative event map pointer"))?,
                )?
                .ok_or_else(|| bad("narrative event map null target"))?;
            let object = graph.source.object(&target)?;
            if object.class != "GameModeId" {
                return Err(bad("narrative event map target class"));
            }
            bits32(
                object
                    .fields
                    .get("IdHash")
                    .ok_or_else(|| bad("narrative event map target identity"))?,
            )
        };
        // The native loader assigns in authored order. An identical duplicate
        // is harmless; conflicting definitions need explicit investigation.
        let from = key("First")?;
        let to = key("Second")?;
        if pairs
            .insert(from, to)
            .is_some_and(|previous| previous != to)
        {
            return Err(bad("conflicting narrative event map identity"));
        }
    }
    Ok(NarrativeEventMap { pairs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, local, ptr};
    use nfs_frostbite::ebx::{Fields, Pointer};

    fn row(first: Value, second: Value) -> Value {
        Value::Struct(Fields(vec![
            ("First".into(), first),
            ("Second".into(), second),
        ]))
    }
    fn fixture(rows: Vec<Value>) -> Memory {
        let mut source = Memory::default();
        source.put(
            "a",
            0,
            "NarrativeToStandardEventMapAsset",
            &[("Pairs", Value::Array(rows))],
        );
        source.put("a", 1, "GameModeId", &[("IdHash", Value::UInt32(20))]);
        source.put("a", 2, "GameModeId", &[("IdHash", Value::UInt32(30))]);
        source.put("b", 1, "GameModeId", &[("IdHash", Value::UInt32(10))]);
        source
    }
    #[test]
    fn imported_ids_and_single_conversion_preserve_standard_and_unknown_keys() {
        let mut source = fixture(vec![
            row(ptr("b", 1), local(1)),
            row(local(1), local(2)),
            row(ptr("b", 1), local(1)),
        ]);
        let map = read(&mut Graph::new(&mut source), &Site::root("a")).unwrap();
        assert_eq!(map.pairs().len(), 2);
        assert_eq!(map.standard_event(10), 20);
        assert_eq!(map.standard_event(20), 30);
        assert_eq!(map.standard_event(30), 30);
        assert_eq!(map.standard_event(99), 99);
        assert_eq!(map.standard_event(0), 0);
    }
    #[test]
    fn malformed_or_conflicting_mapping_cannot_silently_become_identity() {
        for fault in 0..7 {
            let mut source = fixture(vec![row(ptr("b", 1), local(1))]);
            match fault {
                0 => source.0.get_mut(&("a".into(), 0)).unwrap().class = "Other".into(),
                1 => source.put("a", 0, "NarrativeToStandardEventMapAsset", &[]),
                2 => source = fixture(vec![row(Value::Pointer(Pointer::Null), local(1))]),
                3 => source = fixture(vec![row(local(99), local(1))]),
                4 => source.put("a", 1, "Other", &[("IdHash", Value::UInt32(20))]),
                5 => source.put("a", 1, "GameModeId", &[("IdHash", Value::Bool(true))]),
                _ => source = fixture(vec![row(ptr("b", 1), local(1)), row(ptr("b", 1), local(2))]),
            }
            assert!(
                read(&mut Graph::new(&mut source), &Site::root("a")).is_err(),
                "fault{fault}"
            );
        }
    }
    #[test]
    fn pair_bounds_and_signed_hash_bits_are_preserved() {
        let mut source = fixture(vec![row(ptr("b", 1), local(1)); MAX_PAIRS]);
        source.put("a", 1, "GameModeId", &[("IdHash", Value::Int32(-1))]);
        let map = read(&mut Graph::new(&mut source), &Site::root("a")).unwrap();
        assert_eq!(map.standard_event(10), u32::MAX);
        let mut too_many = fixture(vec![row(ptr("b", 1), local(1)); MAX_PAIRS + 1]);
        assert!(read(&mut Graph::new(&mut too_many), &Site::root("a")).is_err());
        let mut empty = fixture(vec![]);
        assert_eq!(
            read(&mut Graph::new(&mut empty), &Site::root("a"))
                .unwrap()
                .standard_event(10),
            10
        );
    }

    #[test]
    fn metadata_category_selects_game_id_not_category_hash_and_preserves_order() {
        let mut source = fixture(vec![row(ptr("b", 1), local(1))]);
        let map = read(&mut Graph::new(&mut source), &Site::root("a")).unwrap();
        source.put(
            "c",
            0,
            "EventMetaDataCollection",
            &[("Events", Value::Array(vec![local(1), local(2)]))],
        );
        source.put(
            "c",
            1,
            "EventMetaData",
            &[
                ("Categories", Value::Array(vec![local(3)])),
                ("Id", ptr("b", 1)),
            ],
        );
        source.put(
            "c",
            2,
            "EventMetaData",
            &[
                ("Categories", Value::Array(vec![local(4)])),
                ("Id", ptr("a", 2)),
            ],
        );
        source.put(
            "c",
            3,
            "GameModeCategory",
            &[("CategoryHash", Value::UInt32(111))],
        );
        source.put(
            "c",
            4,
            "GameModeCategory",
            &[("CategoryHash", Value::UInt32(222))],
        );
        let query = |source: &mut Memory, category| {
            metadata_key(&mut Graph::new(source), &Site::root("c"), category, &map)
        };
        assert_eq!(query(&mut source, 111).unwrap(), 20);
        assert_eq!(query(&mut source, 222).unwrap(), 30);
        assert_eq!(query(&mut source, 999).unwrap(), 0);
        source.put(
            "c",
            4,
            "GameModeCategory",
            &[("CategoryHash", Value::UInt32(111))],
        );
        assert_eq!(query(&mut source, 111).unwrap(), 30);
        source.put(
            "c",
            0,
            "EventMetaDataCollection",
            &[("Events", Value::Array(vec![local(2), local(1)]))],
        );
        assert_eq!(query(&mut source, 111).unwrap(), 20);
        source.put(
            "b",
            1,
            "WrongIdentityClass",
            &[("IdHash", Value::UInt32(10))],
        );
        assert!(query(&mut source, 111).is_err());
    }
}
