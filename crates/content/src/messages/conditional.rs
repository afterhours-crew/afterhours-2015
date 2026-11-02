// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_frostbite::ebx::Object;
use std::{collections::BTreeSet, time::Duration};

pub(super) const ACTIVE: u32 = 1_010_522_579;
pub(super) const SEND: u32 = 1_037_321_618;
pub(super) const ATTACHMENT: u32 = (-1_087_517_622i32) as u32;
pub(super) const CONTEXT: u32 = (-1_232_400_965i32) as u32;
pub(super) const GROUP: u32 = (-420_521_610i32) as u32;
pub(super) const VALUE: u32 = 225_375_086;
pub(super) const IN: u32 = 5_862_146;
pub(super) const OUT: u32 = 193_453_899;
pub(super) const BOOL: u32 = 2_088_812_747;
pub(super) const TRUE: u32 = (-1_433_512_622i32) as u32;
pub(super) const SET_TRUE: u32 = (-1_541_066_415i32) as u32;
pub(super) const OPEN: u32 = 2_089_008_817;
pub(super) const CLOSE: u32 = 212_633_683;
pub(super) const BOOL_IN: u32 = (-1_615_583_540i32) as u32;
pub(super) const BOOL_OUT: u32 = (-1_774_651_195i32) as u32;
pub(super) const OR_INPUTS: [u32; 2] = [193_450_867, 193_450_864];
pub(super) fn check(ok: bool, reason: &str) -> Result<(), Error> {
    if ok { Ok(()) } else { Err(bad(reason)) }
}
pub(super) fn class<S: Source>(g: &mut Graph<S>, s: &Site, name: &str) -> Result<Object, Error> {
    let o = g.object(s)?;
    check(o.class == name, "conditional invitation node class")?;
    if name != "RaceObjectiveEntityData" {
        check(
            o.fields.get("Realm").and_then(Value::as_enum_name) == Some("Realm_Server"),
            "conditional invitation node realm",
        )?;
    }
    Ok(o)
}
pub(super) fn flag(o: &Object, key: &str, expected: bool) -> Result<(), Error> {
    check(
        o.fields.get(key).and_then(Value::as_bool) == Some(expected),
        "conditional invitation flag",
    )
}
pub(super) fn ports<S: Source>(g: &mut Graph<S>, s: &Site, expected: &[u32]) -> Result<(), Error> {
    check(
        g.input_ports(s)?.into_iter().collect::<BTreeSet<_>>()
            == expected.iter().copied().collect(),
        "conditional invitation property inputs",
    )
}
/// Validate every incoming event, including its delivery realm. No ignored
/// reset/open/alternate event may invalidate a graph reduction.
pub(super) fn events<S: Source>(g: &mut Graph<S>, s: &Site, expected: &[u32]) -> Result<(), Error> {
    let bus = g.source.object(&s.bus)?;
    let rows = bus
        .fields
        .get("EventConnections")
        .and_then(Value::as_array)
        .filter(|v| v.len() <= crate::scenes::MAX_GRAPH_OBJECTS)
        .ok_or_else(|| bad("conditional invitation event table bound"))?;
    let mut actual = BTreeSet::new();
    for row in rows {
        let f = row
            .as_fields()
            .ok_or_else(|| bad("conditional invitation event row"))?;
        let p = f
            .get("Target")
            .and_then(Value::as_pointer)
            .ok_or_else(|| bad("conditional invitation event target"))?;
        if g.source.target(&s.bus.0, p)?.as_ref() != Some(&s.key) {
            continue;
        }
        check(
            f.get("TargetType").and_then(Value::as_enum_name)
                == Some("EventConnectionTargetType_Server"),
            "conditional invitation event realm",
        )?;
        let port = f
            .get("TargetEvent")
            .and_then(Value::as_fields)
            .and_then(|f| f.get("Id"))
            .and_then(Value::bits32)
            .ok_or_else(|| bad("conditional invitation event port"))?;
        check(actual.insert(port), "conditional invitation event fan-in")?;
    }
    check(
        actual == expected.iter().copied().collect(),
        "conditional invitation event inputs",
    )
}
pub(super) fn event<S: Source>(
    g: &mut Graph<S>,
    s: &Site,
    input: u32,
    output: u32,
) -> Result<Site, Error> {
    let sources = g.event_sources(s, input)?;
    match sources.as_slice() {
        [(source, port)] if *port == output => Ok(source.clone()),
        _ => Err(bad("conditional invitation event source")),
    }
}
pub(super) fn property<S: Source>(
    g: &mut Graph<S>,
    s: &Site,
    input: u32,
    output: u32,
) -> Result<Site, Error> {
    let e = g.input(s, input)?;
    check(e.port == output, "conditional invitation property output")?;
    Ok(e.site)
}
pub(super) fn activation<S: Source>(g: &mut Graph<S>, compare: &Site) -> Result<Site, Error> {
    let o = class(g, compare, "CompareBoolEntityData")?;
    for (name, value) in [
        ("Bool", false),
        ("TriggerOnPropertyChange", true),
        ("TriggerOnStart", false),
        ("AlwaysSend", false),
    ] {
        flag(&o, name, value)?;
    }
    ports(g, compare, &[BOOL])?;
    events(g, compare, &[])?;
    property(g, compare, BOOL, ACTIVE)
}
pub(super) fn gate<S: Source>(g: &mut Graph<S>, site: &Site, input: u32) -> Result<(), Error> {
    let o = class(g, site, "PropertyGateEntityData")?;
    flag(&o, "Default", false)?;
    flag(&o, "WritePropertyOnOpenGate", true)?;
    ports(g, site, &[input])?;
    events(g, site, &[OPEN])
}
pub(super) fn or<S: Source>(g: &mut Graph<S>, site: &Site) -> Result<[Endpoint; 2], Error> {
    let o = class(g, site, "OrEntityData")?;
    check(
        o.fields.get("InputCount").and_then(Value::bits32) == Some(2),
        "conditional invitation OR count",
    )?;
    ports(g, site, &OR_INPUTS)?;
    events(g, site, &[])?;
    Ok([g.input(site, OR_INPUTS[0])?, g.input(site, OR_INPUTS[1])?])
}
pub(super) fn delayed<S: Source>(
    g: &mut Graph<S>,
    endpoint: &Endpoint,
    objective: &Site,
) -> Result<Duration, Error> {
    check(
        endpoint.port == BOOL_OUT,
        "conditional invitation delayed output",
    )?;
    gate(g, &endpoint.site, BOOL_IN)?;
    check(
        property(g, &endpoint.site, BOOL_IN, ACTIVE)? == *objective,
        "conditional invitation delayed objective",
    )?;
    let delay = event(g, &endpoint.site, OPEN, OUT)?;
    let o = class(g, &delay, "DelayEntityData")?;
    for (name, value) in [
        ("AutoStart", false),
        ("RunOnce", false),
        ("RemoveDuplicateEvents", true),
    ] {
        flag(&o, name, value)?;
    }
    check(
        o.fields.get("Realm").and_then(Value::as_enum_name) == Some("Realm_Server")
            && o.fields.get("TimeDeltaType").and_then(Value::as_enum_name)
                == Some("TimeDeltaType_World"),
        "conditional invitation delay clock",
    )?;
    ports(g, &delay, &[])?;
    events(g, &delay, &[IN])?;
    let compare = event(g, &delay, IN, TRUE)?;
    check(
        activation(g, &compare)? == *objective,
        "conditional invitation delay activation",
    )?;
    match o.fields.get("Delay") {
        Some(Value::Float32(v)) if v.is_finite() && *v > 0. && *v <= 60. => {
            Ok(Duration::from_secs_f32(*v))
        }
        _ => Err(bad("conditional invitation delay bound")),
    }
}

pub(super) struct Group {
    pub senders: Vec<Site>,
    pub objective: Site,
    pub contexts: Vec<Endpoint>,
    pub manager: usize,
    pub hashes: [u32; 3],
}
pub(super) fn group<S: Source>(
    g: &mut Graph<S>,
    nodes: &[Node],
    world: &[Node],
    bindings: &BTreeMap<Key, usize>,
    selected: [&str; 3],
) -> Result<Group, Error> {
    let mut senders = Vec::new();
    let mut hashes = Vec::new();
    for name in selected {
        let mut found = Vec::new();
        for n in nodes
            .iter()
            .filter(|n| n.object.class == "UniqueMessageSenderEntityData")
        {
            let asset = g.target_field(&n.site, "MessageAsset")?;
            if asset.0 != name {
                continue;
            }
            flag(&n.object, "HasMessageRestrictor", true)?;
            flag(&n.object, "SendMessage", false)?;
            ports(g, &n.site, &[SEND])?;
            events(g, &n.site, &[])?;
            let m = g.source.object(&asset)?;
            check(
                m.class == "MessageAsset",
                "conditional invitation message asset",
            )?;
            flag(&m, "Persisted", true)?;
            hashes.push(bits32(
                m.fields
                    .get("HashId")
                    .ok_or_else(|| bad("conditional invitation hash"))?,
            )?);
            found.push(n.site.clone());
        }
        check(
            found.len() == 1,
            "conditional invitation sender missing or ambiguous",
        )?;
        senders.push(found.remove(0));
    }
    let objectives = senders
        .iter()
        .map(|s| g.links(s, ATTACHMENT))
        .collect::<Result<Vec<_>, _>>()?;
    let [objective] = objectives[0].as_slice() else {
        return Err(bad("conditional invitation objective attachment"));
    };
    check(
        objective.port == 0 && objectives.iter().all(|v| v == &objectives[0]),
        "conditional invitation shared objective",
    )?;
    class(g, &objective.site, "RaceObjectiveEntityData")?;
    let contexts = senders
        .iter()
        .map(|s| g.links(s, CONTEXT))
        .collect::<Result<Vec<_>, _>>()?;
    check(
        contexts[0].len() == 1 && contexts.iter().all(|c| c == &contexts[0]),
        "conditional invitation shared participant",
    )?;
    let managers = senders
        .iter()
        .map(|s| sender_manager(g, s, world, bindings))
        .collect::<Result<Vec<_>, _>>()?;
    check(
        managers.iter().all(|m| *m == managers[0]),
        "conditional invitation shared manager",
    )?;
    let mut groups = 0;
    let expected: BTreeSet<_> = senders
        .iter()
        .cloned()
        .map(|site| Endpoint { site, port: 0 })
        .collect();
    for n in nodes
        .iter()
        .filter(|n| n.object.class == "MessageRestrictorEntityData")
    {
        let links = g.links(&n.site, GROUP)?;
        if !links.iter().any(|e| expected.contains(e)) {
            continue;
        }
        check(
            links.len() == 3 && links.into_iter().collect::<BTreeSet<_>>() == expected,
            "conditional invitation restrictor membership",
        )?;
        ports(g, &n.site, &[])?;
        events(g, &n.site, &[])?;
        groups += 1;
    }
    check(
        groups == 1,
        "conditional invitation restrictor missing or ambiguous",
    )?;

    Ok(Group {
        senders,
        objective: objective.site.clone(),
        contexts: contexts[0].clone(),
        manager: managers[0],
        hashes: hashes
            .try_into()
            .map_err(|_| bad("conditional invitation count"))?,
    })
}
