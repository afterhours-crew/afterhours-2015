// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{bits::BitWriter, participants::Endpoint, replication::Error};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

pub mod presence;
pub mod vehicle;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Slots([Option<u64>; 5]);
impl Slots {
    pub fn new(ids: [Option<u64>; 5]) -> Result<Self, Error> {
        for (i, id) in ids.iter().enumerate() {
            if let Some(id) = id
                && (*id == 0 || ids[..i].contains(&Some(*id)))
            {
                return Err(Error::Shape);
            }
        }
        Ok(Self(ids))
    }
    pub fn values(self) -> [Option<u64>; 5] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    pub endpoint: Endpoint,
    pub participant: u16,
    pub item: u64,
}
impl Binding {
    pub fn encode(self) -> Result<BitWriter, Error> {
        if self.endpoint.scene == 0
            || self.endpoint.scene > 8191
            || self.participant == 0
            || self.participant > 8191
            || self.endpoint.selector > 511
            || self.item == 0
        {
            return Err(Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload
            .put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(1, 32)
            .put(self.item & 0xffff_ffff, 32)
            .put(self.item >> 32, 32)
            .align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(self.endpoint.scene.into(), 13)
            .put(self.participant.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
    pub fn decode(body: BitSpan<'_>) -> Result<Self, Error> {
        let e = Envelope::decode(
            body,
            Limits {
                max_input_bits: 227,
                max_references: 2,
                max_payload_bytes: 15,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = e
            .route(RouteProfile::ClientReceive)
            .map_err(|_| Error::Shape)?;
        if e.words() != [0, 0]
            || e.references().len() != 2
            || !e.remaining().is_empty()
            || route.method_index() != 1
            || route.arguments().len() != 69
        {
            return Err(Error::Shape);
        }
        let args = route.arguments();
        let low = args.read_u32(0, 32).map_err(|_| Error::Shape)?;
        let high = args.read_u32(32, 32).map_err(|_| Error::Shape)?;
        let result = Self {
            endpoint: Endpoint {
                scene: e.references()[0],
                selector: route.selector(),
                serial: route.serial().ok_or(Error::Shape)?,
            },
            participant: e.references()[1],
            item: u64::from(low) | (u64::from(high) << 32),
        };
        result.encode()?;
        Ok(result)
    }
}
