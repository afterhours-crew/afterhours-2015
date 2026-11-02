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
    availability: Option<Arc<super::availability::Content>>,
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
            availability: None,
        })
    }

    pub fn new_gated(
        contents: Vec<Content>,
        availability: Arc<super::availability::Content>,
    ) -> Result<Self, Error> {
        let mut catalog = Self::new(contents, availability.entries())?;
        catalog.discovery = availability.notifications();
        catalog.availability = Some(availability);
        Ok(catalog)
    }
    pub fn availability(&self) -> Option<&Arc<super::availability::Content>> {
        self.availability.as_ref()
    }

    /// The shared authored fade-in used for an explicitly selected departure.
    /// Require identical registration/manager identity across all event copies;
    /// choosing the first conflicting definition would hide a preparation error.
    pub fn destination_arrival(&self) -> Result<Option<nfs_world::arrival::Immediate>, Error> {
        let mut selected: Option<(&Content, &nfs_content::events::Sequence)> = None;
        for content in self.iter().filter(|c| c.event().restart.is_some()) {
            let sequence = content.sequence(super::request::RESTART_ARRIVAL)?;
            if let Some((_, old)) = selected {
                if old.manager != sequence.manager
                    || old.layout.asset != sequence.layout.asset
                    || old.layout.sub_id != sequence.layout.sub_id
                    || old.layout.bus_count != sequence.layout.bus_count
                    || old.layout.catalog != sequence.layout.catalog
                {
                    return Err(Error::Shape);
                }
            } else {
                selected = Some((content, sequence));
            }
        }
        selected
            .map(|(content, sequence)| {
                nfs_world::arrival::Immediate::new(
                    sequence.layout.asset,
                    nfs_world::sequences::Component {
                        scene_key: sequence.manager.0,
                        index: sequence.manager.1,
                    },
                    sequence.layout.sub_id,
                    sequence.layout.bus_count,
                    content.creation_content().clone(),
                )
            })
            .transpose()
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
            availability: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn with_arrival() -> Content {
        let mut event = super::super::tests::content().event().clone();
        event.restart = Some(vec![nfs_content::events::restart::Destination {
            position: [20., 3., -15.],
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        }]);
        let mut sequence = event.sequences[0].clone();
        sequence.layout.blueprint = super::super::request::RESTART_ARRIVAL.into();
        sequence.layout.asset.local_index = 39;
        sequence.layout.fields.truncate(1);
        sequence.layout.bus_count = 1;
        sequence.spawner = None;
        sequence.goto = None;
        sequence.wander = None;
        event.sequences.push(sequence);
        Content::new(event, "constructed".into()).unwrap()
    }
    #[test]
    fn destination_arrival_requires_consistent_authored_asset_and_manager() {
        assert!(
            Catalog::from(super::super::tests::content())
                .destination_arrival()
                .unwrap()
                .is_none()
        );
        let first = with_arrival();
        let mut second = first.clone();
        second.event.game_mode_id = "constructed/second".into();
        second.record_id = nfs_content::events::record_id(&second.event.game_mode_id);
        second.event.scene_content_key += 1;
        let roster = [entry(&first), entry(&second)];
        assert!(
            Catalog::new(vec![first.clone(), second.clone()], &roster)
                .unwrap()
                .destination_arrival()
                .unwrap()
                .is_some()
        );
        for field in 0..5 {
            let mut changed = second.event().clone();
            let arrival = changed.sequences.last_mut().unwrap();
            match field {
                0 => arrival.manager.0 += 1,
                1 => arrival.manager.1 += 1,
                2 => arrival.layout.asset.bundle = 42,
                3 => arrival.layout.sub_id += 1,
                4 => arrival.layout.bus_count += 1,
                _ => unreachable!(),
            }
            let changed = Content::new(changed, "constructed".into())
                .unwrap_or_else(|error| panic!("invalid mutation {field}: {error:?}"));
            assert!(
                matches!(
                    Catalog::new(vec![first.clone(), changed], &roster)
                        .unwrap()
                        .destination_arrival(),
                    Err(Error::Shape)
                ),
                "ambiguous field {field}"
            );
        }
    }
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
