// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{bits::BitWriter, participants::Endpoint, replication::Error};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    pub endpoint: Endpoint,
    pub participant: u16,
    pub actor: u16,
}
impl Binding {
    pub fn encode(self) -> Result<BitWriter, Error> {
        if [self.endpoint.scene, self.participant, self.actor]
            .into_iter()
            .any(|id| id == 0 || id > 8191)
            || self.endpoint.selector > 511
        {
            return Err(Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload
            .put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(0, 32)
            .align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(3, 8)
            .put(self.endpoint.scene.into(), 13)
            .put(self.participant.into(), 13)
            .put(self.actor.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
    pub fn decode(body: BitSpan<'_>) -> Result<Self, Error> {
        let e = Envelope::decode(
            body,
            Limits {
                max_input_bits: 176,
                max_references: 3,
                max_payload_bytes: 7,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = e
            .route(RouteProfile::ClientReceive)
            .map_err(|_| Error::Shape)?;
        if e.words() != [0, 0]
            || e.references().len() != 3
            || !e.remaining().is_empty()
            || route.method_index() != 0
            || route.arguments().len() != 5
        {
            return Err(Error::Shape);
        }
        let result = Self {
            endpoint: Endpoint {
                scene: e.references()[0],
                selector: route.selector(),
                serial: route.serial().ok_or(Error::Shape)?,
            },
            participant: e.references()[1],
            actor: e.references()[2],
        };
        result.encode()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_protocol::world::rpc::Serial;
    #[test]
    fn route_bounds_truncation_extra_data_and_padding() {
        let call = Binding {
            endpoint: Endpoint {
                scene: 17,
                selector: 511,
                serial: Serial::new(1023).unwrap(),
            },
            participant: 19,
            actor: 8191,
        };
        let wire = call.encode().unwrap();
        assert_eq!(wire.len(), 176);
        assert_eq!(Binding::decode(wire.span()), Ok(call));
        for bits in 0..176 {
            assert!(Binding::decode(BitSpan::new(wire.bytes(), 0, bits).unwrap()).is_err());
        }
        let mut raw = wire.bytes().to_vec();
        raw.push(0);
        assert!(Binding::decode(BitSpan::new(&raw, 0, 177).unwrap()).is_err());
        raw[21] |= 31;
        assert_eq!(
            Binding::decode(BitSpan::new(&raw, 0, 176).unwrap()),
            Ok(call)
        );
        raw[21] |= 32;
        assert!(Binding::decode(BitSpan::new(&raw, 0, 176).unwrap()).is_err());
        for id in [0, 8192, u16::MAX] {
            assert!(Binding { actor: id, ..call }.encode().is_err());
            assert!(
                Binding {
                    participant: id,
                    ..call
                }
                .encode()
                .is_err()
            );
            assert!(
                Binding {
                    endpoint: Endpoint {
                        scene: id,
                        ..call.endpoint
                    },
                    ..call
                }
                .encode()
                .is_err()
            );
        }
    }
}
