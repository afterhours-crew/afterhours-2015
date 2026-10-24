// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared game-mode construction. Content owns layout and initial-state choice;
//! the session supplies live bindings, participant identity and clock values.
use crate::replication::{
    Error,
    entity::{
        Body, GameModeRoot, Initial, Kind, Profile, Rpc, Update,
        creation::{Asset, Creation, Prefix},
    },
    sublevel::Set64,
};
use nfs_protocol::world::rpc::Serial;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Blueprint {
    asset: Asset,
    embedded_bus: u32,
    bus_count: u16,
    profile: Profile,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    /// Game time, game-mode manager and participant manager, in that order.
    pub bindings: [(u16, u16); 3],
    pub participant: u16,
    pub start_seconds: f64,
    pub age_seconds: f32,
    pub laps: u8,
    pub waypoints: Vec<(u16, u16)>,
    /// States containing this participant when the creation is published.
    pub members: BTreeSet<usize>,
}

impl Blueprint {
    pub fn new(
        asset: Asset,
        embedded_bus: u32,
        bus_count: u16,
        fields: &[Kind],
    ) -> Result<Self, Error> {
        let profile = Profile::new(fields)?;
        if embedded_bus == 0
            || bus_count == 0
            || fields[0] != Kind::GameModeRoot
            || fields[1..].iter().any(|k| {
                !matches!(
                    k,
                    Kind::Rpc
                        | Kind::RpcReference
                        | Kind::RpcReferences
                        | Kind::CountdownState
                        | Kind::WaypointsState
                        | Kind::DnfTransition
                        | Kind::WaypointList
                        | Kind::RpcGuid
                        | Kind::RpcSet64
                )
            })
        {
            return Err(Error::Unsupported);
        }
        Ok(Self {
            asset,
            embedded_bus,
            bus_count,
            profile,
        })
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    pub fn creation(
        &self,
        instance: Instance,
        mut channel: impl FnMut(usize) -> Result<(u16, u16), Error>,
    ) -> Result<Creation, Error> {
        let valid = |(id, selector): (u16, u16)| id != 0 && id <= 8191 && selector <= 511;
        if !valid((instance.participant, 0))
            || instance.bindings.iter().any(|&e| !valid(e))
            || instance.waypoints.iter().any(|&e| !valid(e))
            || instance
                .members
                .iter()
                .any(|&i| self.profile.kinds().get(i) != Some(&Kind::RpcReferences))
            || instance.waypoints.len() > 255
            || !(1..=15).contains(&instance.laps)
            || !instance.start_seconds.is_finite()
            || instance.start_seconds < 0.
            || !instance.age_seconds.is_finite()
            || instance.age_seconds < 0.
        {
            return Err(Error::Bound);
        }
        let mut initial = Vec::with_capacity(self.profile.kinds().len());
        let mut updates = Vec::with_capacity(self.profile.kinds().len());
        for (index, kind) in self.profile.kinds().iter().enumerate() {
            let rpc = Rpc {
                selector: index as u16,
                serial: Serial::new(1).expect("fresh lifetime"),
            };
            let field = match kind {
                Kind::GameModeRoot => Initial::GameModeRoot(Box::new(GameModeRoot {
                    value: self.bus_count,
                    rpc,
                    start: instance.start_seconds.to_bits(),
                    age: instance.age_seconds.to_bits(),
                    bindings: instance.bindings,
                    participants: vec![instance.participant],
                    others: vec![],
                    flag: false,
                })),
                Kind::Rpc => Initial::Rpc(rpc),
                Kind::RpcReference => {
                    let (reference, target_selector) = channel(index)?;
                    if !valid((reference, target_selector)) {
                        return Err(Error::Bound);
                    }
                    Initial::RpcReference {
                        rpc,
                        reference,
                        target_selector,
                    }
                }
                Kind::RpcReferences => Initial::RpcReferences {
                    rpc,
                    references: if instance.members.contains(&index) {
                        vec![instance.participant]
                    } else {
                        vec![]
                    },
                },
                Kind::CountdownState => Initial::CountdownState {
                    rpc,
                    references: vec![],
                    start: None,
                },
                Kind::WaypointsState => Initial::WaypointsState {
                    rpc,
                    references: vec![],
                    words: [0; 3],
                },
                Kind::DnfTransition => Initial::DnfTransition {
                    rpc,
                    values: vec![],
                },
                Kind::WaypointList => Initial::WaypointList {
                    rpc,
                    laps: instance.laps,
                    waypoints: instance.waypoints.clone(),
                },
                Kind::RpcGuid => Initial::RpcGuid { rpc, guid: None },
                Kind::RpcSet64 => Initial::RpcSet64(rpc),
                _ => return Err(Error::Unsupported),
            };
            initial.push(field);
            updates.push(Some(if *kind == Kind::RpcSet64 {
                Update::Set64(Some(Set64 { entries: None }))
            } else {
                Update::Noop
            }));
        }
        Ok(Creation {
            prefix: Prefix {
                parent: None,
                blueprint: instance.bindings[1].0,
                sub_id: self.embedded_bus,
                owner: None,
                asset: self.asset,
            },
            body: Body {
                initial: Some(initial),
                updates,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn blueprint() -> Blueprint {
        Blueprint::new(
            Asset {
                bundle: 2,
                type_id: 6,
                local_index: 1,
            },
            3,
            2,
            &[
                Kind::GameModeRoot,
                Kind::RpcReferences,
                Kind::RpcReference,
                Kind::RpcReferences,
                Kind::WaypointList,
                Kind::RpcSet64,
            ],
        )
        .unwrap()
    }
    fn instance() -> Instance {
        Instance {
            bindings: [(30, 6), (30, 5), (30, 0)],
            participant: 42,
            start_seconds: 123.25,
            age_seconds: 0.125,
            laps: 1,
            waypoints: vec![(35, 20)],
            members: BTreeSet::from([3]),
        }
    }
    #[test]
    fn authored_initial_state_and_live_context_survive_wire_encoding() {
        let b = blueprint();
        for participant in [42, 99] {
            let c = b
                .creation(
                    Instance {
                        participant,
                        ..instance()
                    },
                    |i| {
                        assert_eq!(i, 2);
                        Ok((participant + 1, 17))
                    },
                )
                .unwrap();
            let first = c.body.initial.as_ref().unwrap();
            assert!(
                matches!(&first[1], Initial::RpcReferences { references, .. } if references.is_empty())
            );
            assert!(
                matches!(&first[3], Initial::RpcReferences { references, .. } if references == &[participant])
            );
            let bits = c.body.encode(b.profile()).unwrap();
            assert_eq!(
                Body::decode(bits.span(), b.profile(), true).unwrap().body,
                c.body
            );
        }
    }
    #[test]
    fn invalid_context_and_binding_fail_without_a_partial_creation() {
        let b = blueprint();
        for value in [f64::NAN, f64::INFINITY, -1.] {
            assert_eq!(
                b.creation(
                    Instance {
                        start_seconds: value,
                        ..instance()
                    },
                    |_| Ok((1, 0))
                ),
                Err(Error::Bound)
            );
        }
        assert_eq!(
            b.creation(instance(), |_| Err(Error::UnknownObject)),
            Err(Error::UnknownObject)
        );
        for endpoint in [(0, 0), (8192, 0), (1, 512)] {
            assert_eq!(b.creation(instance(), |_| Ok(endpoint)), Err(Error::Bound));
            assert_eq!(
                b.creation(
                    Instance {
                        bindings: [endpoint; 3],
                        ..instance()
                    },
                    |_| Ok((1, 0))
                ),
                Err(Error::Bound)
            );
        }
        assert_eq!(
            b.creation(
                Instance {
                    members: BTreeSet::from([2]),
                    ..instance()
                },
                |_| Ok((1, 0))
            ),
            Err(Error::Bound)
        );
        assert!(
            Blueprint::new(
                b.asset,
                1,
                1,
                &[Kind::GameModeRoot, Kind::RpcReferences, Kind::Root]
            )
            .is_err()
        );
    }
}
