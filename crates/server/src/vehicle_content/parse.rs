// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

pub(super) fn keys(v: &Value, required: &[&str], optional: &[&str]) -> Result<(), Failure> {
    let obj = v.as_object().ok_or(Failure::ProfileConfig)?;
    if required.iter().any(|k| !obj.contains_key(*k))
        || obj
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err(Failure::ProfileConfig);
    }
    Ok(())
}
pub(super) fn array(v: &Value, max: usize) -> Result<&[Value], Failure> {
    let a = v.as_array().ok_or(Failure::ProfileConfig)?;
    if a.len() > max {
        return Err(Failure::BodyLimit);
    }
    Ok(a)
}
pub(super) fn uint<T: TryFrom<u64>>(v: &Value) -> Result<T, Failure> {
    T::try_from(v.as_u64().ok_or(Failure::ProfileConfig)?).map_err(|_| Failure::ProfileConfig)
}
pub(super) fn text(v: &Value) -> Result<&str, Failure> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.contains('\0'))
        .ok_or(Failure::ProfileConfig)
}
pub(super) fn guid(v: &Value) -> Result<Guid, Failure> {
    let s = text(v)?;
    if s.len() != 32 || !s.is_ascii() {
        return Err(Failure::ProfileConfig);
    }
    let mut bytes = [0; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| Failure::ProfileConfig)?;
    }
    Ok(bytes)
}

pub(super) fn definition(v: &Value) -> Result<Definition, Failure> {
    Definition::from_json(v).map_err(|e| match e {
        nfs_content::vehicle_profiles::ParseError::Invalid => Failure::ProfileConfig,
        nfs_content::vehicle_profiles::ParseError::Limit => Failure::BodyLimit,
    })
}
