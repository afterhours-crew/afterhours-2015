// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Sequence construction shared by story and event owners. Field order comes
//! from installed composition; channel endpoints come from the live owner.
use crate::replication::{
    Error,
    entity::{
        Body, Initial, Kind, Profile, Rpc, Update,
        creation::{Asset, Creation, Prefix},
    },
};
use nfs_protocol::world::rpc::Serial;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Blueprint {
    asset: Asset,
    embedded_bus: u32,
    bus_count: u16,
    profile: Profile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Instance {
    pub manager: (u16, u16),
    pub number: u32,
    pub participant: u16,
}

impl Blueprint {
    pub fn new(
        asset: Asset,
        embedded_bus: u32,
        bus_count: u16,
        fields: &[Kind],
    ) -> Result<Self, Error> {
        let profile = Profile::new(fields)?;
        if bus_count == 0
            || embedded_bus == 0
            || fields[0] != Kind::SequenceRoot
            || fields[1..].iter().any(|kind| {
                !matches!(
                    kind,
                    Kind::Rpc
                        | Kind::RpcReference
                        | Kind::RpcOptionalReference
                        | Kind::RpcReferences
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

    /// Resolve each channel in serializer order. No allocation or live state is
    /// changed here; a failed binding leaves publication to the caller's transaction.
    pub fn creation(
        &self,
        instance: Instance,
        mut channel: impl FnMut(usize) -> Result<(u16, u16), Error>,
    ) -> Result<Creation, Error> {
        let valid_ref = |id: u16| id != 0 && id <= 8191;
        if !valid_ref(instance.manager.0)
            || instance.manager.1 > 511
            || !valid_ref(instance.participant)
        {
            return Err(Error::Bound);
        }
        let mut initial = Vec::with_capacity(self.profile.kinds().len());
        for (index, kind) in self.profile.kinds().iter().enumerate() {
            let rpc = Rpc {
                selector: index as u16,
                serial: Serial::new(1).expect("fresh lifetime"),
            };
            initial.push(match kind {
                Kind::SequenceRoot => Initial::SequenceRoot {
                    value: self.bus_count,
                    rpc,
                    manager: instance.manager.0,
                    manager_selector: instance.manager.1,
                    sequence: instance.number,
                    participants: vec![instance.participant],
                    stopping: false,
                },
                Kind::RpcOptionalReference => Initial::RpcOptionalReference {
                    rpc,
                    reference: Some(instance.participant),
                },
                Kind::RpcReference => {
                    let (reference, target_selector) = channel(index)?;
                    if !valid_ref(reference) || target_selector > 511 {
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
                    references: vec![instance.participant],
                },
                Kind::Rpc => Initial::Rpc(rpc),
                _ => return Err(Error::Unsupported),
            });
        }
        let updates = vec![Some(Update::Noop); initial.len()];
        Ok(Creation {
            prefix: Prefix {
                parent: None,
                blueprint: instance.manager.0,
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
    fn asset() -> Asset {
        Asset {
            bundle: 7,
            type_id: 12,
            local_index: 2,
        }
    }
    fn instance() -> Instance {
        Instance {
            manager: (20, 4),
            number: 3,
            participant: 100,
        }
    }

    #[test]
    fn channel_before_provider_and_multiple_channels_keep_authored_order() {
        let blueprint = Blueprint::new(
            asset(),
            2,
            5,
            &[
                Kind::SequenceRoot,
                Kind::RpcReference,
                Kind::RpcOptionalReference,
                Kind::RpcReference,
                Kind::Rpc,
                Kind::RpcReferences,
            ],
        )
        .unwrap();
        let mut resolved = vec![];
        let creation = blueprint
            .creation(instance(), |index| {
                resolved.push(index);
                Ok((200 + index as u16, 40 + index as u16))
            })
            .unwrap();
        assert_eq!(resolved, [1, 3]);
        let fields = creation.body.initial.as_ref().unwrap();
        assert!(
            matches!(&fields[0], Initial::SequenceRoot { value: 5, manager: 20, manager_selector: 4, sequence: 3, participants, stopping: false, .. } if participants == &[100])
        );
        assert!(
            matches!(&fields[1], Initial::RpcReference { rpc, reference: 201, target_selector: 41 } if rpc.selector == 1)
        );
        assert!(
            matches!(&fields[2], Initial::RpcOptionalReference { rpc, reference: Some(100) } if rpc.selector == 2)
        );
        assert!(
            matches!(&fields[5], Initial::RpcReferences { references, .. } if references == &[100])
        );
        let wire = creation.body.encode(blueprint.profile()).unwrap();
        let decoded = Body::decode(wire.span(), blueprint.profile(), true).unwrap();
        assert_eq!(decoded.body, creation.body);
        assert_eq!(decoded.bits, wire.span().len());
    }

    #[test]
    fn binding_failure_and_invalid_identity_never_return_partial_creation() {
        let blueprint =
            Blueprint::new(asset(), 1, 1, &[Kind::SequenceRoot, Kind::RpcReference]).unwrap();
        assert_eq!(
            blueprint.creation(instance(), |_| Err(Error::UnknownObject)),
            Err(Error::UnknownObject)
        );
        for endpoint in [(0, 0), (8192, 1), (1, 512)] {
            assert_eq!(
                blueprint.creation(instance(), |_| Ok(endpoint)),
                Err(Error::Bound)
            );
            assert_eq!(
                blueprint.creation(
                    Instance {
                        manager: endpoint,
                        ..instance()
                    },
                    |_| Ok((1, 0))
                ),
                Err(Error::Bound)
            );
        }
        assert_eq!(
            blueprint.creation(
                Instance {
                    participant: 0,
                    ..instance()
                },
                |_| Ok((1, 0))
            ),
            Err(Error::Bound)
        );
        assert!(Blueprint::new(asset(), 1, 1, &[Kind::SequenceRoot, Kind::BoolProperty]).is_err());
        assert!(Blueprint::new(asset(), 1, 0, &[Kind::SequenceRoot]).is_err());
        assert!(Blueprint::new(asset(), 1, 1, &[Kind::SequenceRoot, Kind::SequenceRoot]).is_err());
    }
}
