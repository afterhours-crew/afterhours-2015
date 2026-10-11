// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Decoded, Error, Initial, MAX_RECORD_BITS, Record, Update, sublevel};
use crate::bits::BitWriter;
use nfs_protocol::world::BitSpan;

impl Record {
    pub fn from_scene(
        id: u16,
        creation: sublevel::Creation,
        content: &sublevel::Content,
    ) -> Result<Self, Error> {
        creation.encode(content)?;
        let profile = content.profile(creation.prefix.content_key)?.clone();
        let result = Self {
            id,
            initial: Some(Initial::SubLevel {
                prefix: creation.prefix,
                fields: creation.body.initial.ok_or(Error::Shape)?,
            }),
            update: Update::SubLevel {
                profile,
                fields: creation.body.updates,
            },
        };
        result.encode()?;
        Ok(result)
    }
    pub(crate) fn scene_profile(&self) -> Option<&sublevel::Profile> {
        match &self.update {
            Update::SubLevel { profile, .. } => Some(profile),
            _ => None,
        }
    }
    pub(crate) fn scene_wire(&self) -> Result<BitWriter, Error> {
        let Update::SubLevel { profile, fields } = &self.update else {
            return Err(Error::TypeMismatch);
        };
        match &self.initial {
            Some(Initial::SubLevel {
                prefix,
                fields: initial,
            }) => {
                let content = sublevel::Content::new(vec![(prefix.content_key, profile.clone())])?;
                sublevel::Creation {
                    prefix: prefix.clone(),
                    body: sublevel::Body {
                        initial: Some(initial.clone()),
                        updates: fields.clone(),
                    },
                }
                .encode(&content)
            }
            None => sublevel::Body {
                initial: None,
                updates: fields.clone(),
            }
            .encode(profile),
            _ => Err(Error::TypeMismatch),
        }
    }
    pub(crate) fn decode_scene(
        input: BitSpan<'_>,
        id: u16,
        create: bool,
        header: usize,
        content: Option<&sublevel::Content>,
        profile: Option<&sublevel::Profile>,
    ) -> Result<Decoded, Error> {
        let body = input.after(header).map_err(|_| Error::Truncated)?;
        let (record, initial_bits, bits) = if create {
            let content = content.ok_or(Error::Unsupported)?;
            let decoded = sublevel::Creation::decode(body, content)?;
            (
                Self::from_scene(id, decoded.creation, content)?,
                Some(header + decoded.initial_bits),
                header + decoded.bits,
            )
        } else {
            let profile = profile.ok_or(Error::UnknownObject)?;
            let decoded = sublevel::Body::decode(body, profile, false)?;
            (
                Record {
                    id,
                    initial: None,
                    update: Update::SubLevel {
                        profile: profile.clone(),
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
