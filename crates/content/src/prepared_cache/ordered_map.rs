// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! JSON representation for static maps with structured identity keys.
//! Preserve all key fields and reject duplicates instead of silently replacing.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn serialize<K: Serialize, V: Serialize, S: serde::Serializer>(
    value: &BTreeMap<K, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.iter().collect::<Vec<_>>().serialize(serializer)
}

pub fn deserialize<
    'de,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
    D: serde::Deserializer<'de>,
>(
    deserializer: D,
) -> Result<BTreeMap<K, V>, D::Error> {
    let pairs = Vec::<(K, V)>::deserialize(deserializer)?;
    let mut map = BTreeMap::new();
    for (key, value) in pairs {
        if map.insert(key, value).is_some() {
            return Err(serde::de::Error::custom("duplicate prepared map identity"));
        }
    }
    Ok(map)
}
