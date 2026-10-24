// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    Error,
    graph::{Node, Site, bad},
    sequences::Rules,
};
use nfs_frostbite::ebx::Value;
use nfs_world::replication::entity::{Kind, Profile};

/// Native serializer order, independently of the service which spawns it.
pub(crate) type Field = (Site, String, Kind);
pub(crate) fn shape(nodes: &[Node], rules: &Rules) -> Result<(u16, Vec<Field>), Error> {
    if nodes
        .first()
        .is_none_or(|n| n.object.class != "ParticipantStateSequenceBlueprint")
    {
        return Err(bad("sequence blueprint class"));
    }
    let mut buses = 0u16;
    let mut fields = Vec::new();
    for node in nodes {
        let role = rules
            .classes
            .get(&node.object.class)
            .ok_or_else(|| bad(&format!("unknown sequence class {}", node.object.class)))?;
        if node.object.class.ends_with("Blueprint") {
            let flags = match node.object.fields.get("Flags") {
                Some(Value::UInt16(v)) => u32::from(*v),
                Some(v) => v.bits32().ok_or_else(|| bad("sequence bus flags"))?,
                None => return Err(bad("sequence bus flags")),
            };
            buses = buses
                .checked_add(u16::from(flags & 1 != 0))
                .ok_or_else(|| bad("sequence bus bound"))?;
        }
        let kind = match role.as_deref() {
            None => continue,
            Some("root") => Kind::SequenceRoot,
            Some("provider") => Kind::RpcOptionalReference,
            Some("event") => Kind::Rpc,
            Some("channel") => Kind::RpcReference,
            Some("participants") => Kind::RpcReferences,
            Some(_) => return Err(bad("sequence role")),
        };
        fields.push((node.site.clone(), node.object.class.clone(), kind));
    }
    if buses == 0
        || fields
            .first()
            .is_none_or(|(_, _, kind)| *kind != Kind::SequenceRoot)
    {
        return Err(bad("sequence root or bus count"));
    }
    Profile::new(&fields.iter().map(|(_, _, k)| *k).collect::<Vec<_>>())
        .map_err(|_| bad("sequence serializer profile"))?;
    Ok((buses, fields))
}
