// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Immutable event content indexed by full identity and exact start endpoint.
//! Session membership and running instances belong to the connection, not here.
use super::Content;
use nfs_content::events::discovery::{Endpoint, Entry, MAX_EVENTS};
use nfs_world::replication::Error;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub mod prepare;

#[derive(Clone, Debug)]
pub struct Catalog {
    events: BTreeMap<u32, Arc<Content>>,
    starts: BTreeMap<Endpoint, u32>,
    discovery: BTreeSet<Endpoint>,
}

impl Catalog {
    /// All playable entries must agree with the independently resolved map
    /// roster. Reject hash collisions and ambiguous endpoints before publishing
    /// any of the catalog. Other families can remain visible without being offered.
    pub fn new(contents: Vec<Content>, roster: &[Entry]) -> Result<Self, Error> {
        if contents.is_empty() || contents.len() > MAX_EVENTS || roster.len() > MAX_EVENTS {
            return Err(Error::Bound);
        }
        let mut identities = BTreeMap::new();
        let mut discovery = BTreeSet::new();
        for entry in roster {
            if entry.id.is_empty()
                || entry.id.len() > 512
                || entry.notifications.len() > 16
                || identities.insert(entry.id.as_str(), entry).is_some()
            {
                return Err(Error::Shape);
            }
            for endpoint in &entry.notifications {
                if endpoint.scene_key == 0 {
                    return Err(Error::Shape);
                }
                discovery.insert(*endpoint);
            }
        }
        let mut events = BTreeMap::new();
        let mut starts = BTreeMap::new();
        for content in contents {
            let event = content.event();
            let entry = identities
                .get(event.game_mode_id.as_str())
                .ok_or(Error::UnknownObject)?;
            let endpoint = Endpoint {
                scene_key: event.scene_content_key,
                serializer: event.start_volume,
            };
            if entry.volume != endpoint {
                return Err(Error::Shape);
            }
            let id = content.record_id();
            if starts.insert(endpoint, id).is_some()
                || events.insert(id, Arc::new(content)).is_some()
            {
                return Err(Error::DuplicateObject);
            }
        }
        Ok(Self {
            events,
            starts,
            discovery,
        })
    }

    pub fn get(&self, id: u32) -> Option<&Content> {
        self.events.get(&id).map(Arc::as_ref)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Content> {
        self.events.values().map(Arc::as_ref)
    }
    pub fn at_start(&self, endpoint: Endpoint) -> Option<&Content> {
        self.get(*self.starts.get(&endpoint)?)
    }
    pub fn discovery_notifications(&self) -> &BTreeSet<Endpoint> {
        &self.discovery
    }
}

/// Preserve the single-event adapter for isolated/reference callers. It has
/// already validated its own layout; discovery follows its explicit selection.
impl From<Content> for Catalog {
    fn from(content: Content) -> Self {
        let endpoint = Endpoint {
            scene_key: content.event().scene_content_key,
            serializer: content.event().start_volume,
        };
        let id = content.record_id();
        Self {
            discovery: content.discovery_notifications().clone(),
            starts: BTreeMap::from([(endpoint, id)]),
            events: BTreeMap::from([(id, Arc::new(content))]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(content: &Content) -> Entry {
        Entry {
            id: content.event().game_mode_id.clone(),
            scene: "constructed".into(),
            game_mode: "constructed".into(),
            volume: Endpoint {
                scene_key: content.event().scene_content_key,
                serializer: content.event().start_volume,
            },
            notifications: vec![],
        }
    }
    #[test]
    fn exact_start_endpoint_and_identity_must_both_match() {
        let first = super::super::tests::content();
        let mut second = first.clone();
        second.event.game_mode_id = "constructed/second".into();
        second.record_id = nfs_content::events::record_id(&second.event.game_mode_id);
        second.event.scene_content_key += 1;
        let roster = [entry(&first), entry(&second)];
        let catalog = Catalog::new(vec![first.clone(), second.clone()], &roster).unwrap();
        assert_eq!(
            catalog.at_start(roster[0].volume).unwrap().record_id(),
            first.record_id()
        );
        assert_eq!(
            catalog.at_start(roster[1].volume).unwrap().record_id(),
            second.record_id()
        );
        assert!(
            catalog
                .at_start(Endpoint {
                    scene_key: roster[1].volume.scene_key,
                    serializer: 999
                })
                .is_none()
        );
        assert!(catalog.get(0).is_none());
        let mut mismatched = roster.clone();
        mismatched[1].volume.serializer += 1;
        assert!(matches!(
            Catalog::new(vec![first.clone(), second.clone()], &mismatched),
            Err(Error::Shape)
        ));
        second.record_id = first.record_id();
        assert!(matches!(
            Catalog::new(vec![first, second], &roster),
            Err(Error::DuplicateObject)
        ));
    }
    #[test]
    fn duplicate_starts_and_roster_identities_are_rejected() {
        let first = super::super::tests::content();
        let mut second = first.clone();
        second.event.game_mode_id = "constructed/second".into();
        second.record_id = nfs_content::events::record_id(&second.event.game_mode_id);
        assert!(matches!(
            Catalog::new(
                vec![first.clone(), second.clone()],
                &[entry(&first), entry(&second)]
            ),
            Err(Error::DuplicateObject)
        ));
        assert!(matches!(
            Catalog::new(vec![first.clone()], &[entry(&first), entry(&first)]),
            Err(Error::Shape)
        ));
        assert!(matches!(Catalog::new(vec![], &[]), Err(Error::Bound)));
    }
}
