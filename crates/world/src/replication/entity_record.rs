// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Decoded, Error, Initial, MAX_RECORD_BITS, Record, Update, entity};
use crate::bits::BitWriter;
use entity::creation::{Binding, Content, Creation};
use nfs_protocol::world::BitSpan;
use std::sync::Arc;

impl Record {
    pub fn from_entity(id: u16, creation: Creation, content: &Content) -> Result<Self, Error> {
        let binding = Arc::new(content.binding(creation.prefix.asset)?);
        let result = Self {
            id,
            initial: Some(Initial::Entity {
                prefix: creation.prefix,
                fields: creation.body.initial.ok_or(Error::Shape)?,
            }),
            update: Update::Entity {
                binding,
                fields: creation.body.updates,
            },
        };
        result.encode()?;
        Ok(result)
    }

    pub fn entity_binding(&self) -> Option<&Arc<Binding>> {
        match &self.update {
            Update::Entity { binding, .. } | Update::Vehicle { binding, .. } => Some(binding),
            _ => None,
        }
    }

    pub(crate) fn entity_wire(&self) -> Result<BitWriter, Error> {
        let Update::Entity { binding, fields } = &self.update else {
            return Err(Error::TypeMismatch);
        };
        let (mut wire, initial) = match &self.initial {
            Some(Initial::Entity { prefix, fields }) => {
                (binding.prefix(prefix)?, Some(fields.clone()))
            }
            None => (BitWriter::new(), None),
            _ => return Err(Error::TypeMismatch),
        };
        let body = entity::Body {
            initial,
            updates: fields.clone(),
        }
        .encode(binding.profile()?)?;
        if wire.len() + body.len() > MAX_RECORD_BITS {
            return Err(Error::Bound);
        }
        wire.put_span(body.span());
        Ok(wire)
    }

    pub(crate) fn decode_entity(
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
            let (prefix, _) = entity::creation::Prefix::decode(span, content)?;
            if matches!(
                content.binding(prefix.asset)?.composition(),
                entity::creation::Composition::Vehicle(_)
            ) {
                return Self::decode_vehicle(input, id, create, header, Some(content), None);
            }
            let decoded = Creation::decode(span, content)?;
            (
                Self::from_entity(id, decoded.creation, content)?,
                Some(header + decoded.initial_bits),
                header + decoded.bits,
            )
        } else {
            let binding = binding.ok_or(Error::UnknownObject)?;
            if matches!(
                binding.composition(),
                entity::creation::Composition::Vehicle(_)
            ) {
                return Self::decode_vehicle(input, id, create, header, None, Some(binding));
            }
            let decoded = entity::Body::decode(span, binding.profile()?, false)?;
            (
                Self {
                    id,
                    initial: None,
                    update: Update::Entity {
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
