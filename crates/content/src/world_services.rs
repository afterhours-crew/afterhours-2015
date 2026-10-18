// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed service roles for local free roam. Volume eligibility remains a
//! separate progression policy; no race/event volumes are enabled wholesale.
use crate::{
    Error,
    assets::{AssetIndex, scene_key},
    graph::{Graph, Site as Definition},
};
use nfs_world::world_activation::{Catalog, Site};

fn bad(message: &str) -> Error {
    Error::Content(format!("world services: {message}"))
}
fn only<T>(mut values: Vec<T>, role: &str) -> Result<T, Error> {
    if values.len() != 1 {
        return Err(bad(&format!("missing or ambiguous {role}")));
    }
    Ok(values.remove(0))
}

impl AssetIndex {
    /// Resolve presence, player location and controlled-vehicle streaming from
    /// the scene's typed definitions and link contracts, not wire ordinals.
    pub fn local_world_services(&mut self, scene: &str) -> Result<Catalog, Error> {
        let bindings = self.scene_bindings(scene)?;
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Definition::root(scene))?;
        let bind = |site: &Definition, class: &str| -> Result<Site, Error> {
            let b = only(
                bindings
                    .iter()
                    .filter(|b| b.asset == site.key.0 && b.object == site.key.1 && b.class == class)
                    .collect(),
                class,
            )?;
            Ok(Site {
                scene_key: scene_key(scene),
                serializer: b.serializer,
            })
        };
        let mut presence = Vec::new();
        for node in nodes
            .iter()
            .filter(|n| n.object.class == "SetClientNotificationEntityData")
        {
            // ParticipantSet link. The world-presence notification consumes the
            // intersection set; the distinct input-mode notification consumes
            // a participant Boolean adapter and must not be selected.
            let linked = graph.links(&node.site, 2664990019)?;
            if linked.len() == 1
                && graph.object(&linked[0].site)?.class == "ParticipantSetPredicateEntityData"
            {
                presence.push(bind(&node.site, "SetClientNotificationEntityData")?);
            }
        }
        let tracker = only(
            nodes
                .iter()
                .filter(|n| n.object.class == "ControllableOutOfBoundsTrackerEntityData")
                .collect(),
            "controlled-vehicle tracker",
        )?;
        let streamed = only(
            graph.links(&tracker.site, 566010025)?,
            "controlled-vehicle streamed collection",
        )?;
        let location = only(
            nodes
                .iter()
                .filter(|n| n.object.class == "ParticipantLocationServiceEntityData")
                .collect(),
            "location service",
        )?;
        let catalog = Catalog {
            volumes: Vec::new(),
            presence: only(presence, "world presence")?,
            streamed: bind(&streamed.site, "StreamedTriggerVolumeCollectionEntityData")?,
            location: bind(&location.site, "ParticipantLocationServiceEntityData")?,
        };
        catalog
            .validate()
            .map_err(|_| bad("invalid service bindings"))?;
        Ok(catalog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::{self, Asset, Field, PartitionWriter, guid};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
    fn fixture(order: Vec<u32>, bad_link: bool) -> (std::path::PathBuf, AssetIndex) {
        let classes = [
            "SetClientNotificationEntityData",
            "ParticipantSetPredicateEntityData",
            "ControllableOutOfBoundsTrackerEntityData",
            "StreamedTriggerVolumeCollectionEntityData",
            "ParticipantLocationServiceEntityData",
            "SetClientNotificationEntityData",
            "ParticipantSetBoolPropertyAdapterEntityData",
        ];
        let connection = |from, port, to| {
            vec![
                ("Source", Field::RawPointer(from)),
                ("SourceFieldId", Field::UInt(port)),
                ("Target", Field::RawPointer(to)),
                ("TargetFieldId", Field::UInt(0)),
            ]
        };
        let asset = Asset {
            name: "scene".into(),
            partition: PartitionWriter::default().build_many(
                guid(1),
                "SubWorldData",
                guid(129),
                vec![
                    ("Objects", Field::RawPointers(order)),
                    (
                        "LinkConnections",
                        Field::Structs(
                            "LinkConnection",
                            vec![
                                connection(2, 2664990019, 3),
                                connection(4, 566010025, if bad_link { 6 } else { 5 }),
                                connection(7, 2664990019, 8),
                            ],
                        ),
                    ),
                ],
                classes.iter().map(|c| (*c, vec![])).collect(),
            ),
        };
        let path = std::env::temp_dir().join(format!(
            "afterhours-world-services-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let (options, _) = synthetic::write(&path, &[asset]).unwrap();
        let mut index = AssetIndex::open(&path, &options).unwrap();
        let supported = crate::BuildIdentity::supported();
        for class in classes {
            index
                .identity
                .scenes
                .classes
                .insert(class.into(), supported.scenes.classes[class]);
        }
        (path, index)
    }
    #[test]
    fn service_roles_follow_constructed_order_and_semantic_links() {
        for order in [(2..=8).collect(), (2..=8).rev().collect()] {
            let (path, mut index) = fixture(order, false);
            let catalog = index.local_world_services("scene").unwrap();
            let bindings = index.scene_bindings("scene").unwrap();
            for (site, object) in [
                (catalog.presence, 1),
                (catalog.streamed, 4),
                (catalog.location, 5),
            ] {
                assert_eq!(site.scene_key, scene_key("scene"));
                assert_eq!(
                    bindings
                        .iter()
                        .find(|b| b.serializer == site.serializer)
                        .unwrap()
                        .object,
                    object
                );
            }
            assert!(catalog.volumes.is_empty());
            let profile = index.scene("scene").unwrap().profile;
            catalog
                .validate_profiles(
                    &nfs_world::replication::sublevel::Content::new(vec![(
                        scene_key("scene"),
                        profile,
                    )])
                    .unwrap(),
                )
                .unwrap();
            std::fs::remove_dir_all(path).unwrap();
        }
    }
    #[test]
    fn missing_duplicate_or_wrong_role_targets_are_rejected() {
        for (order, bad_link) in [
            ((2..=8).filter(|i| *i != 6).collect(), false),
            ((2..=8).chain([6]).collect(), false),
            ((2..=8).collect(), true),
            ((3..=8).collect(), false),
        ] {
            let (path, mut index) = fixture(order, bad_link);
            assert!(
                index.local_world_services("scene").is_err(),
                "malformed fixture unexpectedly accepted at {} (bad_link={bad_link})",
                path.display()
            );
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}
