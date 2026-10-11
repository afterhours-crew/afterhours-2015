// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_world::{
    logic::{EntityRef, Fire, Message},
    replication::Error,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Condition {
    BelowThreshold,
    AtOrAboveThreshold,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Owner {
    pub vehicle: u16,
    pub participant: u16,
    pub local_slot: u8,
    pub runtime_index: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Glass {
    vehicles: BTreeMap<u16, (Owner, [Option<Condition>; 3])>,
}
fn signal(event: u32) -> Option<(usize, Condition, u32)> {
    use Condition::*;
    Some(match event {
        28_404_286 => (0, BelowThreshold, 7),
        10_621_459 => (0, AtOrAboveThreshold, 6),
        14_703_462 => (1, BelowThreshold, 9),
        31_234_473 => (1, AtOrAboveThreshold, 8),
        21_578_436 => (2, BelowThreshold, 11),
        22_948_705 => (2, AtOrAboveThreshold, 10),
        _ => return None,
    })
}

impl Glass {
    pub fn recognizes(message: &Message) -> bool {
        matches!(message, Message::Reached { event, .. } if signal(*event).is_some())
    }

    pub fn state(&self, vehicle: u16) -> Option<&[Option<Condition>; 3]> {
        self.vehicles.get(&vehicle).map(|(_, state)| state)
    }
    pub fn receive(&mut self, owner: Owner, message: &Message) -> Result<Fire, Error> {
        let Message::Reached {
            event,
            target,
            player,
        } = *message
        else {
            return Err(Error::Shape);
        };
        let (channel, condition, event) = signal(event).ok_or(Error::Unsupported)?;
        if target
            != (EntityRef {
                ghost: owner.vehicle,
                entity: 2,
            })
            || player != owner.local_slot
            || owner.local_slot > 7
            || owner.vehicle == 0
            || owner.participant == 0
        {
            return Err(Error::UnknownObject);
        }
        if let Some((old, _)) = self.vehicles.get(&owner.vehicle) {
            if *old != owner {
                return Err(Error::UnknownObject);
            }
        } else if self.vehicles.len() >= 640 {
            return Err(Error::Bound);
        }
        let fire = Fire {
            event,
            player: Some(i32::from(owner.runtime_index)),
            target,
        };
        fire.encode().map_err(|_| Error::Shape)?;
        self.vehicles
            .entry(owner.vehicle)
            .or_insert((owner, [None; 3]))
            .1[channel] = Some(condition);
        Ok(fire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn glass_tracks_each_vehicle_and_channel_and_refires_explicit_repeats() {
        let owner = Owner {
            vehicle: 293,
            participant: 260,
            local_slot: 0,
            runtime_index: 4,
        };
        let mut glass = Glass::default();
        for (event, expected) in [(21_578_436, 11), (14_703_462, 9), (28_404_286, 7)] {
            let input = Message::Reached {
                event,
                target: EntityRef {
                    ghost: 293,
                    entity: 2,
                },
                player: 0,
            };
            let output = glass.receive(owner, &input).unwrap();
            assert_eq!(output.event, expected);
            assert_eq!(output.player, Some(4));
            assert_eq!(glass.receive(owner, &input).unwrap(), output);
        }
        assert_eq!(
            glass.state(293),
            Some(&[Some(Condition::BelowThreshold); 3])
        );
        let input = Message::Reached {
            event: 10_621_459,
            target: EntityRef {
                ghost: 294,
                entity: 2,
            },
            player: 0,
        };
        assert_eq!(
            glass
                .receive(
                    Owner {
                        vehicle: 294,
                        ..owner
                    },
                    &input
                )
                .unwrap()
                .event,
            6
        );
        assert_eq!(
            glass.state(294),
            Some(&[Some(Condition::AtOrAboveThreshold), None, None])
        );
        assert_eq!(
            glass.state(293),
            Some(&[Some(Condition::BelowThreshold); 3])
        );
        let before = glass.clone();
        for bad in [
            Owner {
                local_slot: 1,
                ..owner
            },
            Owner {
                participant: 261,
                ..owner
            },
            Owner {
                runtime_index: 5,
                ..owner
            },
        ] {
            assert!(
                glass
                    .receive(
                        bad,
                        &Message::Reached {
                            event: 28_404_286,
                            target: EntityRef {
                                ghost: 293,
                                entity: 2
                            },
                            player: 0
                        }
                    )
                    .is_err()
            );
        }
        assert_eq!(glass, before);
    }
}
