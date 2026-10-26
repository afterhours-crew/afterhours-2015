// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! FollowTargetGoal (8) and ChaseTargetGoal (3). Follow has a completable-goal
//! RPC identity and an optional ordered triple; chase has neither. Both native
//! update serializers are empty. The triple's gameplay meaning is not assumed.
use super::{Error, Reader, Writer};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Follow {
    pub selector: u16,
    pub serial: u16,
    pub allocation_serial: u8,
    /// Ghost reference, zero when no target is supplied.
    pub target: u16,
    /// Finite f32 bit patterns in native wire order.
    pub parameters: Option<[u32; 3]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chase {
    pub allocation_serial: u8,
    pub target: u16,
}

fn finite(values: [u32; 3]) -> Result<[u32; 3], Error> {
    if values.into_iter().all(|v| f32::from_bits(v).is_finite()) {
        Ok(values)
    } else {
        Err(Error::Shape)
    }
}

impl Follow {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            selector: r.take(9)? as u16,
            serial: r.take(10)? as u16,
            allocation_serial: r.take(6)? as u8,
            target: r.reference()?,
            parameters: r.optional(|r| finite([r.take(32)?, r.take(32)?, r.take(32)?]))?,
        })
    }

    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.put(u64::from(self.selector), 9)?;
        w.put(u64::from(self.serial), 10)?;
        w.put(u64::from(self.allocation_serial), 6)?;
        w.reference(&self.target)?;
        w.optional(&self.parameters, |w, values| {
            for value in finite(*values)? {
                w.put(u64::from(value), 32)?;
            }
            Ok(())
        })
    }
}

impl Chase {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            allocation_serial: r.take(6)? as u8,
            target: r.reference()?,
        })
    }

    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.put(u64::from(self.allocation_serial), 6)?;
        w.reference(&self.target)
    }
}

#[cfg(test)]
mod tests;
