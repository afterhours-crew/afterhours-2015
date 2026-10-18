// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Json, MAX_PERSONAS, MAX_SPLINES, bad, json_keys, number};
use crate::graph::{Graph, Key, Site, Source};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Links {
    pub roads: u32,
    pub provider: u32,
    pub personas: u32,
}
impl Links {
    pub(crate) fn to_json(self) -> Json {
        serde_json::json!({"roads": self.roads, "provider": self.provider, "personas": self.personas})
    }
    pub(crate) fn from_json(v: &Json) -> Result<Self, Error> {
        json_keys(v, &["roads", "provider", "personas"])?;
        let out = Self {
            roads: number(&v["roads"])?,
            provider: number(&v["provider"])?,
            personas: number(&v["personas"])?,
        };
        let ports = BTreeSet::from([out.roads, out.provider, out.personas]);
        if ports.len() != 3 || ports.contains(&0) {
            return Err(bad("link ports"));
        }
        Ok(out)
    }
}

pub(super) struct Linked {
    pub personas: Vec<Key>,
    pub roads: Vec<(String, Key)>,
}

/// Preserve manager/provider connection order independently of network catalog
/// order. A layer shares its scene bus; a nested prefab or subworld has its own
/// interface context. Resolving a matching asset in another instance is unsafe.
pub(super) fn resolve<S: Source>(
    graph: &mut Graph<'_, S>,
    root: Site,
    scenes: &BTreeSet<String>,
    ports: Links,
) -> Result<Linked, Error> {
    let nodes = graph.read(root)?;
    let mut managers = nodes
        .iter()
        .filter(|n| n.object.class == "TrafficManagerEntityData");
    let manager = managers
        .next()
        .ok_or_else(|| bad("missing traffic manager"))?;
    if managers.next().is_some() {
        return Err(bad("ambiguous traffic manager"));
    }
    let providers = graph.links(&manager.site, ports.provider)?;
    if providers.len() != 1 || graph.object(&providers[0].site)?.class != "PersonaSetEntityData" {
        return Err(bad("unsupported traffic persona provider"));
    }
    let mut personas = Vec::new();
    let mut seen = BTreeSet::new();
    for endpoint in graph.links(&providers[0].site, ports.personas)? {
        if graph.object(&endpoint.site)?.class != "PersonaEntityData"
            || !seen.insert(endpoint.site.key.clone())
            || personas.len() >= MAX_PERSONAS
        {
            return Err(bad("persona provider members"));
        }
        personas.push(endpoint.site.key);
    }
    if personas.is_empty() {
        return Err(bad("empty traffic persona provider"));
    }
    let mut roads = Vec::new();
    let mut seen = BTreeSet::new();
    for endpoint in graph.links(&manager.site, ports.roads)? {
        if graph.object(&endpoint.site)?.class != "TrafficSplineEntityData" {
            return Err(bad("traffic road link class"));
        }
        let site = endpoint.site;
        let mut scene = None;
        for bus in std::iter::once(&site.bus).chain(site.parents.iter().rev().map(|f| &f.bus)) {
            if matches!(
                graph.source.object(bus)?.class.as_str(),
                "SubWorldData" | "LevelData"
            ) {
                if !scenes.contains(&bus.0) {
                    return Err(bad("traffic road outside startup scenes"));
                }
                scene = Some(bus.0.clone());
                break;
            }
        }
        let scene = scene.ok_or_else(|| bad("traffic road has no owning scene"))?;
        if !seen.insert((scene.clone(), site.key.clone())) || roads.len() >= MAX_SPLINES {
            return Err(bad("duplicate or excessive traffic roads"));
        }
        roads.push((scene, site.key));
    }
    if roads.is_empty() {
        return Err(bad("no linked traffic roads"));
    }
    Ok(Linked { personas, roads })
}

#[cfg(test)]
mod tests;
