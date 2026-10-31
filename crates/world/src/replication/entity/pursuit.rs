// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Wire layouts for inspection. No pursuit lifecycle or reward policy lives here.
use super::{Error, MAX_REFERENCES, Reader, Writer};

/// The native list count is a signed byte. Negative counts are rejected.
pub const MAX_FINE_ENTRIES: usize = 127;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FineSnapshot {
    pub values: [i8; 3],
    pub entries: Vec<(i8, i8, i16)>,
}
impl FineSnapshot {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let values = [r.take(8)? as i8, r.take(8)? as i8, r.take(8)? as i8];
        let count = r.take(8)? as usize;
        if count > MAX_FINE_ENTRIES {
            return Err(Error::Bound);
        }
        let entries = (0..count)
            .map(|_| Ok((r.take(8)? as i8, r.take(8)? as i8, r.take(16)? as i16)))
            .collect::<Result<_, Error>>()?;
        Ok(Self { values, entries })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        if self.entries.len() > MAX_FINE_ENTRIES {
            return Err(Error::Bound);
        }
        for value in self.values {
            w.put(u64::from(value as u8), 8)?;
        }
        w.put(self.entries.len() as u64, 8)?;
        for (first, second, value) in &self.entries {
            w.put(u64::from(*first as u8), 8)?;
            w.put(u64::from(*second as u8), 8)?;
            w.put(u64::from(*value as u16), 16)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChaseUpdate {
    pub value: Option<u8>,
    pub participant: Option<u16>,
    pub flag: Option<bool>,
}
impl ChaseUpdate {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            value: r.optional(|r| Ok(r.take(8)? as u8))?,
            participant: r.optional(Reader::reference)?,
            flag: r.optional(Reader::bit)?,
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.optional(&self.value, |w, v| w.put(u64::from(*v), 8))?;
        w.optional(&self.participant, Writer::reference)?;
        w.optional(&self.flag, |w, v| w.bit(*v))
    }
}

/// Preserve absent lists separately from empty lists. These are wire values;
/// the codec does not infer whether the client treats a list as an accumulated
/// delta or a replacement of gameplay state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetVisibilityUpdate {
    pub participant: Option<u16>,
    pub references: Option<Vec<u16>>,
}
impl TargetVisibilityUpdate {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            participant: r.optional(Reader::reference)?,
            references: r.optional(|r| read_references(r, 16))?,
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.optional(&self.participant, Writer::reference)?;
        w.optional(&self.references, |w, v| write_references(w, v, 16))
    }
}

pub(super) fn read_references(r: &mut Reader<'_>, width: u8) -> Result<Vec<u16>, Error> {
    let count = r.take(width)? as usize;
    if count > MAX_REFERENCES {
        return Err(Error::Bound);
    }
    (0..count).map(|_| r.reference()).collect()
}
pub(super) fn write_references(w: &mut Writer, values: &[u16], width: usize) -> Result<(), Error> {
    if values.len() > MAX_REFERENCES || values.len() >= 1usize << width {
        return Err(Error::Bound);
    }
    w.put(values.len() as u64, width)?;
    values.iter().try_for_each(|v| w.reference(v))
}
