// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! EventSync network identities, preserving repeated prefab instances.
use super::events::{Composition, Identity};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad, bits32},
};
pub use nfs_world::scoring::event_sync::Binding;

fn read<S: Source>(
    source: &mut S,
    blueprint: &str,
    composition: &Composition,
) -> Result<Vec<Binding>, Error> {
    if composition.buses.len() != composition.bus_entities.len() {
        return Err(bad("event sync bus table"));
    }
    let mut bindings = Vec::new();
    for row in Graph::new(source).read(Site::root(blueprint))? {
        if row.object.class != "EventSyncEntityData" {
            continue;
        }
        let identity = Identity::from(&row.site);
        let node = composition
            .nodes
            .iter()
            .position(|n| n.identity == identity && n.class == row.object.class)
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| bad("event sync node identity"))?;
        let bus = Identity::from(&Site {
            key: row.site.bus.clone(),
            ..row.site.clone()
        });
        let index = composition
            .buses
            .iter()
            .position(|b| *b == bus)
            .ok_or_else(|| bad("event sync bus identity"))?;
        let entity =
            composition.bus_entities[index].ok_or_else(|| bad("event sync non-networked bus"))?;
        let event = bits32(super::field(&row.object.fields, "Flags")?)? & 0x1ff_ffff;
        bindings.push(Binding {
            entity: u32::from(entity),
            event,
            node,
        });
    }
    // Validate identity collisions independently of the eventual instance ghost.
    nfs_world::scoring::event_sync::Router::new(1, &bindings).map_err(|e| bad(&e.to_string()))?;
    Ok(bindings)
}
impl AssetIndex {
    pub fn scoring_event_sync(
        &mut self,
        blueprint: &str,
        composition: &Composition,
    ) -> Result<Vec<Binding>, Error> {
        read(&mut self.assets, blueprint, composition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, local};
    use nfs_frostbite::ebx::Value;
    #[test]
    fn masked_data_identifier_is_distinct_from_composition_node_and_bus() {
        let mut source = Memory::default();
        source.put(
            "a",
            0,
            "ParticipantLogicBlueprint",
            &[("Objects", Value::Array(vec![local(1)]))],
        );
        source.put(
            "a",
            1,
            "EventSyncEntityData",
            &[("Flags", Value::UInt32(0xe000_0037))],
        );
        let root = Identity {
            asset: "a".into(),
            index: 0,
            instances: vec![],
        };
        let mut composition = Composition {
            nodes: vec![super::super::events::Node {
                identity: Identity {
                    index: 1,
                    ..root.clone()
                },
                class: "EventSyncEntityData".into(),
                listeners: None,
                forward: None,
            }],
            buses: vec![root],
            bus_entities: vec![Some(4)],
            connections: vec![],
            properties: vec![],
            links: vec![],
        };
        assert_eq!(
            read(&mut source, "a", &composition).unwrap(),
            vec![Binding {
                entity: 4,
                event: 55,
                node: 0
            }]
        );
        composition.bus_entities[0] = None;
        assert!(read(&mut source, "a", &composition).is_err());
        composition.bus_entities.clear();
        assert!(read(&mut source, "a", &composition).is_err());
    }
}
