// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed phone messages and the gameplay scene's two message managers.
//!
//! A message manager call names its message by an asset reference: mode 2,
//! the startup bundle of `levels/genesis01/gameplay` and a flat index into
//! that registry's data-container range, which begins with its
//! AchievementData objects and continues with its MessageAsset objects in
//! registry order. Official E1026 906.624 s sends flat 969 =
//! `intro_01_obj05_pho_reminder01_01` (registry message 922 after 47
//! achievements). The range width sets the index width.
use crate::{Error, assets::AssetIndex, graph::*, world::StartupKind};
use nfs_frostbite::ebx::Value;
use std::collections::BTreeMap;

const GAMEPLAY: &str = "levels/genesis01/gameplay";
const MANAGER: &str = "MessageManagerEntityData";
/// MessageManager link to its attachment handlers.
const ATTACHMENT_HANDLERS: u32 = 1_872_310_455;
/// The normal manager carries objective attachments (the intro reminder).
const OBJECTIVE_HANDLER: &str = "ObjectiveMessageAttachmentHandlerEntityData";
/// Width of the flat index on the wire (the range holds 2048–4095 objects).
pub const FLAT_BITS: u32 = 12;
const MAX_MESSAGES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub name: String,
    /// The message's HashId: the StaticMessage table's `Asset` value.
    pub hash: u32,
    /// The wire flat index.
    pub flat: u16,
    pub persisted: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Catalog {
    /// Startup bundle number of the gameplay registry.
    pub bundle: u16,
    messages: Vec<Message>,
    by_key: BTreeMap<(String, usize), usize>,
    by_flat: BTreeMap<u16, usize>,
    by_hash: BTreeMap<u32, usize>,
}
impl Catalog {
    pub fn new(bundle: u16, rows: Vec<((String, usize), Message)>) -> Result<Self, Error> {
        if bundle == 0 || bundle > 2047 || rows.len() > MAX_MESSAGES {
            return Err(bad("message catalog bound"));
        }
        let mut catalog = Self {
            bundle,
            ..Self::default()
        };
        for (key, message) in rows {
            let i = catalog.messages.len();
            if u32::from(message.flat) >= 1 << FLAT_BITS
                || catalog.by_key.insert(key, i).is_some()
                || catalog.by_flat.insert(message.flat, i).is_some()
                || catalog.by_hash.insert(message.hash, i).is_some()
            {
                return Err(bad("duplicate or unbounded message identity"));
            }
            catalog.messages.push(message);
        }
        Ok(catalog)
    }
    pub fn get(&self, partition: &str, object: usize) -> Option<&Message> {
        self.by_key
            .get(&(partition.to_lowercase(), object))
            .map(|&i| &self.messages[i])
    }
    pub fn by_flat(&self, flat: u16) -> Option<&Message> {
        self.by_flat.get(&flat).map(|&i| &self.messages[i])
    }
    pub fn by_hash(&self, hash: u32) -> Option<&Message> {
        self.by_hash.get(&hash).map(|&i| &self.messages[i])
    }
    pub fn len(&self) -> usize {
        self.messages.len()
    }
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Gameplay scene serializers of the two managers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Managers {
    pub scene_key: u32,
    /// The normal manager (StaticMessage table).
    pub normal: usize,
    /// The underground manager (UgStaticMessages and DynamicMessage tables).
    pub underground: usize,
}

impl AssetIndex {
    pub fn message_catalog(&mut self) -> Result<Catalog, Error> {
        let level = self.identity.scenes.level.clone();
        let bundle = self
            .startup_entries(&level)?
            .iter()
            .position(|e| e.kind == StartupKind::SubLevel && e.name.eq_ignore_ascii_case(GAMEPLAY))
            .and_then(|i| u16::try_from(i + 1).ok())
            .ok_or_else(|| bad("gameplay bundle is not loaded"))?;
        let counts = self.registered_class_counts(GAMEPLAY)?;
        let achievements = counts.get("AchievementData").copied().unwrap_or(0);
        let range: usize = achievements
            + counts.get("MessageAsset").copied().unwrap_or(0)
            + counts
                .iter()
                .filter(|(class, _)| class.ends_with("AttachmentAsset"))
                .map(|(_, n)| *n)
                .sum::<usize>();
        // The reader's width is the bit length of the range's object count.
        if usize::BITS - range.leading_zeros() != FLAT_BITS {
            return Err(bad("message index width"));
        }
        let mut rows = vec![];
        for (local, (partition, object)) in self
            .registered_objects(GAMEPLAY, "MessageAsset")?
            .into_iter()
            .enumerate()
        {
            let fields = self
                .assets
                .objects(&partition)?
                .get(object)
                .ok_or_else(|| bad("message asset object"))?
                .fields
                .clone();
            let hash = bits32(fields.get("HashId").ok_or_else(|| bad("message hash"))?)?;
            let persisted = matches!(fields.get("Persisted"), Some(Value::Bool(true)));
            let flat = u16::try_from(achievements + local).map_err(|_| bad("message index"))?;
            rows.push((
                (partition.to_lowercase(), object),
                Message {
                    name: partition.to_lowercase(),
                    hash,
                    flat,
                    persisted,
                },
            ));
        }
        Catalog::new(bundle, rows)
    }

    /// The HashId of one MessageAsset object (a sender's target).
    pub fn message_hash(&mut self, partition: &str, object: usize) -> Result<u32, Error> {
        let object = self
            .assets
            .objects(partition)?
            .get(object)
            .ok_or_else(|| bad("message asset object"))?;
        if object.class != "MessageAsset" {
            return Err(bad("message sender target class"));
        }
        bits32(
            object
                .fields
                .get("HashId")
                .ok_or_else(|| bad("message hash"))?,
        )
    }

    pub fn message_managers(&mut self) -> Result<Managers, Error> {
        let bindings: Vec<_> = self
            .scene_bindings(GAMEPLAY)?
            .into_iter()
            .filter(|b| b.class == MANAGER)
            .collect();
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(GAMEPLAY))?;
        let (mut normal, mut underground) = (None, None);
        for binding in &bindings {
            let mut found = nodes.iter().filter(|n| {
                n.site.key.1 == binding.object && n.site.key.0.eq_ignore_ascii_case(&binding.asset)
            });
            let (Some(node), None) = (found.next(), found.next()) else {
                return Err(bad("message manager occurrence"));
            };
            let mut objective = false;
            for handler in graph.links(&node.site, ATTACHMENT_HANDLERS)? {
                objective |= graph.object(&handler.site)?.class == OBJECTIVE_HANDLER;
            }
            let slot = if objective {
                &mut normal
            } else {
                &mut underground
            };
            if slot.replace(binding.serializer).is_some() {
                return Err(bad("ambiguous message managers"));
            }
        }
        match (normal, underground, bindings.len()) {
            (Some(normal), Some(underground), 2) => Ok(Managers {
                scene_key: crate::assets::scene_key(GAMEPLAY),
                normal,
                underground,
            }),
            _ => Err(bad("gameplay message managers")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message(name: &str, hash: u32, flat: u16) -> ((String, usize), Message) {
        (
            (name.into(), 0),
            Message {
                name: name.into(),
                hash,
                flat,
                persisted: true,
            },
        )
    }
    #[test]
    fn catalog_looks_up_by_asset_flat_and_hash_and_rejects_duplicates() {
        let catalog = Catalog::new(
            1,
            vec![
                message("gameplay/progression/messages/a", 7, 47),
                message("gameplay/progression/messages/b", 9, 969),
            ],
        )
        .unwrap();
        assert_eq!(catalog.len(), 2);
        assert_eq!(
            catalog
                .get("Gameplay/Progression/Messages/B", 0)
                .unwrap()
                .flat,
            969
        );
        assert_eq!(catalog.by_flat(47).unwrap().hash, 7);
        assert_eq!(catalog.by_hash(9).unwrap().flat, 969);
        assert!(catalog.by_flat(48).is_none());
        for rows in [
            vec![message("a", 1, 47), message("a", 2, 48)],
            vec![message("a", 1, 47), message("b", 2, 47)],
            vec![message("a", 1, 47), message("b", 1, 48)],
            vec![message("a", 1, 4096)],
        ] {
            assert!(Catalog::new(1, rows).is_err());
        }
        assert!(Catalog::new(0, vec![]).is_err());
    }
}
