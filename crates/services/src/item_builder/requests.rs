// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Begin,
    Equip {
        vehicle: u16,
        item: u64,
    },
    Dyno {
        vehicle: u16,
        stats: nfs_world_core::items::DynoStats,
    },
    Commit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Call {
    pub player: u16,
    pub operation: Operation,
}

impl Binding {
    /// Decode the supported calls at the current scene binding. The caller must
    /// additionally resolve the equip vehicle to the same player's garage car.
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
            || !matches!(route.method_index(), 0 | 3 | 4 | 5)
        {
            return Ok(None);
        }
        let vehicle_call = matches!(route.method_index(), 0 | 3);
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || envelope.references().len() != if vehicle_call { 3 } else { 2 }
        {
            return Err(Error::Shape);
        }
        let player = envelope.references()[1];
        if player == 0 || !owns(player) {
            return Err(Error::Ownership);
        }
        let operation = match route.method_index() {
            0 => {
                if route.arguments().len() != 71 {
                    return Err(Error::Shape);
                }
                let vehicle = envelope.references()[2];
                let low = route
                    .arguments()
                    .read_u32(0, 32)
                    .map_err(|_| Error::Shape)?;
                let high = route
                    .arguments()
                    .read_u32(32, 32)
                    .map_err(|_| Error::Shape)?;
                let item = u64::from(low) | (u64::from(high) << 32);
                if vehicle == 0 || item == 0 {
                    return Err(Error::Shape);
                }
                Operation::Equip { vehicle, item }
            }
            3 => {
                let vehicle = envelope.references()[2];
                if vehicle == 0 {
                    return Err(Error::Shape);
                }
                let (stats, bits) = nfs_world_core::items::DynoStats::read(route.arguments())
                    .map_err(|_| Error::Shape)?;
                // The route is 41 bits; the native RPC pads the entire payload.
                let padding = (8 - (41 + bits) % 8) % 8;
                if route.arguments().len() != bits + padding {
                    return Err(Error::Shape);
                }
                Operation::Dyno { vehicle, stats }
            }
            4 | 5 => {
                if route.arguments().len() != 7 {
                    return Err(Error::Shape);
                }
                if route.method_index() == 4 {
                    Operation::Begin
                } else {
                    Operation::Commit
                }
            }
            _ => return Err(Error::Shape),
        };
        // Native alignment bits are not request arguments.
        Ok(Some(Call { player, operation }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world_core::bits::BitWriter;

    fn equip(item: u64) -> BitWriter {
        let mut payload = BitWriter::new();
        payload
            .put(6, 9)
            .put(0, 32)
            .put(item as u32 as u64, 32)
            .put(item >> 32, 32)
            .put(127, 7);
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(3, 8)
            .put(10, 13)
            .put(12, 13)
            .put(15, 13)
            .put(14, 9)
            .put_span(payload.span());
        body
    }
    #[test]
    fn equip_low_word_precedes_high_word_and_every_extent_is_checked() {
        let binding = Binding::new(10, 6).unwrap();
        let item = 0x12345678_90abcdef;
        let wire = equip(item);
        assert_eq!(
            binding.read(wire.span(), |p| p == 12),
            Ok(Some(Call {
                player: 12,
                operation: Operation::Equip { vehicle: 15, item },
            }))
        );
        for length in 0..wire.len() {
            assert_eq!(
                binding.read(wire.span().slice(0, length).unwrap(), |_| true),
                Err(Error::Shape)
            );
        }
        assert_eq!(binding.read(wire.span(), |_| false), Err(Error::Ownership));
        assert_eq!(binding.read(equip(0).span(), |_| true), Err(Error::Shape));
        assert_eq!(
            Binding::new(11, 6).unwrap().read(wire.span(), |_| true),
            Ok(None)
        );
        assert_eq!(
            Binding::new(10, 7).unwrap().read(wire.span(), |_| true),
            Ok(None)
        );
        let mut trailing = wire.clone();
        trailing.put(0, 1);
        assert_eq!(binding.read(trailing.span(), |_| true), Err(Error::Shape));
    }
    #[test]
    fn begin_and_commit_have_only_the_owned_player_reference() {
        let binding = Binding::new(19, 5).unwrap();
        for (method, operation) in [(4, Operation::Begin), (5, Operation::Commit)] {
            let wire = super::super::tests::wire(19, 5, method, &[23], 127);
            assert_eq!(
                binding.read(wire.span(), |p| p == 23),
                Ok(Some(Call {
                    player: 23,
                    operation
                }))
            );
            let bad = super::super::tests::wire(19, 5, method, &[23, 24], 0);
            assert_eq!(binding.read(bad.span(), |_| true), Err(Error::Shape));
        }
    }

    #[test]
    fn dyno_requires_owned_player_vehicle_and_exact_variable_extent() {
        use nfs_world_core::items::DynoStats;
        let binding = Binding::new(10, 6).unwrap();
        for stats in [
            DynoStats::new([50; 4], [0.; 3]).unwrap(),
            DynoStats::new([200, 450, 600, 120], [4.5, 0., 12.]).unwrap(),
            DynoStats::new([300, 1500, 1500, 200], [15., 35., 20.]).unwrap(),
        ] {
            let make = |vehicle, extra| {
                let mut payload = BitWriter::new();
                payload.put(6, 9).put(3, 32).put_span(stats.encode().span());
                payload.align();
                if extra {
                    payload.put(0, 8);
                }
                let mut wire = BitWriter::new();
                wire.put(0, 32)
                    .put(0, 32)
                    .put(3, 8)
                    .put(10, 13)
                    .put(12, 13)
                    .put(vehicle, 13)
                    .put((payload.len() / 8) as u64, 9)
                    .put_span(payload.span());
                wire
            };
            let wire = make(15, false);
            assert_eq!(
                binding.read(wire.span(), |p| p == 12),
                Ok(Some(Call {
                    player: 12,
                    operation: Operation::Dyno { vehicle: 15, stats },
                }))
            );
            for end in 0..wire.len() {
                assert!(
                    binding
                        .read(wire.span().slice(0, end).unwrap(), |_| true)
                        .is_err()
                );
            }
            assert_eq!(binding.read(wire.span(), |_| false), Err(Error::Ownership));
            assert_eq!(
                binding.read(make(0, false).span(), |_| true),
                Err(Error::Shape)
            );
            assert_eq!(
                binding.read(make(15, true).span(), |_| true),
                Err(Error::Shape)
            );
        }
    }
}
