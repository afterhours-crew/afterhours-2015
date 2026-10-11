// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::entity::creation::{Binding, Content};
use super::{Decoded, Error, Initial, MAX_RECORD_BITS, Record, Update, vehicle};
use crate::bits::BitWriter;
use nfs_protocol::world::BitSpan;
use std::sync::Arc;

impl Record {
    pub fn from_vehicle(
        id: u16,
        creation: vehicle::EntityCreation,
        content: &Content,
    ) -> Result<Self, Error> {
        let binding = Arc::new(content.binding(creation.prefix.asset)?);
        let result = Self {
            id,
            initial: Some(Initial::Vehicle {
                prefix: creation.prefix,
                creation: creation.body.creation.ok_or(Error::Shape)?,
            }),
            update: Update::Vehicle {
                binding,
                fields: creation.body.updates,
            },
        };
        result.encode()?;
        Ok(result)
    }

    pub(crate) fn vehicle_wire(&self) -> Result<BitWriter, Error> {
        let Update::Vehicle { binding, fields } = &self.update else {
            return Err(Error::TypeMismatch);
        };
        let (mut wire, creation) = match &self.initial {
            Some(Initial::Vehicle { prefix, creation }) => {
                (binding.prefix(prefix)?, Some(creation.clone()))
            }
            None => (BitWriter::new(), None),
            _ => return Err(Error::TypeMismatch),
        };
        let body = vehicle::Body {
            creation,
            updates: fields.clone(),
        }
        .encode(binding.vehicle_profile()?)?;
        if wire.len() + body.len() > MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        wire.put_span(body.span());
        Ok(wire)
    }

    pub(crate) fn decode_vehicle(
        input: BitSpan<'_>,
        id: u16,
        create: bool,
        header: usize,
        content: Option<&Content>,
        binding: Option<&Arc<Binding>>,
    ) -> Result<Decoded, Error> {
        let span = input.after(header).map_err(|_| Error::Truncated)?;
        let (record, initial_bits, bits) = if create {
            let content = content.ok_or(Error::Unsupported)?;
            let decoded = vehicle::EntityCreation::decode(span, content)?;
            (
                Self::from_vehicle(id, decoded.creation, content)?,
                Some(header + decoded.initial_bits),
                header + decoded.bits,
            )
        } else {
            let binding = binding.ok_or(Error::UnknownObject)?;
            let decoded = vehicle::Body::decode(span, binding.vehicle_profile()?, false)?;
            (
                Self {
                    id,
                    initial: None,
                    update: Update::Vehicle {
                        binding: Arc::clone(binding),
                        fields: decoded.body.updates,
                    },
                },
                None,
                header + decoded.bits,
            )
        };
        if bits > MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        Ok(Decoded {
            record,
            initial_bits,
            bits,
        })
    }
}
