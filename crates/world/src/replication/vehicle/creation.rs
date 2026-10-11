// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Body;
use crate::bits::BitWriter;
use crate::replication::{
    Error, MAX_RECORD_BITS,
    entity::creation::{Content, Prefix},
};
use nfs_protocol::world::BitSpan;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntityCreation {
    pub prefix: Prefix,
    pub body: Body,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decoded {
    pub creation: EntityCreation,
    pub prefix_bits: usize,
    pub initial_bits: usize,
    pub bits: usize,
}
impl EntityCreation {
    pub fn decode(input: BitSpan<'_>, content: &Content) -> Result<Decoded, Error> {
        let (prefix, prefix_bits) = Prefix::decode(input, content)?;
        let body = Body::decode(
            input.after(prefix_bits).map_err(|_| Error::Truncated)?,
            content.vehicle_profile(prefix.asset)?,
            true,
        )?;
        let initial_bits = prefix_bits + body.initial_bits.ok_or(Error::Shape)?;
        let bits = prefix_bits + body.bits;
        if bits > MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        Ok(Decoded {
            creation: Self {
                prefix,
                body: body.body,
            },
            prefix_bits,
            initial_bits,
            bits,
        })
    }
    pub fn encode(&self, content: &Content) -> Result<BitWriter, Error> {
        if self.body.creation.is_none() {
            return Err(Error::Shape);
        }
        let mut wire = self.prefix.encode(content)?;
        let body = self
            .body
            .encode(content.vehicle_profile(self.prefix.asset)?)?;
        if wire.len() + body.len() > MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        wire.put_span(body.span());
        Ok(wire)
    }
    pub(crate) fn validate_references(&self, known: impl Fn(u16) -> bool) -> Result<(), Error> {
        let valid = |id| id == 0 || known(id);
        if !valid(self.prefix.blueprint)
            || self.prefix.owner.is_some_and(|id| !valid(id))
            || self
                .prefix
                .parent
                .as_ref()
                .and_then(|p| p.reference)
                .is_some_and(|id| !valid(id))
        {
            return Err(Error::UnknownObject);
        }
        Ok(())
    }
}
