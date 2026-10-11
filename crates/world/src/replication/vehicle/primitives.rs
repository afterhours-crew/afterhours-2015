// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Reader, Writer};

pub(super) fn array<T, const N: usize>(
    mut read: impl FnMut() -> Result<T, Error>,
) -> Result<[T; N], Error> {
    (0..N)
        .map(|_| read())
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| Error::Shape)
}
pub(super) fn signed(r: &mut Reader<'_>, width: u8) -> Result<i32, Error> {
    if !(1..=32).contains(&width) {
        return Err(Error::Shape);
    }
    Ok((r.take(width)? as i32).wrapping_shl(u32::from(32 - width)) >> (32 - width))
}
pub(super) fn put_signed(w: &mut Writer, value: i32, width: u8) -> Result<(), Error> {
    if !(1..=32).contains(&width) {
        return Err(Error::Shape);
    }
    let limit = 1i64 << (width - 1);
    if i64::from(value) < -limit || i64::from(value) >= limit {
        return Err(Error::Shape);
    }
    w.put(
        u64::from(value as u32) & ((1u64 << width) - 1),
        usize::from(width),
    )
}
fn width(mode: u8, fractional: u8) -> Result<u8, Error> {
    if mode > 7 || fractional > 16 {
        return Err(Error::Shape);
    }
    Ok([1, 3, 5, 7, 9, 11, 14][usize::from(mode.max(1) - 1)] + fractional)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Vector {
    FloatBits([u32; 3]),
    Packed { mode: u8, values: [i32; 3] },
}
impl Vector {
    pub(super) fn read(r: &mut Reader<'_>, fractional: u8) -> Result<Self, Error> {
        let mode = r.take(3)? as u8;
        Ok(if mode == 0 {
            Self::FloatBits(array(|| r.take(32))?)
        } else {
            Self::Packed {
                mode,
                values: array(|| signed(r, width(mode, fractional)?))?,
            }
        })
    }
    pub(super) fn write(&self, w: &mut Writer, fractional: u8) -> Result<(), Error> {
        match self {
            Self::FloatBits(values) => {
                w.put(0, 3)?;
                for v in values {
                    w.put(u64::from(*v), 32)?;
                }
            }
            Self::Packed { mode, values } => {
                if *mode == 0 {
                    return Err(Error::Shape);
                }
                let n = width(*mode, fractional)?;
                w.put(u64::from(*mode), 3)?;
                for v in values {
                    put_signed(w, *v, n)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SparseVector {
    FloatBits([u32; 3]),
    Packed {
        mode: Option<u8>,
        values: [Option<i32>; 3],
    },
}
impl SparseVector {
    pub(super) fn read(r: &mut Reader<'_>, fractional: u8) -> Result<Self, Error> {
        let mut mode = None;
        let mut values = [None; 3];
        for (i, value) in values.iter_mut().enumerate() {
            if !r.bit()? {
                continue;
            }
            let m = match mode {
                Some(m) => m,
                None => {
                    let m = r.take(3)? as u8;
                    mode = Some(m);
                    m
                }
            };
            if m == 0 && i == 0 {
                return Ok(Self::FloatBits(array(|| r.take(32))?));
            }
            *value = Some(signed(r, width(m, fractional)?)?);
        }
        Ok(Self::Packed { mode, values })
    }
    pub(super) fn write(&self, w: &mut Writer, fractional: u8) -> Result<(), Error> {
        match self {
            Self::FloatBits(values) => {
                w.bit(true)?;
                w.put(0, 3)?;
                for v in values {
                    w.put(u64::from(*v), 32)?;
                }
            }
            Self::Packed { mode, values } => {
                if mode.is_some() != values.iter().any(Option::is_some)
                    || (*mode == Some(0) && values[0].is_some())
                {
                    return Err(Error::Shape);
                }
                let mut first = true;
                for v in values {
                    w.bit(v.is_some())?;
                    if let Some(v) = v {
                        let m = mode.ok_or(Error::Shape)?;
                        if first {
                            w.put(u64::from(m), 3)?;
                            first = false;
                        }
                        put_signed(w, *v, width(m, fractional)?)?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Axis25 {
    pub negative: bool,
    pub mantissa: Option<u32>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Quaternion25 {
    pub negative_w: bool,
    pub axes: [Axis25; 3],
}
impl Quaternion25 {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            negative_w: r.bit()?,
            axes: array(|| {
                Ok(Axis25 {
                    negative: r.bit()?,
                    mantissa: if r.bit()? { None } else { Some(r.take(23)?) },
                })
            })?,
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.bit(self.negative_w)?;
        for a in &self.axes {
            w.bit(a.negative)?;
            w.bit(a.mantissa.is_none())?;
            if let Some(v) = a.mantissa {
                w.put(u64::from(v), 23)?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Quaternion14 {
    pub negative_w: bool,
    pub axes: [Option<i16>; 3],
}
impl Quaternion14 {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            negative_w: r.bit()?,
            axes: array(|| r.optional(|r| Ok(signed(r, 14)? as i16)))?,
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.bit(self.negative_w)?;
        for a in &self.axes {
            w.optional(a, |w, v| put_signed(w, i32::from(*v), 14))?;
        }
        Ok(())
    }
}
pub(super) fn sparse_float_read(r: &mut Reader<'_>) -> Result<[Option<u32>; 3], Error> {
    array(|| r.optional(|r| r.take(32)))
}
pub(super) fn sparse_float_write(w: &mut Writer, values: &[Option<u32>; 3]) -> Result<(), Error> {
    for v in values {
        w.optional(v, |w, v| w.put(u64::from(*v), 32))?;
    }
    Ok(())
}

#[derive(Clone, Eq, PartialEq)]
pub struct Resource {
    pub words: [u32; 2],
    pub name: Vec<u8>,
}
impl std::fmt::Debug for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resource")
            .field("name_bytes", &self.name.len())
            .finish_non_exhaustive()
    }
}
impl Resource {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            words: array(|| r.take(32))?,
            name: r.string(5, 16)?,
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        for v in self.words {
            w.put(u64::from(v), 32)?;
        }
        w.string(&self.name, 5, 16)
    }
}
