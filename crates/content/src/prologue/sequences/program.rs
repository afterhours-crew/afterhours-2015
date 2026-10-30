// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Sequence execution uses the shared scalar/property interpreter. Native
//! persistence and garage services remain explicit calls to the session owner.
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad},
    scoring::{
        events::{Composition, Identity},
        program,
    },
};
use nfs_world::scoring::{event_sync::Binding, program::Program};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Service {
    Root,
    ParticipantProvider,
    AwardItem {
        definition: [u8; 16],
    },
    ItemSelector,
    VehicleSlotLoader,
    ParticipantEvent,
    StateReporter,
    /// Authored diagnostic commands are retained but not executed by the server.
    Console,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Definition {
    pub program: Program,
    pub composition: Composition,
    pub signals: Vec<Binding>,
    pub services: BTreeMap<u16, Service>,
    pub root: u16,
    pub links: BTreeMap<u16, BTreeMap<u32, super::ServiceLink>>,
}

fn service(class: &str) -> Option<Service> {
    Some(match class {
        "ParticipantStateSequenceEntityData" => Service::Root,
        "StateSequenceParticipantProviderEntityData" => Service::ParticipantProvider,
        "AwardItemTransactionEntityData" => Service::AwardItem {
            definition: [0; 16],
        },
        "ItemSelectorEntityData" => Service::ItemSelector,
        "VehicleSlotLoaderEntityData" => Service::VehicleSlotLoader,
        "ParticipantEventEntityData" => Service::ParticipantEvent,
        "ParticipantProviderStateReporterEntityData" => Service::StateReporter,
        "ConsoleCommandEntityData" => Service::Console,
        _ => return None,
    })
}

fn services<S: Source>(
    source: &mut S,
    name: &str,
    composition: &Composition,
) -> Result<BTreeMap<u16, Service>, Error> {
    let mut graph = Graph::new(source);
    let mut out = BTreeMap::new();
    let rows = graph.read(Site::root(name))?;
    let roots = rows
        .iter()
        .filter(|r| r.object.class == "ParticipantStateSequenceEntityData")
        .collect::<Vec<_>>();
    let [root] = roots.as_slice() else {
        return Err(bad("sequence service root"));
    };
    let providers = graph.links(&root.site, 304_393_993)?;
    let [provider] = providers.as_slice() else {
        return Err(bad("sequence owner provider"));
    };
    if provider.port != 0
        || graph.object(&provider.site)?.class != "StateSequenceParticipantProviderEntityData"
    {
        return Err(bad("sequence owner provider class"));
    }
    for row in rows {
        let Some(mut value) = service(&row.object.class) else {
            continue;
        };
        if matches!(
            value,
            Service::AwardItem { .. }
                | Service::ItemSelector
                | Service::VehicleSlotLoader
                | Service::ParticipantEvent
                | Service::StateReporter
        ) {
            let owners = graph.links(&row.site, 3_062_566_331)?;
            if owners.as_slice() != std::slice::from_ref(provider) {
                return Err(bad("foreign sequence service participant"));
            }
        }
        let id = composition
            .nodes
            .iter()
            .position(|n| n.identity == Identity::from(&row.site) && n.class == row.object.class)
            .and_then(|i| u16::try_from(i).ok())
            .ok_or_else(|| bad("sequence service identity"))?;
        if matches!(value, Service::AwardItem { .. }) {
            let key = graph.target_field(&row.site, "AwardedItem")?;
            let item = graph.source.object(&key)?;
            if item.class != "RaceVehicleItemData" {
                return Err(bad("sequence award item class"));
            }
            value = Service::AwardItem {
                definition: item
                    .guid
                    .filter(|g| *g != [0; 16])
                    .ok_or_else(|| bad("sequence award item identity"))?,
            };
        }
        if out.insert(id, value).is_some() {
            return Err(bad("duplicate sequence service"));
        }
    }
    Ok(out)
}

pub fn extract(index: &mut AssetIndex, name: &str) -> Result<Definition, Error> {
    let (_, bound) = super::extract_bound(index, name, true, None)?;
    let composition = index.scoring_events(name)?;
    let services = services(&mut index.assets, name, &composition)?;
    let roots = services
        .iter()
        .filter_map(|(&id, s)| (*s == Service::Root).then_some(id))
        .collect::<Vec<_>>();
    let [root] = roots.as_slice() else {
        return Err(bad("sequence execution root"));
    };
    let native = services
        .keys()
        .map(|&id| composition.nodes[usize::from(id)].identity.clone())
        .collect::<BTreeSet<_>>();
    let sources = index.scoring_sources_with_native(name, &native)?;
    let program = program::compile_with_native(&sources, &BTreeMap::new(), &native)?;
    let signals = index.scoring_event_sync(name, &composition)?;
    let links = bound
        .into_iter()
        .map(|(identity, links)| {
            let id = composition
                .nodes
                .iter()
                .position(|n| n.identity == identity)
                .and_then(|i| u16::try_from(i).ok())
                .ok_or_else(|| bad("sequence service link identity"))?;
            Ok((id, links))
        })
        .collect::<Result<_, Error>>()?;
    Ok(Definition {
        program,
        composition,
        signals,
        services,
        root: *root,
        links,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local};
    use nfs_frostbite::ebx::Value;
    fn fixture(owner: usize) -> (Memory, Composition) {
        let mut source = Memory::default();
        source.put(
            "a",
            0,
            "ParticipantStateSequenceBlueprint",
            &[
                (
                    "Objects",
                    Value::Array(vec![local(1), local(2), local(3), local(5)]),
                ),
                (
                    "LinkConnections",
                    Value::Array(vec![
                        edge(local(1), 304_393_993, local(2), 0),
                        edge(local(3), 3_062_566_331, local(owner), 0),
                    ]),
                ),
            ],
        );
        source.put("a", 1, "ParticipantStateSequenceEntityData", &[]);
        source.put("a", 2, "StateSequenceParticipantProviderEntityData", &[]);
        source.put(
            "a",
            3,
            "AwardItemTransactionEntityData",
            &[("AwardedItem", local(4))],
        );
        source.put("a", 4, "RaceVehicleItemData", &[]);
        source.0.get_mut(&("a".into(), 4)).unwrap().guid = Some([9; 16]);
        source.put("a", 5, "StateSequenceParticipantProviderEntityData", &[]);
        let nodes = Graph::new(&mut source)
            .read(Site::root("a"))
            .unwrap()
            .into_iter()
            .map(|n| crate::scoring::events::Node {
                identity: Identity::from(&n.site),
                class: n.object.class,
                listeners: None,
                forward: None,
            })
            .collect();
        let composition = Composition {
            nodes,
            buses: vec![],
            bus_entities: vec![],
            connections: vec![],
            properties: vec![],
            links: vec![],
        };
        (source, composition)
    }
    #[test]
    fn award_definition_and_participant_come_from_the_owned_sequence_links() {
        let (mut source, graph) = fixture(2);
        assert!(
            services(&mut source, "a", &graph)
                .unwrap()
                .values()
                .any(|s| *s
                    == Service::AwardItem {
                        definition: [9; 16]
                    })
        );
        let (mut foreign, graph) = fixture(5);
        assert!(
            services(&mut foreign, "a", &graph)
                .unwrap_err()
                .to_string()
                .contains("foreign sequence service participant")
        );
        let (mut source, graph) = fixture(2);
        source.0.get_mut(&("a".into(), 4)).unwrap().guid = None;
        assert!(services(&mut source, "a", &graph).is_err());
        source.0.get_mut(&("a".into(), 4)).unwrap().guid = Some([9; 16]);
        source.0.get_mut(&("a".into(), 4)).unwrap().class = "OtherItemData".into();
        assert!(services(&mut source, "a", &graph).is_err());
    }
}
