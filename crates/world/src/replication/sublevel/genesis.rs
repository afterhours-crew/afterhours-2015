// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The startup scene tree of the Genesis level.
//!
//! Every official SubLevel creation except the root names the scene it hangs
//! under in its prefix (`Blueprint { ghost: parent scene, sub_id: 1 }`): the
//! eight root children name the root scene, the Gameplay children name the
//! Gameplay scene, and so on down the registration hierarchy. The client
//! creates each sub-level under that parent's bus, which is where the parent
//! level's cross-sub-level link connections live (the Gameplay sub-level wires
//! its Audio child to its ParticipantStateMachine child; the music director
//! resolves that link once when the Audio scene is created). A child created
//! without its parent never sees those links, so the parent is part of the
//! creation, not an optional detail.

use super::{Blueprint, Content, Creation, Error, Rpc, root};
use crate::content::bindings::Bindings;

/// One scene of the startup tree: its creation and the content key of the
/// scene it hangs under (`None` only for the root scene).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scene {
    pub parent_key: Option<u32>,
    pub creation: Creation,
}

impl Scene {
    /// The parent reference the official host writes once the parent scene's
    /// ghost is known: the parent's ghost and embedded bus 1.
    pub fn parent(ghost: u16) -> Blueprint {
        Blueprint { ghost, sub_id: 1 }
    }
}

/// The root scene followed by every registered level, parents before
/// children, each with its parent's content key.
pub fn tree(
    rpc: Rpc,
    bindings: &Bindings,
    content: &Content,
    traffic_keys: &[u32; root::REFERENCES],
) -> Result<Vec<Scene>, Error> {
    root::validate_traffic_keys(traffic_keys)?;
    if content.profile(1)? != root::content()?.profile(1)? {
        return Err(Error::TypeMismatch);
    }
    let mut links = [None; root::REFERENCES];
    for (slot, key) in traffic_keys.iter().copied().enumerate() {
        links[slot] = Some(
            bindings
                .root_child(key)
                .map_err(|_| Error::UnknownObject)?
                .id,
        );
    }
    let serial = rpc.serial;
    let mut result = vec![Scene {
        parent_key: None,
        creation: root::creation(rpc, links)?,
    }];
    for level in bindings.hierarchy() {
        let parent_key = if level.parent == 0 {
            root::CONTENT_KEY
        } else {
            bindings
                .level(level.parent)
                .ok_or(Error::UnknownObject)?
                .content_key
        };
        let values = super::ordinary::unpopulated(content.profile(level.content_key)?)?;
        result.push(Scene {
            parent_key: Some(parent_key),
            creation: super::ordinary::creation(
                level.id,
                level.content_key,
                None,
                serial,
                &values,
                content,
            )?,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        content::{Message, Registration, Registrations, SubLevelNames, bindings::name_key},
        replication::{Initial, players::Players, sublevel},
    };
    use nfs_protocol::world::rpc::Serial;

    fn registration(name: u16, level: u16, parent: u16) -> Registration {
        Registration {
            handle: name,
            next: level,
            region: parent,
            word: u32::from(level) + 100,
            byte: 0,
            flag: false,
            enum3: 0,
            word2: 0,
            bit: false,
            enum2: 0,
            word3: 0,
            text: vec![],
        }
    }

    /// Ten traffic roots, one more root with a child and a grandchild.
    fn fixture() -> (Bindings, Content, [u32; root::REFERENCES]) {
        let mut names = Vec::new();
        let mut entries = Vec::new();
        let mut traffic = [0u32; root::REFERENCES];
        for slot in 0..root::REFERENCES as u16 {
            let name = format!("Traffic{slot}").into_bytes();
            traffic[usize::from(slot)] = name_key(&name).unwrap();
            names.push((slot, name));
            entries.push(registration(slot, slot + 1, 0));
        }
        for (name, level, parent) in [(20u16, 21u16, 0u16), (22, 23, 21), (24, 25, 23)] {
            names.push((name, format!("Level{level}").into_bytes()));
            entries.push(registration(name, level, parent));
        }
        let bindings = Bindings::from_messages(&[
            Message::Names(SubLevelNames {
                word: 0,
                entries: names.clone(),
            }),
            Message::Registrations(Registrations { header: 0, entries }),
        ])
        .unwrap();
        let mut profiles = vec![(
            root::CONTENT_KEY,
            root::content().unwrap().profile(1).unwrap().clone(),
        )];
        for (_, name) in &names {
            profiles.push((
                name_key(name).unwrap(),
                sublevel::Profile::new(false, &[sublevel::Kind::Noop]).unwrap(),
            ));
        }
        (bindings, Content::new(profiles).unwrap(), traffic)
    }

    fn rpc() -> Rpc {
        Rpc {
            selector: 0,
            serial: Serial::new(0).unwrap(),
        }
    }

    fn prefix(record: &crate::replication::Record) -> &sublevel::Prefix {
        match record.initial.as_ref().unwrap() {
            Initial::SubLevel { prefix, .. } => prefix,
            other => panic!("not a scene: {other:?}"),
        }
    }

    #[test]
    fn every_scene_but_the_root_names_its_parent_scene() {
        let (bindings, content, traffic) = fixture();
        let scenes = tree(rpc(), &bindings, &content, &traffic).unwrap();
        assert_eq!(scenes.len(), 1 + root::REFERENCES + 3);
        assert_eq!(scenes[0].parent_key, None);
        assert_eq!(scenes[0].creation.prefix.level_id, 0);
        let by_level: std::collections::BTreeMap<u16, &Scene> = scenes[1..]
            .iter()
            .map(|s| (s.creation.prefix.level_id, s))
            .collect();
        for level in 1..=root::REFERENCES as u16 {
            assert_eq!(by_level[&level].parent_key, Some(root::CONTENT_KEY));
        }
        assert_eq!(by_level[&21].parent_key, Some(root::CONTENT_KEY));
        assert_eq!(
            by_level[&23].parent_key,
            Some(name_key(b"Level21").unwrap())
        );
        assert_eq!(
            by_level[&25].parent_key,
            Some(name_key(b"Level23").unwrap())
        );
        // Parents come before their children, so the tree can be spawned in order.
        let order: Vec<u16> = scenes.iter().map(|s| s.creation.prefix.level_id).collect();
        assert!(order.iter().position(|&l| l == 21) < order.iter().position(|&l| l == 23));
        assert!(order.iter().position(|&l| l == 23) < order.iter().position(|&l| l == 25));
        // Creations carry no ghost yet: the parent's ghost exists only once spawned.
        assert!(scenes.iter().all(|s| s.creation.prefix.blueprint.is_none()));
    }

    #[test]
    fn spawning_the_tree_writes_the_parent_ghost_into_every_child_prefix() {
        let (bindings, content, traffic) = fixture();
        let scenes = tree(rpc(), &bindings, &content, &traffic).unwrap();
        let mut players = Players::default();
        let records = players.create_scene_tree(scenes, &content).unwrap();
        assert_eq!(records.len(), 1 + root::REFERENCES + 3);
        let root = &records[0];
        assert_eq!(prefix(root).level_id, 0);
        assert_eq!(prefix(root).blueprint, None);
        let ghost_of = |level: u16| {
            records
                .iter()
                .find(|r| prefix(r).level_id == level)
                .map(|r| r.id)
                .unwrap()
        };
        for record in &records[1..] {
            let p = prefix(record);
            let expected = match p.level_id {
                21 => root.id,
                23 => ghost_of(21),
                25 => ghost_of(23),
                _ => root.id,
            };
            assert_eq!(
                p.blueprint,
                Some(Scene::parent(expected)),
                "level {}",
                p.level_id
            );
            // The stored prefix is what the record encodes (`Prefix::encode`
            // writes the optional reference); it must survive a re-encode.
            assert!(record.encode().is_ok());
        }
        assert_eq!(players.objects().scene(root::CONTENT_KEY), Some(root.id));
    }

    #[test]
    fn a_child_without_a_spawned_parent_is_refused_and_leaves_the_world_unchanged() {
        let (bindings, content, traffic) = fixture();
        let mut scenes = tree(rpc(), &bindings, &content, &traffic).unwrap();
        // Point the grandchild at a scene nobody creates.
        scenes.last_mut().unwrap().parent_key = Some(0xdead_beef);
        let mut players = Players::default();
        assert_eq!(
            players.create_scene_tree(scenes, &content).unwrap_err(),
            Error::UnknownObject
        );
        // Nothing of the batch is kept: no scene, not even the root.
        assert_eq!(players.objects().iter().count(), 0);
        assert_eq!(players.objects().scene(root::CONTENT_KEY), None);
    }
}
