// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Failure, array, boolean, fields, uint};
use nfs_world::replication::sublevel::{Content, Kind, MAX_PROFILES, MAX_SERIALIZERS, Profile};
use serde_json::{Value, json};

const KINDS: &[(&str, Kind)] = &[
    ("noop", Kind::Noop),
    ("rpc", Kind::Rpc),
    ("level_reference", Kind::LevelReference),
    ("bool_property", Kind::BoolProperty),
    ("float_property", Kind::FloatProperty),
    ("i32_property", Kind::I32Property),
    ("rpc_references", Kind::RpcReferences),
    ("rpc_pairs", Kind::RpcPairs),
    ("rpc_tagged", Kind::RpcTagged),
    ("rpc_optional64", Kind::RpcOptional64),
    ("rpc_guid", Kind::RpcGuid),
    ("rpc_bool", Kind::RpcBool),
    ("rpc_map", Kind::RpcMap),
    ("rpc_four_references", Kind::RpcFourReferences),
    ("rpc_set64", Kind::RpcSet64),
    ("rpc_pursuit_maps", Kind::RpcPursuitMaps),
    ("rpc_spawn", Kind::RpcSpawn),
];

pub(super) fn parse(value: &Value) -> Result<Content, Failure> {
    let profiles = array(value, MAX_PROFILES)?
        .iter()
        .map(|value| {
            fields(value, &["content_key", "root", "serializers"])?;
            let kinds = array(&value["serializers"], MAX_SERIALIZERS)?
                .iter()
                .map(|v| {
                    let name = v.as_str().ok_or(Failure::ProfileConfig)?;
                    KINDS
                        .iter()
                        .find_map(|(n, k)| (*n == name).then_some(*k))
                        .ok_or(Failure::ProfileConfig)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((
                uint(&value["content_key"])?,
                Profile::new(boolean(&value["root"])?, &kinds)
                    .map_err(|_| Failure::ProfileConfig)?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Content::new(profiles).map_err(|_| Failure::ProfileConfig)
}

pub(super) fn json(content: &Content) -> Value {
    json!(content.profiles().map(|(key,profile)|json!({"content_key":key,"root":profile.is_root(),
        "serializers":profile.kinds().iter().map(|kind| KINDS.iter().find(|(_,k)| k==kind).expect("all enum kinds mapped").0).collect::<Vec<_>>() })).collect::<Vec<_>>())
}

pub(super) fn launchers(
    value: &Value,
    content: &Content,
) -> Result<nfs_world::launchers::Catalog, Failure> {
    let entries = array(value, MAX_PROFILES)?
        .iter()
        .map(|v| {
            fields(v, &["content_key", "serializers"])?;
            Ok((
                uint(&v["content_key"])?,
                array(&v["serializers"], MAX_SERIALIZERS)?
                    .iter()
                    .map(uint)
                    .collect::<Result<Vec<usize>, Failure>>()?,
            ))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    nfs_world::launchers::Catalog::new(entries, content).map_err(|_| Failure::ProfileConfig)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_store_rejects_unknown_shapes_and_instance_data() {
        let value = json!([{"content_key":7,"root":false,"serializers":["noop","rpc_bool"]}]);
        assert_eq!(json(&parse(&value).unwrap()), value);
        let mut altered = value.clone();
        altered[0]["values"] = json!([false]);
        assert!(parse(&altered).is_err());
        let mut altered = value.clone();
        altered[0]["serializers"][1] = json!("untraced");
        assert!(parse(&altered).is_err());
        let mut altered = value.clone();
        altered[0]["serializers"] = json!(vec!["noop"; MAX_SERIALIZERS + 1]);
        assert!(parse(&altered).is_err());
        assert!(parse(&json!([value[0], value[0]])).is_err());
    }
}
