// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! BankEntity's account-targeted updateBalance notification. The scene binding
//! comes from installed content; the balance comes from the recipient's account.
use crate::{bits::BitWriter, participants::Endpoint, replication::Error};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Balance {
    pub endpoint: Endpoint,
    pub amount: u32,
}

impl Balance {
    pub fn encode(self) -> Result<BitWriter, Error> {
        if self.endpoint.scene == 0 || self.endpoint.scene > 8191 || self.endpoint.selector > 511 {
            return Err(Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload
            .put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(0, 32)
            .put(self.amount.into(), 32)
            .align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(1, 8)
            .put(self.endpoint.scene.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }

    pub fn decode(body: BitSpan<'_>) -> Result<Self, Error> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 182,
                max_references: 1,
                max_payload_bytes: 11,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientReceive)
            .map_err(|_| Error::Shape)?;
        if envelope.words() != [0, 0]
            || envelope.references().len() != 1
            || !envelope.remaining().is_empty()
            || route.method_index() != 0
            || route.arguments().len() != 37
        {
            return Err(Error::Shape);
        }
        let result = Self {
            endpoint: Endpoint {
                scene: envelope.references()[0],
                selector: route.selector(),
                serial: route.serial().ok_or(Error::Shape)?,
            },
            amount: route
                .arguments()
                .read_u32(0, 32)
                .map_err(|_| Error::Shape)?,
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
    fn native_field_order_and_unsigned_balance() {
        // Authored fixture: two words, one scene reference, eleven-byte RPC;
        // selector7/serial3/method0/balance370 followed by five padding bits.
        let bytes = [
            0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0x98, 0x2c, 0x0e, 0x01, 0x80, 0, 0, 0, 0, 0, 0, 0xb9, 0,
        ];
        let value = Balance {
            endpoint: Endpoint {
                scene: 19,
                selector: 7,
                serial: Serial::new(3).unwrap(),
            },
            amount: 370,
        };
        assert_eq!(value.encode().unwrap().bytes(), bytes);
        assert_eq!(
            Balance::decode(BitSpan::new(&bytes, 0, 182).unwrap()),
            Ok(value)
        );
        for amount in [0, 1, i32::MAX as u32, u32::MAX] {
            let sample = Balance { amount, ..value };
            assert_eq!(Balance::decode(sample.encode().unwrap().span()), Ok(sample));
        }
    }

    #[test]
    fn rejects_partial_extra_and_wrong_route_data() {
        let value = Balance {
            endpoint: Endpoint {
                scene: 1,
                selector: 511,
                serial: Serial::new(1023).unwrap(),
            },
            amount: 123,
        };
        let wire = value.encode().unwrap();
        for len in 0..wire.len() {
            assert!(Balance::decode(wire.span().slice(0, len).unwrap()).is_err());
        }
        let mut extra = wire.clone();
        extra.put(0, 1);
        assert!(Balance::decode(extra.span()).is_err());
        for bit in [0, 71, 84, 144] {
            let mut bad = wire.bytes().to_vec();
            bad[bit / 8] ^= 1 << (7 - bit % 8);
            assert!(
                Balance::decode(BitSpan::new(&bad, 0, wire.len()).unwrap()).is_err(),
                "bit {bit}"
            );
        }
        for (scene, selector) in [(0, 0), (8192, 0), (1, 512)] {
            assert!(
                Balance {
                    endpoint: Endpoint {
                        scene,
                        selector,
                        ..value.endpoint
                    },
                    ..value
                }
                .encode()
                .is_err()
            );
        }
    }
}
