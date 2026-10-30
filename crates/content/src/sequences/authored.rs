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
    if nodes.first().is_none_or(|n| {
        !matches!(
            n.object.class.as_str(),
            "ParticipantStateSequenceBlueprint" | "VehicleStateSequenceBlueprint"
        )
    }) {
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
            Some("vehicle_root") => Kind::VehicleSequenceRoot,
            Some("provider") => Kind::RpcOptionalReference,
            Some("event") => Kind::Rpc,
            Some("channel") => Kind::RpcReference,
            Some("participants") => Kind::RpcReferences,
            Some(_) => return Err(bad("sequence role")),
        };
        fields.push((node.site.clone(), node.object.class.clone(), kind));
    }
    if buses == 0
        || fields.first().is_none_or(|(_, _, kind)| {
            !matches!(kind, Kind::SequenceRoot | Kind::VehicleSequenceRoot)
        })
    {
        return Err(bad("sequence root or bus count"));
    }
    Profile::new(&fields.iter().map(|(_, _, k)| *k).collect::<Vec<_>>())
        .map_err(|_| bad("sequence serializer profile"))?;
    Ok((buses, fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        Graph,
        tests::{Memory, local},
    };

    #[test]
    fn participant_and_vehicle_sequences_keep_distinct_updates_and_unknown_roots_fail() {
        let rules = crate::builds::profiles().unwrap().remove(0).sequences;
        for (blueprint, root) in [
            (
                "ParticipantStateSequenceBlueprint",
                "ParticipantStateSequenceEntityData",
            ),
            (
                "VehicleStateSequenceBlueprint",
                "VehicleStateSequenceEntityData",
            ),
        ] {
            let mut source = Memory::default();
            source.put(
                "a",
                0,
                blueprint,
                &[("Flags", Value::UInt16(1)), ("Object", local(1))],
            );
            source.put("a", 1, root, &[]);
            let mut nodes = Graph::new(&mut source).read(Site::root("a")).unwrap();
            let (buses, fields) = shape(&nodes, &rules).unwrap();
            assert_eq!(buses, 1);
            assert_eq!(fields.len(), 1);
            assert_eq!(
                fields[0].2,
                if root == "VehicleStateSequenceEntityData" {
                    Kind::VehicleSequenceRoot
                } else {
                    Kind::SequenceRoot
                }
            );
            for class in [
                "StringEntityData",
                "TelemetryHookEntityData",
                "EventAndGateEntityData",
                "LocalPlayerVehicleInterfaceEntityData",
                "PropertyDebugEntityData",
                "SchematicChannelEntityData",
            ] {
                let mut node = nodes[1].clone();
                node.object.class = class.into();
                nodes.push(node);
            }
            assert_eq!(shape(&nodes, &rules).unwrap(), (buses, fields));
            let mut unknown = nodes[1].clone();
            unknown.object.class = "UnclassifiedEntityData".into();
            nodes.push(unknown);
            assert!(shape(&nodes, &rules).is_err());
            nodes.pop();
            nodes[0].object.class = "OtherBlueprint".into();
            assert!(shape(&nodes, &rules).is_err());
        }
    }
}
