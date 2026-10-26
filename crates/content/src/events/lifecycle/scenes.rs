// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed scene topology used by shared event graphs. The same occurrence
//! walk supplies serializer identities, channel definitions and object links.
//! This data does not replace the scene's service owners or execute its graph.
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, bad, scenes::scene_roots},
    scoring::events::{self, Composition, Identity},
};
use std::collections::{BTreeMap, BTreeSet};

mod membership;
pub use membership::{MemberState, Membership};

#[derive(Clone, Debug)]
pub struct Boundary {
    pub parent_key: u32,
    pub reference: Identity,
    pub interface: Identity,
}

#[derive(Clone, Debug)]
pub struct SceneSources {
    pub name: String,
    pub content_key: u32,
    pub boundary: Option<Boundary>,
    pub composition: Composition,
    /// Linked data outside the scene constructor's Objects walk. These have no
    /// activation step merely because a link names them.
    pub linked_data: BTreeSet<Identity>,
    /// Slot zero is the common scene serializer and has no EBX entity.
    pub serializers: BTreeMap<usize, Identity>,
    pub channels: BTreeMap<Identity, super::channels::Definition>,
}

impl AssetIndex {
    /// Scenes named by mode imports and fully resolved channel targets, plus
    /// their ancestors. The longest occurrence prefix identifies the owner;
    /// an asset name alone cannot distinguish repeated prefabs.
    pub fn event_scene_keys(
        &mut self,
        mode: &super::super::GameMode,
    ) -> Result<BTreeSet<u32>, Error> {
        let rules = self.identity.scenes.clone();
        let startup = self.startup_entries(&rules.level)?;
        let roots = scene_roots(&mut Graph::new(&mut self.assets), &rules.level, &startup)?;
        let key = |name: &str| {
            if name == rules.level.to_lowercase() {
                1
            } else {
                crate::assets::scene_key(name)
            }
        };
        let mut keys: BTreeSet<_> = mode.root_bindings.iter().map(|(key, _)| *key).collect();
        keys.extend(
            mode.lifecycle
                .imports
                .values()
                .flatten()
                .map(|state| state.scene_content_key),
        );
        for channel in &mode.lifecycle.bindings.channels {
            if let super::super::Binding::Scene { content_key, .. } = channel.owner {
                keys.insert(content_key);
                for target in channel.links.values().flatten() {
                    let owner = roots
                        .iter()
                        .filter(|(_, root)| {
                            let prefix: Vec<_> = root
                                .parents
                                .iter()
                                .map(|frame| frame.reference.clone())
                                .collect();
                            target.identity.instances.starts_with(&prefix)
                        })
                        .max_by_key(|(_, root)| root.parents.len())
                        .ok_or_else(|| bad("channel target scene owner"))?;
                    keys.insert(key(owner.0));
                }
            }
        }
        let selected: Vec<_> = roots
            .iter()
            .filter(|(name, _)| keys.contains(&key(name)))
            .collect();
        for (_, root) in selected {
            for parent in &root.parents {
                if roots.contains_key(&parent.bus.0) {
                    keys.insert(key(&parent.bus.0));
                }
            }
        }
        Ok(keys)
    }

    /// Read only requested loaded scenes. Unknown scene keys fail rather than
    /// supplying stand-in entities for their imported state or channel links.
    pub fn event_scene_sources(
        &mut self,
        keys: &BTreeSet<u32>,
    ) -> Result<BTreeMap<u32, SceneSources>, Error> {
        if keys.len() > 256 {
            return Err(bad("event scene count bound"));
        }
        let rules = self.identity.scenes.clone();
        let startup = self.startup_entries(&rules.level)?;
        let roots = scene_roots(&mut Graph::new(&mut self.assets), &rules.level, &startup)?;
        let mut result = BTreeMap::new();
        for (name, root) in roots {
            let key = if name == rules.level.to_lowercase() {
                1
            } else {
                crate::assets::scene_key(&name)
            };
            if !keys.contains(&key) {
                continue;
            }
            let boundary = if let Some(frame) = root.parents.last() {
                let mut graph = Graph::new(&mut self.assets);
                let object = graph.object(&root)?;
                let pointer = object
                    .fields
                    .get("Interface")
                    .and_then(nfs_frostbite::ebx::Value::as_pointer)
                    .ok_or_else(|| bad("scene interface pointer"))?;
                graph
                    .source
                    .target(&root.key.0, pointer)?
                    .map(|interface| Boundary {
                        parent_key: if frame.bus.0 == rules.level.to_lowercase() {
                            1
                        } else {
                            crate::assets::scene_key(&frame.bus.0)
                        },
                        reference: Identity {
                            asset: frame.reference.0.clone(),
                            index: frame.reference.1,
                            instances: root.parents[..root.parents.len() - 1]
                                .iter()
                                .map(|frame| frame.reference.clone())
                                .collect(),
                        },
                        interface: Identity {
                            asset: interface.0,
                            index: interface.1,
                            instances: root
                                .parents
                                .iter()
                                .map(|frame| frame.reference.clone())
                                .collect(),
                        },
                    })
            } else {
                None
            };
            let rows = Graph::new(&mut self.assets).read(root.clone())?;
            let (_, occurrences) = crate::scenes::construct(&mut self.assets, &rules, &name, true)?;
            if rows.len() != occurrences.len() {
                return Err(bad("event scene occurrence count"));
            }
            let mut serializers = BTreeMap::new();
            for (row, occurrence) in rows.iter().zip(occurrences) {
                if row.site.key != (occurrence.asset, occurrence.index) {
                    return Err(bad("event scene occurrence order"));
                }
                if let Some(serializer) = occurrence.serializer
                    && (serializer == 0
                        || serializers
                            .insert(serializer, Identity::from(&row.site))
                            .is_some())
                {
                    return Err(bad("event scene serializer identity"));
                }
            }
            let composition =
                events::read_scene(&mut self.assets, root.clone()).map_err(|error| {
                    bad(&format!(
                        "scene {name}, {} constructor rows: {error}",
                        rows.len()
                    ))
                })?;
            let channels = super::channels::definitions_at(
                &mut crate::graph::scenes::ServerScene {
                    source: &mut self.assets,
                },
                root,
            )?;
            let constructed: BTreeSet<_> =
                rows.iter().map(|row| Identity::from(&row.site)).collect();
            let linked_data = composition
                .nodes
                .iter()
                .filter(|node| {
                    node.class != "InterfaceDescriptorData" && !constructed.contains(&node.identity)
                })
                .map(|node| node.identity.clone())
                .collect();
            let identities: BTreeSet<_> = composition.nodes.iter().map(|n| &n.identity).collect();
            if serializers
                .values()
                .chain(channels.keys())
                .any(|identity| !identities.contains(identity))
            {
                return Err(bad("event scene identity outside composition"));
            }
            if result
                .insert(
                    key,
                    SceneSources {
                        name,
                        content_key: key,
                        boundary,
                        composition,
                        linked_data,
                        serializers,
                        channels,
                    },
                )
                .is_some()
            {
                return Err(bad("event scene key collision"));
            }
        }
        if result.len() != keys.len() {
            return Err(bad("event scene is not loaded"));
        }
        Ok(result)
    }
}
