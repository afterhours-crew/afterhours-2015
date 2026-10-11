// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Error;
use crate::replication::MAX_OBJECTS;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Group {
    next: u8,
    members: BTreeSet<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Registry {
    groups: BTreeMap<u64, Group>,
    members: BTreeMap<u64, (u64, u8)>,
}
impl Registry {
    pub fn register(&mut self, group: u64, component: u64) -> Result<u8, Error> {
        if group == 0 || component == 0 {
            return Err(Error::Shape);
        }
        if let Some(&(existing, index)) = self.members.get(&component) {
            return if existing == group {
                Ok(index)
            } else {
                Err(Error::DuplicateObject)
            };
        }
        if self.members.len() >= MAX_OBJECTS
            || (!self.groups.contains_key(&group) && self.groups.len() >= MAX_OBJECTS)
        {
            return Err(Error::Bound);
        }
        let index = self.groups.get(&group).map_or(0, |g| g.next);
        if index > 15 {
            return Err(Error::Bound);
        }
        let state = self.groups.entry(group).or_default();
        state.members.insert(component);
        state.next += 1;
        self.members.insert(component, (group, index));
        Ok(index)
    }

    pub fn index(&self, group: u64, component: u64) -> Result<u8, Error> {
        self.members
            .get(&component)
            .filter(|(owner, _)| *owner == group)
            .map(|(_, index)| *index)
            .ok_or(Error::UnknownObject)
    }

    pub fn unregister(&mut self, group: u64, component: u64) -> Result<bool, Error> {
        let Some(&(existing, _)) = self.members.get(&component) else {
            return Ok(false);
        };
        if existing != group {
            return Err(Error::UnknownObject);
        }
        let state = self.groups.get_mut(&group).ok_or(Error::UnknownObject)?;
        state.members.remove(&component);
        if state.members.is_empty() {
            self.groups.remove(&group);
        }
        self.members.remove(&component);
        Ok(true)
    }

    pub fn remove_group(&mut self, group: u64) -> bool {
        let Some(state) = self.groups.remove(&group) else {
            return false;
        };
        for component in state.members {
            self.members.remove(&component);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_registration_retries_and_owner_isolation() {
        let mut r = Registry::default();
        assert_eq!(r.index(1, 10), Err(Error::UnknownObject));
        assert_eq!(r.register(1, 10), Ok(0));
        assert_eq!(r.register(1, 11), Ok(1));
        let before = r.clone();
        assert_eq!(r.register(1, 10), Ok(0));
        assert_eq!(r.register(2, 10), Err(Error::DuplicateObject));
        assert_eq!(r.unregister(2, 10), Err(Error::UnknownObject));
        assert_eq!(r, before);
        assert_eq!(r.register(2, 12), Ok(0));
        assert_eq!(r.index(1, 12), Err(Error::UnknownObject));
        assert_eq!(Registry::default().register(1, 10), Ok(0));
    }
    #[test]
    fn removal_keeps_live_counter_and_last_removal_resets_group() {
        let mut r = Registry::default();
        r.register(1, 10).unwrap();
        r.register(1, 11).unwrap();
        assert_eq!(r.unregister(1, 10), Ok(true));
        assert_eq!(r.unregister(1, 10), Ok(false));
        assert_eq!(r.register(1, 12), Ok(2));
        r.unregister(1, 11).unwrap();
        r.unregister(1, 12).unwrap();
        assert_eq!(r.register(1, 10), Ok(0));
        assert!(r.remove_group(1));
        assert!(!r.remove_group(1));
        assert_eq!(r.index(1, 10), Err(Error::UnknownObject));
        assert_eq!(r, Registry::default());
    }
    #[test]
    fn exhaustion_and_invalid_identity_are_transactional() {
        let mut r = Registry::default();
        for n in 0..16 {
            assert_eq!(r.register(1, 100 + n), Ok(n as u8));
        }
        r.unregister(1, 100).unwrap();
        let before = r.clone();
        assert_eq!(r.register(1, 900), Err(Error::Bound));
        assert_eq!(r.register(0, 900), Err(Error::Shape));
        assert_eq!(r.register(1, 0), Err(Error::Shape));
        assert_eq!(r, before);
        r.remove_group(1);
        for n in 1..=MAX_OBJECTS {
            r.register(n as u64, n as u64).unwrap();
        }
        let before = r.clone();
        assert_eq!(
            r.register(MAX_OBJECTS as u64 + 1, MAX_OBJECTS as u64 + 1),
            Err(Error::Bound)
        );
        assert_eq!(r, before);
    }
}
