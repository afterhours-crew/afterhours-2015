// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant locations. Initialization is empty; a present update replaces
//! the complete grouped map. Group values are preserved without interpreting
//! unobserved meanings. X/Z are signed-magnitude coordinates over +/-8000.
use super::{Error, Reader, Writer};

pub const MAX_GROUPS: usize = 255;
pub const MAX_ENTRIES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Coordinate(u16);
impl Coordinate {
    pub fn from_raw(raw: u16) -> Result<Self, Error> {
        if raw > 0xfff {
            return Err(Error::Shape);
        }
        Ok(Self(raw))
    }
    pub fn raw(self) -> u16 {
        self.0
    }
    pub fn from_world(value: f32) -> Result<Self, Error> {
        if !value.is_finite() {
            return Err(Error::Shape);
        }
        let normalized = value.clamp(-8000., 8000.) / 8000.;
        let magnitude = (normalized.abs() * 2047. + 0.5) as u16;
        Ok(Self(
            ((normalized.is_sign_negative() as u16) << 11) | magnitude,
        ))
    }
    pub fn to_world(self) -> f32 {
        let magnitude = f32::from(self.0 & 0x7ff) / 2047. * 8000.;
        if self.0 & 0x800 != 0 {
            -magnitude
        } else {
            magnitude
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub participant: u16,
    pub x: Coordinate,
    pub z: Coordinate,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Group {
    pub value: u8,
    pub entries: Vec<Entry>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Update {
    /// None leaves the map unchanged; Some(empty) removes all entries.
    pub groups: Option<Vec<Group>>,
}
impl Update {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let groups = r.optional(|r| {
            let count = r.take(8)? as usize;
            let mut total = 0;
            let mut groups = Vec::with_capacity(count);
            for _ in 0..count {
                let count = r.take(16)? as usize;
                total += count;
                if total > MAX_ENTRIES {
                    return Err(Error::Bound);
                }
                let value = r.take(8)? as u8;
                if count > r.span.len().saturating_sub(r.pos) / 37 {
                    return Err(Error::Truncated);
                }
                let entries = (0..count)
                    .map(|_| {
                        Ok(Entry {
                            participant: r.reference()?,
                            x: Coordinate(r.take(12)? as u16),
                            z: Coordinate(r.take(12)? as u16),
                        })
                    })
                    .collect::<Result<_, Error>>()?;
                groups.push(Group { value, entries });
            }
            Ok(groups)
        })?;
        Ok(Self { groups })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.optional(&self.groups, |w, groups| {
            if groups.len() > MAX_GROUPS
                || groups.iter().map(|g| g.entries.len()).sum::<usize>() > MAX_ENTRIES
            {
                return Err(Error::Bound);
            }
            w.put(groups.len() as u64, 8)?;
            for group in groups {
                w.put(group.entries.len() as u64, 16)?;
                w.put(u64::from(group.value), 8)?;
                for entry in &group.entries {
                    w.reference(&entry.participant)?;
                    w.put(u64::from(entry.x.raw()), 12)?;
                    w.put(u64::from(entry.z.raw()), 12)?;
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
