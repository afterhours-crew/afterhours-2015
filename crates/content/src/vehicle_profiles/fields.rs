// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{Error, scan::Assets};
use nfs_frostbite::ebx::{Fields, Object, Pointer, Value};

pub(super) type Key = (String, usize);
pub(super) fn bad(what: &str) -> Error {
    Error::Content(format!("vehicle profiles: {what}"))
}
pub(super) fn get<'a>(fields: &'a Fields, name: &str) -> Result<&'a Value, Error> {
    fields
        .get(name)
        .ok_or_else(|| bad(&format!("missing {name}")))
}
pub(super) fn array<'a>(fields: &'a Fields, name: &str, max: usize) -> Result<&'a [Value], Error> {
    get(fields, name)?
        .as_array()
        .filter(|a| a.len() <= max)
        .ok_or_else(|| bad(name))
}
pub(super) fn structure<'a>(fields: &'a Fields, name: &str) -> Result<&'a Fields, Error> {
    get(fields, name)?.as_fields().ok_or_else(|| bad(name))
}
pub(super) fn text(fields: &Fields, name: &str) -> Result<String, Error> {
    get(fields, name)?
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or_else(|| bad(name))
}
pub(super) fn boolean(fields: &Fields, name: &str) -> Result<bool, Error> {
    get(fields, name)?.as_bool().ok_or_else(|| bad(name))
}
pub(super) fn integer(fields: &Fields, name: &str) -> Result<i64, Error> {
    match get(fields, name)? {
        Value::Int8(v) => Ok(i64::from(*v)),
        Value::UInt8(v) => Ok(i64::from(*v)),
        Value::Int16(v) => Ok(i64::from(*v)),
        Value::UInt16(v) => Ok(i64::from(*v)),
        Value::Int32(v) => Ok(i64::from(*v)),
        Value::UInt32(v) => Ok(i64::from(*v)),
        _ => Err(bad(name)),
    }
}
pub(super) fn word(fields: &Fields, name: &str) -> Result<u32, Error> {
    match get(fields, name)? {
        Value::UInt16(v) => Ok(u32::from(*v)),
        Value::Int32(v) => Ok(*v as u32),
        Value::UInt32(v) => Ok(*v),
        _ => Err(bad(name)),
    }
}

pub(super) fn float(fields: &Fields, name: &str) -> Result<f32, Error> {
    match get(fields, name)? {
        Value::Float32(v) if v.is_finite() => Ok(*v),
        _ => Err(bad(name)),
    }
}
pub(super) fn vector<const N: usize>(
    fields: &Fields,
    name: &str,
    axes: [&str; N],
) -> Result<[f32; N], Error> {
    let fields = structure(fields, name)?;
    let mut out = [0.; N];
    for (value, axis) in out.iter_mut().zip(axes) {
        *value = float(fields, axis)?;
    }
    Ok(out)
}
pub(super) fn enumeration(fields: &Fields, name: &str) -> Result<i32, Error> {
    match get(fields, name)? {
        Value::Enum { value, .. } => Ok(*value),
        _ => Err(bad(name)),
    }
}
pub(super) fn pointer(fields: &Fields, name: &str) -> Result<Pointer, Error> {
    get(fields, name)?.as_pointer().ok_or_else(|| bad(name))
}
pub(super) fn resolve(assets: &mut Assets, source: &str, p: Pointer) -> Result<Key, Error> {
    assets
        .target(source, p)?
        .ok_or_else(|| bad("null required reference"))
}
pub(super) fn object(assets: &mut Assets, key: &Key) -> Result<Object, Error> {
    assets
        .objects(&key.0)?
        .get(key.1)
        .cloned()
        .ok_or_else(|| bad("object index"))
}
pub(super) fn follow(
    assets: &mut Assets,
    source: &str,
    fields: &Fields,
    field: &str,
    class: &str,
) -> Result<(Key, Object), Error> {
    let key = resolve(assets, source, pointer(fields, field)?)?;
    let value = object(assets, &key)?;
    if value.class != class {
        return Err(bad(&format!("{field} target class {}", value.class)));
    }
    Ok((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stored_widths_preserve_blueprint_flags_and_signed_port_bits() {
        let fields = Fields(vec![
            ("Flags".into(), Value::UInt16(0x800f)),
            ("Port".into(), Value::Int32(-1)),
            ("Width".into(), Value::UInt8(4)),
            ("Invalid".into(), Value::Float32(4.)),
        ]);
        assert_eq!(word(&fields, "Flags").unwrap(), 0x800f);
        assert_eq!(word(&fields, "Port").unwrap(), u32::MAX);
        assert_eq!(integer(&fields, "Width").unwrap(), 4);
        assert!(word(&fields, "Invalid").is_err());
        assert!(integer(&fields, "Invalid").is_err());
    }
}
