// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    /// Swap this secondary slot (0..4) with the primary slot.
    Select { secondary: u8 },
    /// A completed item purchase is placed into this secondary slot.
    Place { secondary: u8, item: u64 },
    /// Clear the secondary display after its owned item was sold.
    Sold { secondary: u8 },
    /// Keep the owned car and clear this parked slot.
    MoveToWarehouse { secondary: u8 },
    /// Retrieve or swap an already owned warehouse car into this parked slot.
    TakeFromWarehouse { secondary: u8, item: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Call {
    pub participant: u16,
    pub operation: Operation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Binding,
    Shape,
    Ownership,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    scene: u16,
    selector: u16,
}
impl Binding {
    pub fn new(scene: u16, selector: u16) -> Result<Self, Error> {
        if scene == 0 || scene > 8191 || selector > 511 {
            return Err(Error::Binding);
        }
        Ok(Self { scene, selector })
    }
    pub fn read(
        self,
        body: BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<Call>, Error> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if envelope.references().first() != Some(&self.scene)
            || route.selector() != self.selector
            || !matches!(route.method_index(), 2..=6)
        {
            return Ok(None);
        }
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || envelope.references().len() != 2
        {
            return Err(Error::Shape);
        }
        let participant = envelope.references()[1];
        if participant == 0 || !owns(participant) {
            return Err(Error::Ownership);
        }
        let args = route.arguments();
        if args.len()
            != if matches!(route.method_index(), 3 | 6) {
                103
            } else {
                39
            }
        {
            return Err(Error::Shape);
        }
        let secondary = args.read_u32(0, 32).map_err(|_| Error::Shape)?;
        if secondary >= 4 {
            return Err(Error::Shape);
        }
        let secondary = secondary as u8;
        let operation = match route.method_index() {
            2 => Operation::Select { secondary },
            3 | 6 => {
                let low = args.read_u32(32, 32).map_err(|_| Error::Shape)?;
                let high = args.read_u32(64, 32).map_err(|_| Error::Shape)?;
                let item = u64::from(low) | (u64::from(high) << 32);
                if item == 0 {
                    return Err(Error::Shape);
                }
                if route.method_index() == 3 {
                    Operation::Place { secondary, item }
                } else {
                    Operation::TakeFromWarehouse { secondary, item }
                }
            }
            4 => Operation::Sold { secondary },
            5 => Operation::MoveToWarehouse { secondary },
            _ => unreachable!(),
        };
        Ok(Some(Call {
            participant,
            operation,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world_core::bits::BitWriter;
    fn wire(method: u32, slot: u32, item: Option<u64>, references: &[u16]) -> BitWriter {
        let mut payload = BitWriter::new();
        payload
            .put(6, 9)
            .put(method.into(), 32)
            .put(slot.into(), 32);
        if let Some(id) = item {
            payload.put(id & 0xffff_ffff, 32).put(id >> 32, 32);
        }
        // Native payload alignment bits have no domain meaning.
        payload.put(127, 7);
        let mut body = BitWriter::new();
        body.put(0, 64).put(references.len() as u64, 8);
        for &r in references {
            body.put(r.into(), 13);
        }
        body.put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body
    }
    #[test]
    fn secondary_slot_and_item_are_distinct_from_the_participant_reference() {
        let binding = Binding::new(10, 6).unwrap();
        for slot in 0..4 {
            for (method, item, operation) in [
                (
                    2,
                    None,
                    Operation::Select {
                        secondary: slot as u8,
                    },
                ),
                (
                    3,
                    Some(0xf123456789abcdef),
                    Operation::Place {
                        secondary: slot as u8,
                        item: 0xf123456789abcdef,
                    },
                ),
                (
                    4,
                    None,
                    Operation::Sold {
                        secondary: slot as u8,
                    },
                ),
                (
                    5,
                    None,
                    Operation::MoveToWarehouse {
                        secondary: slot as u8,
                    },
                ),
                (
                    6,
                    Some(0xf123456789abcdef),
                    Operation::TakeFromWarehouse {
                        secondary: slot as u8,
                        item: 0xf123456789abcdef,
                    },
                ),
            ] {
                let body = wire(method, slot, item, &[10, 77]);
                assert_eq!(
                    binding.read(body.span(), |p| p == 77),
                    Ok(Some(Call {
                        participant: 77,
                        operation
                    }))
                );
                assert_eq!(binding.read(body.span(), |_| false), Err(Error::Ownership));
                assert_eq!(
                    Binding::new(11, 6).unwrap().read(body.span(), |_| true),
                    Ok(None)
                );
            }
        }
    }
    #[test]
    fn bounds_and_exact_extent_precede_any_state_change() {
        let binding = Binding::new(10, 6).unwrap();
        for body in [
            wire(2, 4, None, &[10, 77]),
            wire(2, u32::MAX, None, &[10, 77]),
            wire(3, 0, Some(0), &[10, 77]),
            wire(3, 0, None, &[10, 77]),
            wire(2, 0, Some(1), &[10, 77]),
            wire(2, 0, None, &[10, 77, 88]),
            wire(5, 4, None, &[10, 77]),
            wire(5, 0, Some(1), &[10, 77]),
            wire(6, 0, Some(0), &[10, 77]),
            wire(6, 0, None, &[10, 77]),
            wire(6, 4, Some(1), &[10, 77]),
        ] {
            assert_eq!(binding.read(body.span(), |_| true), Err(Error::Shape));
        }
        assert_eq!(
            binding.read(wire(7, 0, None, &[10, 77]).span(), |_| true),
            Ok(None)
        );
        assert_eq!(Binding::new(0, 0), Err(Error::Binding));
    }
}
