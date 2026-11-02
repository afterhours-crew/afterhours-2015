// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compile the supported participant-valid, timed AI handoff. Reject an
//! incomplete or different graph instead of silently reducing it to a timer.
use super::Rules;
use crate::{
    Error,
    graph::{Graph, Node, Site, Source, bad},
};
use nfs_frostbite::ebx::{Fields, Pointer, Value};
use std::collections::BTreeSet;

const STARTED: u32 = 216863469;
const IN: u32 = 5862146;
const OUT: u32 = 193453899;
const IN_A: u32 = 193450864;
const IN_B: u32 = 193450867;
const TRUE: u32 = 2861454674;
const STOPPED: u32 = 2861486076;
const COMPLETED: u32 = 2193212940;
const STOP: u32 = 2089401213;
const ENABLE: u32 = 0x93ef_b252;
const RELEASE: u32 = 1928776733;
const VALID: u32 = 2741198505;
const BOOL: u32 = 2088812747;
const PROVIDER: u32 = 3062566331;
const PARTICIPANTS: u32 = 304393993;
const AI_MANAGER: u32 = 3806204033;
const ACTOR_MANAGER: u32 = 673467093;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Edge(usize, u32, usize, u32, String);
fn field<'a>(fields: &'a Fields, key: &str) -> Result<&'a Value, Error> {
    fields.get(key).ok_or_else(|| bad("arrival field missing"))
}
fn flag(fields: &Fields, key: &str, expected: bool) -> Result<(), Error> {
    if field(fields, key)?.as_bool() != Some(expected) {
        return Err(bad("arrival Boolean contract"));
    }
    Ok(())
}
fn enumeration(fields: &Fields, key: &str, expected: &str) -> Result<(), Error> {
    if field(fields, key)?.as_enum_name() != Some(expected) {
        return Err(bad("arrival enum contract"));
    }
    Ok(())
}
fn unique<'a>(nodes: &'a [Node], class: &str) -> Result<&'a Node, Error> {
    let rows: Vec<_> = nodes.iter().filter(|n| n.object.class == class).collect();
    match rows.as_slice() {
        [node] => Ok(node),
        _ => Err(bad("arrival node missing or ambiguous")),
    }
}
fn edges(fields: &Fields, name: &str, events: bool) -> Result<Vec<Edge>, Error> {
    let rows = field(fields, name)?
        .as_array()
        .filter(|a| a.len() <= 64)
        .ok_or_else(|| bad("arrival connection bound"))?;
    rows.iter()
        .map(|row| {
            let f = row
                .as_fields()
                .ok_or_else(|| bad("arrival connection row"))?;
            let pointer = |key| match field(f, key)?.as_pointer() {
                Some(Pointer::Internal(i)) => Ok(i),
                _ => Err(bad("arrival connection must be local")),
            };
            let port = |key| {
                let v = field(f, key)?;
                let v = if events {
                    field(v.as_fields().ok_or_else(|| bad("arrival event"))?, "Id")?
                } else {
                    v
                };
                v.bits32().ok_or_else(|| bad("arrival port"))
            };
            let context = if events {
                field(f, "TargetType")?
                    .as_enum_name()
                    .ok_or_else(|| bad("arrival event realm"))?
                    .to_owned()
            } else if name == "PropertyConnections" {
                field(f, "Flags")?
                    .bits32()
                    .ok_or_else(|| bad("arrival property flags"))?
                    .to_string()
            } else {
                String::new()
            };
            Ok(Edge(
                pointer("Source")?,
                port(if events {
                    "SourceEvent"
                } else {
                    "SourceFieldId"
                })?,
                pointer("Target")?,
                port(if events {
                    "TargetEvent"
                } else {
                    "TargetFieldId"
                })?,
                context,
            ))
        })
        .collect()
}
fn exact(mut actual: Vec<Edge>, mut expected: Vec<Edge>) -> Result<(), Error> {
    actual.sort();
    expected.sort();
    if actual != expected {
        return Err(bad("arrival connection contract"));
    }
    Ok(())
}

pub(super) fn compile<S: Source>(
    graph: &mut Graph<S>,
    scene: &[Node],
    nodes: &[Node],
    rules: &Rules,
) -> Result<(u64, Site), Error> {
    let blueprint = unique(nodes, "ParticipantStateSequenceBlueprint")?;
    if nodes.len() != 11
        || nodes.iter().any(|n| {
            n.site.key.0 != blueprint.site.key.0
                || n.site.bus != blueprint.site.key
                || n.site.parents != blueprint.site.parents
        })
    {
        return Err(bad("arrival local composition"));
    }
    let root = unique(nodes, "ParticipantStateSequenceEntityData")?;
    let provider = unique(nodes, "StateSequenceParticipantProviderEntityData")?;
    let compare = unique(nodes, "CompareBoolEntityData")?;
    let delay = unique(nodes, "DelayEntityData")?;
    let control = unique(nodes, "EnableAiOnParticipantEntityData")?;
    let channel = unique(nodes, "LinkedSchematicChannelReferenceEntityData")?;
    let (buses, shape) = super::authored::shape(nodes, rules)?;
    if buses != 1
        || shape.iter().map(|(_, _, k)| *k).collect::<Vec<_>>()
            != nfs_world::sequences::Layout::ControlledArrival
                .profile()
                .kinds()
    {
        return Err(bad("arrival serializer contract"));
    }
    flag(&root.object.fields, "Enabled", true)?;
    enumeration(&provider.object.fields, "Realm", "Realm_ClientAndServer")?;
    enumeration(&compare.object.fields, "Realm", "Realm_Server")?;
    for (key, expected) in [
        ("Bool", false),
        ("TriggerOnPropertyChange", true),
        ("TriggerOnStart", true),
        ("AlwaysSend", false),
    ] {
        flag(&compare.object.fields, key, expected)?;
    }
    enumeration(&delay.object.fields, "Realm", "Realm_Server")?;
    enumeration(&delay.object.fields, "TimeDeltaType", "TimeDeltaType_World")?;
    for (key, expected) in [
        ("AutoStart", false),
        ("RunOnce", false),
        ("RemoveDuplicateEvents", true),
    ] {
        flag(&delay.object.fields, key, expected)?;
    }
    let duration = match field(&delay.object.fields, "Delay")? {
        Value::Float32(v) => f64::from(*v) * 1000.,
        _ => return Err(bad("arrival delay type")),
    };
    if !duration.is_finite() || !(1.0..=30000.0).contains(&duration) || duration.fract() != 0. {
        return Err(bad("arrival delay bound"));
    }
    enumeration(
        &control.object.fields,
        "PlayerInclusionMode",
        "PlayerInclusion_OnlyHuman",
    )?;
    flag(
        &control.object.fields,
        "AddParticipantToStateOnEnableAi",
        false,
    )?;
    flag(
        &control.object.fields,
        "RemoveParticipantFromStateOnDisableAi",
        false,
    )?;
    for key in ["OutputProperties", "OutputRefProperties"] {
        if field(&channel.object.fields, key)?
            .as_array()
            .is_none_or(|a| !a.is_empty())
        {
            return Err(bad("arrival channel outputs"));
        }
    }
    let events = edges(&blueprint.object.fields, "EventConnections", true)?;
    let successor =
        |source: &Node, port: u32, target_port: u32, class: &str| -> Result<&Node, Error> {
            let targets: Vec<_> = events
                .iter()
                .filter(|e| e.0 == source.site.key.1 && e.1 == port && e.3 == target_port)
                .filter_map(|e| {
                    nodes
                        .iter()
                        .find(|n| n.site.key.1 == e.2 && n.object.class == class)
                })
                .collect();
            match targets.as_slice() {
                [node] => Ok(*node),
                _ => Err(bad("arrival event successor")),
            }
        };
    let enter = successor(root, STARTED, IN_B, "EventAndGateEntityData")?;
    let leave = successor(delay, OUT, IN_B, "EventAndGateEntityData")?;
    let enable = successor(
        enter,
        OUT,
        IN,
        "ParticipantProviderToParticipantEventEntityData",
    )?;
    let release = successor(
        leave,
        OUT,
        IN,
        "ParticipantProviderToParticipantEventEntityData",
    )?;
    if enter.site == leave.site || enable.site == release.site {
        return Err(bad("arrival gates must be distinct"));
    }
    for node in [enter, leave] {
        enumeration(&node.object.fields, "Realm", "Realm_Server")?;
        if field(&node.object.fields, "EventCount")?.bits32() != Some(2) {
            return Err(bad("arrival gate arity"));
        }
    }
    let edge = |s: &Node, sp, t: &Node, tp, context: &str| {
        Edge(s.site.key.1, sp, t.site.key.1, tp, context.into())
    };
    let server = "EventConnectionTargetType_Server";
    exact(
        events,
        vec![
            edge(root, STARTED, delay, IN, server),
            edge(compare, TRUE, enter, IN_A, server),
            edge(compare, TRUE, leave, IN_A, server),
            edge(root, STARTED, enter, IN_B, server),
            edge(delay, OUT, leave, IN_B, server),
            edge(enter, OUT, enable, IN, server),
            edge(leave, OUT, release, IN, server),
            edge(enable, OUT, control, ENABLE, server),
            edge(release, OUT, control, RELEASE, server),
            edge(release, OUT, root, STOP, server),
            edge(
                root,
                STOPPED,
                root,
                COMPLETED,
                "EventConnectionTargetType_Client",
            ),
        ],
    )?;
    exact(
        edges(&blueprint.object.fields, "PropertyConnections", false)?,
        vec![edge(provider, VALID, compare, BOOL, "3")],
    )?;
    exact(
        edges(&blueprint.object.fields, "LinkConnections", false)?,
        vec![
            edge(root, PARTICIPANTS, provider, 0, ""),
            edge(root, rules.root_channels, channel, 0, ""),
            edge(control, AI_MANAGER, channel, AI_MANAGER, ""),
            edge(control, ACTOR_MANAGER, channel, ACTOR_MANAGER, ""),
            edge(enable, PROVIDER, provider, 0, ""),
            edge(release, PROVIDER, provider, 0, ""),
        ],
    )?;
    let channel_asset = graph.target_field(&channel.site, "ChannelAsset")?;
    let mut targets = BTreeSet::new();
    for class in [
        "CrashedParticipantTransitionEntityData",
        "ControllableOutOfBoundsTrackerEntityData",
    ] {
        let spawner = unique(scene, class)?;
        if graph.target_field(&spawner.site, "AiStateSequenceBlueprint")?
            != (rules.arrival.clone(), 0)
            || field(&spawner.object.fields, "DefaultStateSequenceBlueprint")?.as_pointer()
                != Some(Pointer::Null)
        {
            return Err(bad("arrival spawner blueprint"));
        }
        if class == "CrashedParticipantTransitionEntityData"
            && graph.target_field(&spawner.site, "CrashedSequenceBlueprint")?
                != (rules.crash.clone(), 0)
        {
            return Err(bad("arrival crash blueprint"));
        }
        let links = graph.links(&spawner.site, rules.arrival_channels)?;
        let [target] =
            <[_; 1]>::try_from(links).map_err(|_| bad("arrival spawner channel count"))?;
        if target.port != 0
            || graph.object(&target.site)?.class != "LinkedSchematicChannelEntityData"
            || graph.target_field(&target.site, "ChannelAsset")? != channel_asset
        {
            return Err(bad("arrival spawner channel mismatch"));
        }
        targets.insert(target.site);
    }
    if targets.len() != 1 {
        return Err(bad("arrival spawners must share their channel"));
    }
    let target = targets
        .pop_first()
        .ok_or_else(|| bad("arrival scene channel"))?;
    for (port, class) in [
        (ACTOR_MANAGER, "AiActorManagerEntityData"),
        (AI_MANAGER, "AiParticipantManagerEntityData"),
    ] {
        let [endpoint] = <[_; 1]>::try_from(graph.links(&target, port)?)
            .map_err(|_| bad("arrival manager link count"))?;
        if endpoint.port != 0
            || graph.object(&endpoint.site)?.class != class
            || !scene.iter().any(|n| n.site == endpoint.site)
        {
            return Err(bad("arrival manager link"));
        }
    }
    Ok((duration as u64, target))
}
