// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scene locations exposed by event start areas. Selecting one does not start
//! an event or change its availability.
use super::{Endpoint, MAX_EVENTS};
use crate::{
    Error,
    events::restart::{Destination, MAX_LOCATIONS},
    graph::bad,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definitions {
    pub manager: Option<Endpoint>,
    #[serde(with = "crate::prepared_cache::ordered_map")]
    pub locations: BTreeMap<Endpoint, Vec<Destination>>,
}

impl Definitions {
    pub fn validate(&self) -> Result<(), Error> {
        let valid_site = |site: &Endpoint| site.scene_key != 0 && site.serializer <= 511;
        if self.locations.len() > MAX_EVENTS
            || self.manager.is_some_and(|s| !valid_site(&s))
            || (!self.locations.is_empty() && self.manager.is_none())
            || self.locations.iter().any(|(site, points)| {
                !valid_site(site)
                    || points.is_empty()
                    || points.len() > MAX_LOCATIONS
                    || points.iter().any(|p| {
                        p.position
                            .iter()
                            .chain(p.basis.iter().flatten())
                            .any(|n| !n.is_finite())
                            || nfs_world::replication::vehicle::Quaternion25::from_basis(p.basis)
                                .is_err()
                    })
            })
        {
            return Err(bad("event destination identity or transform bound"));
        }
        Ok(())
    }
    pub(crate) fn insert(&mut self, site: Endpoint, points: Vec<Destination>) -> Result<(), Error> {
        if let Some(previous) = self.locations.get(&site) {
            if previous != &points {
                return Err(bad("inconsistent shared event destination"));
            }
        } else {
            self.locations.insert(site, points);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_locations_keep_order_and_reject_invalid_cached_definitions() {
        let site = Endpoint {
            scene_key: 12,
            serializer: 9,
        };
        let a = Destination {
            position: [3., 4., 5.],
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        };
        let b = Destination {
            position: [6., 7., 8.],
            ..a
        };
        let mut defs = Definitions {
            manager: Some(Endpoint {
                scene_key: 11,
                serializer: 4,
            }),
            ..Default::default()
        };
        defs.insert(site, vec![b, a]).unwrap();
        defs.insert(site, vec![b, a]).unwrap();
        assert_eq!(defs.locations[&site], vec![b, a]);
        assert!(defs.insert(site, vec![a, b]).is_err());
        defs.validate().unwrap();
        let mut no_manager = defs.clone();
        no_manager.manager = None;
        assert!(no_manager.validate().is_err());
        for points in [
            vec![],
            vec![a; MAX_LOCATIONS + 1],
            vec![Destination {
                position: [f32::NAN, 0., 0.],
                ..a
            }],
        ] {
            defs.locations.insert(site, points);
            assert!(defs.validate().is_err());
        }
    }
}
