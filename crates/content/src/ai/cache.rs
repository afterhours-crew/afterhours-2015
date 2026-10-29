// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The same installed AI managers serve many event rosters. Resolve their
//! whole-world lifecycle once per complete source context, not once per race.
use super::*;
use nfs_world::replication::players::ai::Managers;
use std::collections::BTreeMap;

const MAX_ENTRIES: usize = 16;

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct Key {
    roots: BTreeMap<String, Site>,
    rules: crate::scenes::Rules,
    participants: Site,
    actors: Site,
    ports: Links,
}

impl Key {
    pub fn new(lookup: &SceneLookup<'_>, participants: &Site, actors: &Site, ports: Links) -> Self {
        Self {
            roots: lookup.roots.clone(),
            rules: lookup.rules.clone(),
            participants: participants.clone(),
            actors: actors.clone(),
            ports,
        }
    }
}

/// Owned by one immutable installation index. Never shared across installations
/// or used for live participants, spawn choices or mutable manager state.
#[derive(Default)]
pub(crate) struct Cache(Vec<(Key, Managers)>);

impl Cache {
    pub fn get(&self, key: &Key) -> Option<Managers> {
        self.0
            .iter()
            .find(|(stored, _)| stored == key)
            .map(|(_, value)| *value)
    }

    pub fn insert(&mut self, key: Key, value: Managers) {
        if self.get(&key).is_some() {
            return;
        }
        if self.0.len() == MAX_ENTRIES {
            self.0.remove(0);
        }
        self.0.push((key, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_cache_requires_every_source_context_and_retains_a_finite_number() {
        let key = Key {
            roots: BTreeMap::from([("scene".into(), Site::root("scene"))]),
            rules: crate::BuildIdentity::supported().scenes,
            participants: Site::root("participants"),
            actors: Site::root("actors"),
            ports: Links {
                initial: 11,
                source: 22,
                destination: 33,
                reset_volumes: 44,
            },
        };
        let value = Managers {
            scene: 1,
            participants: 2,
            actors: 3,
            alive: State {
                scene: 4,
                serializer: 5,
            },
            reset_volumes: State {
                scene: 6,
                serializer: 7,
            },
        };
        let mut cache = Cache::default();
        assert!(cache.get(&key).is_none());
        cache.insert(key.clone(), value);
        assert_eq!(cache.get(&key), Some(value));
        let mut changed = key.clone();
        changed
            .roots
            .insert("additional".into(), Site::root("additional"));
        assert!(cache.get(&changed).is_none());
        let mut changed = key.clone();
        changed.rules.level = "other".into();
        assert!(cache.get(&changed).is_none());
        let mut changed = key.clone();
        changed.participants.key.1 = 1;
        assert!(cache.get(&changed).is_none());
        let mut changed = key.clone();
        changed.actors.key.1 = 1;
        assert!(cache.get(&changed).is_none());
        let mut changed = key.clone();
        changed.ports.reset_volumes += 1;
        assert!(cache.get(&changed).is_none());
        // A known-good previous installation cannot satisfy another index.
        assert!(Cache::default().get(&key).is_none());
        for i in 0..MAX_ENTRIES + 1 {
            let mut other = key.clone();
            other.participants.key.1 = i + 1;
            cache.insert(other, value);
        }
        assert_eq!(cache.0.len(), MAX_ENTRIES);
        assert!(cache.get(&key).is_none());
    }
}
