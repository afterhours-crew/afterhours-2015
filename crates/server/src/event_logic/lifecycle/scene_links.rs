// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bind a mode's authored channel occurrences to already owned scene graphs.
//! Serializer indexes are checked against occurrence identities before linking.
use super::links::Registry;
use nfs_content::{
    events::{
        Binding,
        lifecycle::{channels::Channel, scenes::SceneSources},
    },
    scoring::events::Identity,
};
use nfs_world::scoring::Error;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct Scene {
    pub instance: u64,
    pub sources: Arc<SceneSources>,
}

impl Registry {
    pub fn bind_scene_boundaries(&mut self, scenes: &BTreeMap<u32, Scene>) -> Result<(), Error> {
        if scenes.len() > 256 {
            return Err(Error::Bound);
        }
        let mut next = self.clone();
        for (&key, scene) in scenes {
            if key != scene.sources.content_key {
                return Err(Error::InvalidGraph);
            }
            if let Some(boundary) = &scene.sources.boundary {
                let parent = scenes
                    .get(&boundary.parent_key)
                    .ok_or(Error::MissingInput)?;
                next.bind_boundary(
                    next.object(parent.instance, &boundary.reference)?,
                    next.object(scene.instance, &boundary.interface)?,
                )?;
            }
        }
        *self = next;
        Ok(())
    }

    pub fn install_scene(&mut self, scene: &Scene) -> Result<(), Error> {
        self.install(
            scene.instance,
            Arc::new(scene.sources.composition.clone()),
            &scene.sources.channels,
        )
    }

    /// All bindings commit together. A failed later occurrence cannot leave an
    /// earlier reference attached to the wrong event or a stale scene instance.
    pub fn bind_event_channels(
        &mut self,
        instance: u64,
        channels: &[Channel],
        serializers: &[Identity],
        scenes: &BTreeMap<u32, Scene>,
    ) -> Result<(), Error> {
        if channels.len() > 256 || scenes.len() > 256 {
            return Err(Error::Bound);
        }
        let mut next = self.clone();
        for channel in channels {
            let reference = next.object(instance, &channel.reference)?;
            let (owner, identity) = match channel.owner {
                Binding::Scene {
                    content_key,
                    serializer,
                } => {
                    let scene = scenes.get(&content_key).ok_or(Error::MissingInput)?;
                    if scene.sources.content_key != content_key {
                        return Err(Error::InvalidGraph);
                    }
                    (
                        scene.instance,
                        scene
                            .sources
                            .serializers
                            .get(&serializer)
                            .ok_or(Error::MissingInput)?,
                    )
                }
                Binding::GameMode { serializer } => (
                    instance,
                    serializers.get(serializer).ok_or(Error::MissingInput)?,
                ),
                Binding::Participant { .. } => return Err(Error::MissingInput),
            };
            if identity != &channel.definition {
                return Err(Error::InvalidGraph);
            }
            let definition = next.object(owner, identity)?;
            next.bind(reference, definition)?;
        }
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
