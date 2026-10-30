// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installation-owned scene indexes. Events share immutable serializer and bus
//! tables, while each lookup retains its own selected roots and bindings.
use super::*;
use std::sync::Arc;

const MAX_CACHED_ROWS: usize = 100_000;
const MAX_CACHED_SCENES: usize = 64;

#[derive(Clone, Default)]
pub(crate) struct Cache {
    entries: BTreeMap<Site, (scenes::Rules, Arc<Prepared>)>,
    rows: usize,
}

impl Cache {
    pub fn get(&self, root: &Site, rules: &scenes::Rules) -> Option<Arc<Prepared>> {
        self.entries
            .get(root)
            .and_then(|(stored, value)| (stored == rules).then(|| value.clone()))
    }

    pub fn insert(&mut self, root: Site, rules: scenes::Rules, value: Arc<Prepared>) {
        if let Some((_, old)) = self.entries.remove(&root) {
            self.rows -= old.rows();
        }
        if value.rows() > MAX_CACHED_ROWS {
            return;
        }
        while self.entries.len() >= MAX_CACHED_SCENES || self.rows + value.rows() > MAX_CACHED_ROWS
        {
            let Some((_, (_, old))) = self.entries.pop_first() else {
                break;
            };
            self.rows -= old.rows();
        }
        self.rows += value.rows();
        self.entries.insert(root, (rules, value));
    }
}

type BusOrdinals = BTreeMap<(Key, Vec<Frame>), Option<u32>>;

pub(crate) struct Prepared {
    pub endpoints: BTreeMap<Site, (u32, usize)>,
    buses: Result<BusOrdinals, Error>,
}

impl Prepared {
    pub fn new(nodes: &[Node], instances: &[scenes::Instance], key: u32) -> Result<Self, Error> {
        if nodes.len() != instances.len() || nodes.len() > scenes::MAX_GRAPH_OBJECTS {
            return Err(bad("scene occurrence count differs"));
        }
        let mut endpoints = BTreeMap::new();
        for (node, instance) in nodes.iter().zip(instances) {
            // Keep the independent constructor/order comparison on the first
            // preparation. A cache hit is valid only for this source and rules.
            if node.site.key != (instance.asset.clone(), instance.index) {
                return Err(bad("scene occurrence order differs"));
            }
            if let Some(serializer) = instance.serializer
                && endpoints
                    .insert(node.site.clone(), (key, serializer))
                    .is_some()
            {
                return Err(bad("ambiguous scene occurrence"));
            }
        }
        // Bus validation remains separate: endpoint-only consumers have never
        // required registered-bus flags. Cache its error without hiding it
        // from a consumer that actually requests a bus.
        let buses = (|| {
            let mut buses = BTreeMap::new();
            let mut count = 0u32;
            for node in nodes {
                let object = &node.object;
                if !object.class.ends_with("Blueprint")
                    && !matches!(object.class.as_str(), "LevelData" | "SubWorldData")
                {
                    continue;
                }
                let flags = match object.fields.get("Flags") {
                    Some(Value::UInt16(v)) => u32::from(*v),
                    Some(v) => bits32(v)?,
                    None => return Err(bad("embedded bus flags")),
                };
                if flags & 1 == 0 {
                    continue;
                }
                count = count
                    .checked_add(1)
                    .ok_or_else(|| bad("embedded bus count"))?;
                buses
                    .entry((node.site.key.clone(), node.site.parents.clone()))
                    .and_modify(|ordinal| *ordinal = None)
                    .or_insert(Some(count));
            }
            Ok(buses)
        })();
        Ok(Self { endpoints, buses })
    }

    fn rows(&self) -> usize {
        self.endpoints.len() + self.buses.as_ref().map_or(0, BTreeMap::len)
    }

    pub fn embedded_bus(&self, site: &Site) -> Result<u32, Error> {
        self.buses
            .as_ref()
            .map_err(|_| bad("embedded bus flags"))?
            .get(&(site.bus.clone(), site.parents.clone()))
            .copied()
            .flatten()
            .ok_or_else(|| bad("unregistered or ambiguous endpoint bus"))
    }
}

#[cfg(test)]
mod tests;
