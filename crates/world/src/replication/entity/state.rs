// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{
    Body, Initial, Kind, Profile, Update,
    creation::{Content, Creation, Prefix},
};
use crate::replication::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    prefix: Prefix,
    profile: Profile,
    initial: Vec<Initial>,
}
impl State {
    pub fn new(
        creation: Creation,
        content: &Content,
        known: impl Fn(u16) -> bool,
    ) -> Result<Self, Error> {
        creation.encode(content)?;
        let initial = creation.body.initial.ok_or(Error::Shape)?;
        let prefix = creation.prefix;
        let valid = |id: u16| id == 0 || known(id);
        if !valid(prefix.blueprint)
            || prefix
                .parent
                .as_ref()
                .and_then(|p| p.reference)
                .is_some_and(|id| !valid(id))
            || prefix.owner.is_some_and(|id| !valid(id))
            || initial.iter().any(|value| match value {
                Initial::Root { reference, .. }
                | Initial::RpcReference { reference, .. }
                | Initial::RpcGhostReference { reference, .. } => !valid(*reference),
                Initial::RpcOptionalReference { reference, .. } => {
                    reference.is_some_and(|id| !valid(id))
                }
                Initial::SequenceRoot {
                    manager,
                    participants,
                    ..
                } => !valid(*manager) || participants.iter().any(|id| !valid(*id)),
                _ => false,
            })
        {
            return Err(Error::UnknownObject);
        }
        let profile = content.profile(prefix.asset)?.clone();
        let mut result = Self {
            prefix,
            profile,
            initial,
        };
        result.apply(creation.body.updates)?;
        Ok(result)
    }

    pub fn apply(&mut self, updates: Vec<Option<Update>>) -> Result<(), Error> {
        if updates.len() != self.profile.kinds().len() {
            return Err(Error::Shape);
        }
        Body {
            initial: None,
            updates: updates.clone(),
        }
        .encode(&self.profile)?;
        let mut initial = self.initial.clone();
        for (value, update) in initial.iter_mut().zip(updates) {
            match (value, update) {
                (Initial::BoolProperty(value), Some(Update::BoolProperty(Some(next)))) => {
                    *value = Some(next)
                }
                (Initial::I32Property(value), Some(Update::I32Property(Some(next)))) => {
                    *value = Some(next)
                }
                (
                    Initial::RpcGhostReference { reference, .. },
                    Some(Update::GhostReference(Some(next))),
                ) => *reference = next,
                _ => (),
            }
        }
        self.initial = initial;
        Ok(())
    }

    pub fn snapshot(&self) -> Creation {
        let updates = self
            .profile
            .kinds()
            .iter()
            .map(|kind| {
                Some(match kind {
                    Kind::Root
                    | Kind::SequenceRoot
                    | Kind::Rpc
                    | Kind::RpcReference
                    | Kind::RpcFlags
                    | Kind::RpcVariant
                    | Kind::RpcOptionalReference => Update::Noop,
                    Kind::RpcGhostReference => Update::GhostReference(None),
                    Kind::BoolProperty => Update::BoolProperty(None),
                    Kind::I32Property => Update::I32Property(None),
                })
            })
            .collect();
        Creation {
            prefix: self.prefix.clone(),
            body: Body {
                initial: Some(self.initial.clone()),
                updates,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        Rpc,
        creation::{Asset, Catalog, Parent},
    };
    use super::*;
    use nfs_protocol::world::rpc::Serial;

    fn fixture() -> (Creation, Content) {
        let asset = Asset {
            bundle: 3,
            type_id: 1,
            local_index: 0,
        };
        let profile = Profile::new(&[Kind::Root, Kind::BoolProperty, Kind::I32Property]).unwrap();
        let content = Content::new(
            vec![Catalog::new(3, &[(1, 1)]).unwrap()],
            vec![(asset, profile)],
        )
        .unwrap();
        let creation = Creation {
            prefix: Prefix {
                parent: Some(Parent { reference: Some(2) }),
                blueprint: 3,
                sub_id: 1,
                owner: Some(4),
                asset,
            },
            body: Body {
                initial: Some(vec![
                    Initial::Root {
                        value: 7,
                        rpc: Rpc {
                            selector: 0,
                            serial: Serial::new(2).unwrap(),
                        },
                        reference: 5,
                    },
                    Initial::BoolProperty(Some(false)),
                    Initial::I32Property(None),
                ]),
                updates: vec![
                    Some(Update::Noop),
                    Some(Update::BoolProperty(Some(true))),
                    None,
                ],
            },
        };
        (creation, content)
    }
    #[test]
    fn initial_delta_and_repeats_keep_current_fields_and_isolate_owners() {
        let (creation, content) = fixture();
        let mut first = State::new(creation.clone(), &content, |_| true).unwrap();
        let other = State::new(creation, &content, |_| true).unwrap();
        assert_eq!(
            first.snapshot().body.initial.as_ref().unwrap()[1],
            Initial::BoolProperty(Some(true))
        );
        let delta = vec![
            None,
            Some(Update::BoolProperty(None)),
            Some(Update::I32Property(Some(-8))),
        ];
        first.apply(delta.clone()).unwrap();
        let saved = first.snapshot();
        first.apply(delta).unwrap();
        assert_eq!(first.snapshot(), saved);
        assert_eq!(
            first.snapshot().body.initial.as_ref().unwrap()[2],
            Initial::I32Property(Some(-8))
        );
        assert_eq!(
            other.snapshot().body.initial.as_ref().unwrap()[2],
            Initial::I32Property(None)
        );
        let wire = saved.encode(&content).unwrap();
        assert_eq!(
            Creation::decode(wire.span(), &content).unwrap().creation,
            saved
        );
    }
    #[test]
    fn each_reference_must_resolve_in_current_world() {
        let (creation, content) = fixture();
        for missing in [2, 3, 4, 5] {
            assert_eq!(
                State::new(creation.clone(), &content, |id| id != missing),
                Err(Error::UnknownObject)
            );
        }
    }
    #[test]
    fn late_bad_delta_never_commits_earlier_property() {
        let (creation, content) = fixture();
        let mut state = State::new(creation, &content, |_| true).unwrap();
        let before = state.clone();
        assert_eq!(
            state.apply(vec![
                None,
                Some(Update::BoolProperty(Some(false))),
                Some(Update::Noop)
            ]),
            Err(Error::TypeMismatch)
        );
        assert_eq!(state, before);
        assert_eq!(state.apply(vec![None]), Err(Error::Shape));
        assert_eq!(state, before);
    }
}
