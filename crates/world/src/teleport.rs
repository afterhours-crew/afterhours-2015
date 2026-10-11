// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Host teleport of a participant's car out of the garage.
//!
//! Method 2 on the startup scene's TeleportingParticipantState endpoint
//! (field 77, wire selector 76) with references `[scene, participant,
//! vehicle]`. The arguments are the destination as three f32, its rotation
//! in the `Quaternion25` layout and 33 zero bits. The official hosts in E101
//! (100,891 ms) and E742 window 4 (125,909 ms) send identical arguments, the
//! SpawnPoints TeleportLocation by the garage, as chunked content (target
//! 47, the RPC message index), 0.7–0.9 s after the exit frame; the client
//! moves its car there 9–46 ms later and then requests its spawn (E767).

use crate::{
    bits::BitWriter,
    participants::Endpoint,
    replication::{Error, MAX_OBJECTS, vehicle::Quaternion25},
};

/// Host method on the TeleportingParticipantState endpoint.
pub const METHOD: u32 = 2;
/// Content target that carries a chunked RPC envelope.
pub const CONTENT_TARGET: u8 = 47;
/// Trailing argument bits, zero in both official commands.
const RESERVED_BITS: usize = 33;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Command {
    pub endpoint: Endpoint,
    pub participant: u16,
    pub vehicle: u16,
    pub position: [f32; 3],
    /// Rows: right, up, forward.
    pub basis: [[f32; 3]; 3],
}

impl Command {
    /// The RPC envelope bytes, as reassembled from the official chunks.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let ids = [self.endpoint.scene, self.participant, self.vehicle];
        if ids
            .iter()
            .any(|&id| id == 0 || usize::from(id) > MAX_OBJECTS)
            || self.endpoint.selector > 511
            || self.position.iter().any(|v| !v.is_finite())
        {
            return Err(Error::Bound);
        }
        let rotation = Quaternion25::from_basis(self.basis)?;
        let mut payload = BitWriter::new();
        payload
            .put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(METHOD.into(), 32);
        for v in self.position {
            payload.put(v.to_bits().into(), 32);
        }
        payload.put_bool(rotation.negative_w);
        for axis in &rotation.axes {
            payload
                .put_bool(axis.negative)
                .put_bool(axis.mantissa.is_none());
            if let Some(mantissa) = axis.mantissa {
                payload.put(mantissa.into(), 23);
            }
        }
        payload.put(0, RESERVED_BITS);
        payload.align();
        let mut body = BitWriter::new();
        body.put(0, 32).put(0, 32).put(ids.len() as u64, 8);
        for id in ids {
            body.put(id.into(), 13);
        }
        body.put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body.align();
        Ok(body.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_protocol::world::{
        BitSpan,
        rpc::{Envelope, Limits, RouteProfile, Serial},
    };

    fn command() -> Command {
        Command {
            endpoint: Endpoint {
                scene: 10,
                selector: 76,
                serial: Serial::new(3).unwrap(),
            },
            participant: 178,
            vehicle: 205,
            position: [12.5, -3.25, 400.0],
            // A quarter turn about the vertical axis.
            basis: [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
        }
    }

    #[test]
    fn envelope_carries_three_references_and_the_destination() {
        let bytes = command().encode().unwrap();
        let span = BitSpan::new(&bytes, 0, bytes.len() * 8).unwrap();
        let envelope = Envelope::decode(
            span,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .unwrap();
        assert_eq!(envelope.references(), &[10, 178, 205]);
        let route = envelope.route(RouteProfile::ClientReceive).unwrap();
        assert_eq!(
            (
                route.selector(),
                route.method_index(),
                route.serial().map(|s| s.value())
            ),
            (76, METHOD, Some(3))
        );
        let args = route.arguments();
        let f = |i: usize| f32::from_bits(args.read_u32(32 * i, 32).unwrap());
        assert_eq!([f(0), f(1), f(2)], [12.5, -3.25, 400.0]);
        // Quaternion25 after the position: w sign, then per axis a sign, an
        // absent flag and a 23-bit mantissa when present.
        let expected = Quaternion25::from_basis(command().basis).unwrap();
        let mut at = 96;
        let mut bit = |n: usize| {
            let v = args.read_u32(at, n as u8).unwrap();
            at += n;
            v
        };
        assert_eq!(bit(1) == 1, expected.negative_w);
        for axis in &expected.axes {
            assert_eq!(bit(1) == 1, axis.negative);
            assert_eq!(bit(1) == 1, axis.mantissa.is_none());
            if let Some(m) = axis.mantissa {
                assert_eq!(bit(23), m);
            }
        }
        // A quarter turn about y: only the y axis is present.
        assert_eq!(
            expected.axes.map(|a| a.mantissa.is_some()),
            [false, true, false]
        );
        // 33 reserved zero bits, then byte alignment of the payload.
        let rest = args.len() - at;
        assert!((33..33 + 8).contains(&rest));
        assert!((at..args.len()).all(|i| args.read_u32(i, 1).unwrap() == 0));
        assert_eq!((51 + args.len()) % 8, 0);
    }

    #[test]
    fn invalid_ids_positions_and_rotations_are_refused() {
        for change in [
            (|c: &mut Command| c.vehicle = 0) as fn(&mut Command),
            |c| c.participant = (MAX_OBJECTS + 1) as u16,
            |c| c.endpoint.selector = 512,
            |c| c.position[1] = f32::NAN,
        ] {
            let mut c = command();
            change(&mut c);
            assert_eq!(c.encode(), Err(Error::Bound));
        }
        let mut c = command();
        c.basis = [[0.0; 3]; 3];
        assert!(c.encode().is_err());
    }
}
