// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_frostbite::ebx::Object;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

const ACTIVE: u32 = 1_010_522_579;
const SEND: u32 = 1_037_321_618;
const ATTACHMENT: u32 = (-1_087_517_622i32) as u32;
const CONTEXT: u32 = (-1_232_400_965i32) as u32;
const GROUP: u32 = (-420_521_610i32) as u32;
const VALUE: u32 = 225_375_086;
const IN: u32 = 5_862_146;
const OUT: u32 = 193_453_899;
const BOOL: u32 = 2_088_812_747;
const TRUE: u32 = (-1_433_512_622i32) as u32;
const SET_TRUE: u32 = (-1_541_066_415i32) as u32;
const OPEN: u32 = 2_089_008_817;
const CLOSE: u32 = 212_633_683;
const LESS: u32 = 193_445_146;
const GE: u32 = 2_088_727_237;
const A: u32 = 177_636;
const B: u32 = 177_639;
const INT_IN: u32 = 214_528_497;
const INT_OUT: u32 = (-1_510_504_872i32) as u32;
const BOOL_IN: u32 = (-1_615_583_540i32) as u32;
const BOOL_OUT: u32 = (-1_774_651_195i32) as u32;
const INPUTS: [u32; 2] = [193_450_866, 193_450_867];
const OR_INPUTS: [u32; 2] = [193_450_867, 193_450_864];
const CURRENT: u32 = nfs_world::scoring::stats::MAX_HP;
const STOCK: u32 = nfs_world::scoring::stats::STOCK_MAX_HP;

fn check(ok: bool, reason: &str) -> Result<(), Error> {
    if ok { Ok(()) } else { Err(bad(reason)) }
}
fn class<S: Source>(g: &mut Graph<S>, s: &Site, name: &str) -> Result<Object, Error> {
    let o = g.object(s)?;
    check(o.class == name, "power invitation node class")?;
    if name != "RaceObjectiveEntityData" {
        check(
            o.fields.get("Realm").and_then(Value::as_enum_name) == Some("Realm_Server"),
            "power invitation node realm",
        )?;
    }
    Ok(o)
}
fn flag(o: &Object, key: &str, expected: bool) -> Result<(), Error> {
    check(
        o.fields.get(key).and_then(Value::as_bool) == Some(expected),
        "power invitation flag",
    )
}
fn ports<S: Source>(g: &mut Graph<S>, s: &Site, expected: &[u32]) -> Result<(), Error> {
    check(
        g.input_ports(s)?.into_iter().collect::<BTreeSet<_>>()
            == expected.iter().copied().collect(),
        "power invitation property inputs",
    )
}
/// Validate every incoming event, including its delivery realm. No ignored
/// reset/open/alternate event may invalidate a graph reduction.
fn events<S: Source>(g: &mut Graph<S>, s: &Site, expected: &[u32]) -> Result<(), Error> {
    let bus = g.source.object(&s.bus)?;
    let rows = bus
        .fields
        .get("EventConnections")
        .and_then(Value::as_array)
        .filter(|v| v.len() <= crate::scenes::MAX_GRAPH_OBJECTS)
        .ok_or_else(|| bad("power invitation event table bound"))?;
    let mut actual = BTreeSet::new();
    for row in rows {
        let f = row
            .as_fields()
            .ok_or_else(|| bad("power invitation event row"))?;
        let p = f
            .get("Target")
            .and_then(Value::as_pointer)
            .ok_or_else(|| bad("power invitation event target"))?;
        if g.source.target(&s.bus.0, p)?.as_ref() != Some(&s.key) {
            continue;
        }
        check(
            f.get("TargetType").and_then(Value::as_enum_name)
                == Some("EventConnectionTargetType_Server"),
            "power invitation event realm",
        )?;
        let port = f
            .get("TargetEvent")
            .and_then(Value::as_fields)
            .and_then(|f| f.get("Id"))
            .and_then(Value::bits32)
            .ok_or_else(|| bad("power invitation event port"))?;
        check(actual.insert(port), "power invitation event fan-in")?;
    }
    check(
        actual == expected.iter().copied().collect(),
        "power invitation event inputs",
    )
}
fn event<S: Source>(g: &mut Graph<S>, s: &Site, input: u32, output: u32) -> Result<Site, Error> {
    let sources = g.event_sources(s, input)?;
    match sources.as_slice() {
        [(source, port)] if *port == output => Ok(source.clone()),
        _ => Err(bad("power invitation event source")),
    }
}
fn property<S: Source>(g: &mut Graph<S>, s: &Site, input: u32, output: u32) -> Result<Site, Error> {
    let e = g.input(s, input)?;
    check(e.port == output, "power invitation property output")?;
    Ok(e.site)
}
fn activation<S: Source>(g: &mut Graph<S>, compare: &Site) -> Result<Site, Error> {
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
fn gate<S: Source>(g: &mut Graph<S>, site: &Site, input: u32) -> Result<(), Error> {
    let o = class(g, site, "PropertyGateEntityData")?;
    flag(&o, "Default", false)?;
    flag(&o, "WritePropertyOnOpenGate", true)?;
    ports(g, site, &[input])?;
    events(g, site, &[OPEN])
}
fn or<S: Source>(g: &mut Graph<S>, site: &Site) -> Result<[Endpoint; 2], Error> {
    let o = class(g, site, "OrEntityData")?;
    check(
        o.fields.get("InputCount").and_then(Value::bits32) == Some(2),
        "power invitation OR count",
    )?;
    ports(g, site, &OR_INPUTS)?;
    events(g, site, &[])?;
    Ok([g.input(site, OR_INPUTS[0])?, g.input(site, OR_INPUTS[1])?])
}
fn latch<S: Source>(g: &mut Graph<S>, value: &Site) -> Result<Site, Error> {
    let o = class(g, value, "BoolEntityData")?;
    flag(&o, "DefaultValue", false)?;
    ports(g, value, &[])?;
    events(g, value, &[SET_TRUE])?;
    let gate = event(g, value, SET_TRUE, OUT)?;
    let o = class(g, &gate, "EventGateEntityData")?;
    flag(&o, "Default", true)?;
    ports(g, &gate, &[])?;
    events(g, &gate, &[IN, CLOSE])?;
    Ok(gate)
}
fn threshold<S: Source>(
    g: &mut Graph<S>,
    compare: &Site,
    triggered: bool,
) -> Result<(Site, Site, u32), Error> {
    let o = class(g, compare, "CompareIntEntityData")?;
    flag(&o, "TriggerOnPropertyChange", triggered)?;
    flag(&o, "TriggerOnStart", false)?;
    ports(g, compare, &[A, B])?;
    events(g, compare, if triggered { &[] } else { &[IN] })?;
    let latched = property(g, compare, A, INT_OUT)?;
    let math = property(g, compare, B, OUT)?;
    let o = class(g, &math, "MathIntOpEntityData")?;
    let operators = o
        .fields
        .get("Operators")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("power invitation operators"))?;
    check(
        operators.len() == 1 && operators[0].as_enum_name() == Some("IntMathOp_Add"),
        "power invitation addition",
    )?;
    ports(g, &math, &INPUTS)?;
    events(g, &math, &[])?;
    let stock = property(g, &math, INPUTS[0], STOCK)?;
    let number = property(g, &math, INPUTS[1], VALUE)?;
    let o = class(g, &number, "IntEntityData")?;
    ports(g, &number, &[])?;
    events(g, &number, &[])?;
    let n = o
        .fields
        .get("DefaultValue")
        .and_then(Value::bits32)
        .ok_or_else(|| bad("power invitation increment"))?;
    Ok((latched, stock, n))
}
fn delayed<S: Source>(
    g: &mut Graph<S>,
    endpoint: &Endpoint,
    objective: &Site,
) -> Result<Duration, Error> {
    check(endpoint.port == BOOL_OUT, "power invitation delayed output")?;
    gate(g, &endpoint.site, BOOL_IN)?;
    check(
        property(g, &endpoint.site, BOOL_IN, ACTIVE)? == *objective,
        "power invitation delayed objective",
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
        "power invitation delay clock",
    )?;
    ports(g, &delay, &[])?;
    events(g, &delay, &[IN])?;
    let compare = event(g, &delay, IN, TRUE)?;
    check(
        activation(g, &compare)? == *objective,
        "power invitation delay activation",
    )?;
    match o.fields.get("Delay") {
        Some(Value::Float32(v)) if v.is_finite() && *v > 0. && *v <= 60. => {
            Ok(Duration::from_secs_f32(*v))
        }
        _ => Err(bad("power invitation delay bound")),
    }
}

pub(super) fn read<S: Source>(
    g: &mut Graph<S>,
    nodes: &[Node],
    world: &[Node],
    bindings: &BTreeMap<Key, usize>,
    stable: u32,
    selected: [&str; 3],
) -> Result<Invitation, Error> {
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
            check(m.class == "MessageAsset", "power invitation message asset")?;
            flag(&m, "Persisted", true)?;
            hashes.push(bits32(
                m.fields
                    .get("HashId")
                    .ok_or_else(|| bad("power invitation hash"))?,
            )?);
            found.push(n.site.clone());
        }
        check(
            found.len() == 1,
            "power invitation sender missing or ambiguous",
        )?;
        senders.push(found.remove(0));
    }
    let objectives = senders
        .iter()
        .map(|s| g.links(s, ATTACHMENT))
        .collect::<Result<Vec<_>, _>>()?;
    let [objective] = objectives[0].as_slice() else {
        return Err(bad("power invitation objective attachment"));
    };
    check(
        objective.port == 0 && objectives.iter().all(|v| v == &objectives[0]),
        "power invitation shared objective",
    )?;
    class(g, &objective.site, "RaceObjectiveEntityData")?;
    let contexts = senders
        .iter()
        .map(|s| g.links(s, CONTEXT))
        .collect::<Result<Vec<_>, _>>()?;
    check(
        contexts[0].len() == 1 && contexts.iter().all(|c| c == &contexts[0]),
        "power invitation shared participant",
    )?;
    let managers = senders
        .iter()
        .map(|s| sender_manager(g, s, world, bindings))
        .collect::<Result<Vec<_>, _>>()?;
    check(
        managers.iter().all(|m| *m == managers[0]),
        "power invitation shared manager",
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
            "power invitation restrictor membership",
        )?;
        ports(g, &n.site, &[])?;
        events(g, &n.site, &[])?;
        groups += 1;
    }
    check(
        groups == 1,
        "power invitation restrictor missing or ambiguous",
    )?;
    let basic_or = property(g, &senders[0], SEND, OUT)?;
    let basic_inputs = or(g, &basic_or)?;
    let (basic, timeout) = match (&basic_inputs[0], &basic_inputs[1]) {
        (a, b) if a.port == VALUE && b.port == BOOL_OUT => (a, b),
        (a, b) if b.port == VALUE && a.port == BOOL_OUT => (b, a),
        _ => return Err(bad("power invitation default alternatives")),
    };
    let delay = delayed(g, timeout, &objective.site)?;
    let values = [
        basic.site.clone(),
        property(g, &senders[1], SEND, VALUE)?,
        property(g, &senders[2], SEND, VALUE)?,
    ];
    check(
        values.iter().collect::<BTreeSet<_>>().len() == 3,
        "power invitation distinct latches",
    )?;
    let gates = values
        .iter()
        .map(|s| latch(g, s))
        .collect::<Result<Vec<_>, _>>()?;
    let first = event(g, &gates[0], IN, LESS)?;
    let second = event(g, &gates[1], IN, LESS)?;
    check(
        event(g, &gates[2], IN, GE)? == second && event(g, &second, IN, GE)? == first,
        "power invitation comparison tree",
    )?;
    let (latched, stock, low) = threshold(g, &first, true)?;
    let (other, other_stock, high) = threshold(g, &second, false)?;
    check(
        latched == other && stock == other_stock,
        "power invitation shared vehicle figures",
    )?;
    gate(g, &latched, INT_IN)?;
    check(
        property(g, &latched, INT_IN, CURRENT)? == stock,
        "power invitation current versus stock provider",
    )?;
    class(g, &stock, "VehicleStatsProviderEntityData")?;
    ports(g, &stock, &[])?;
    events(g, &stock, &[])?;
    check(
        g.links(&stock, CONTEXT)? == contexts[0],
        "power invitation vehicle participant",
    )?;
    let compare = event(g, &latched, OPEN, TRUE)?;
    check(
        activation(g, &compare)? == objective.site,
        "power invitation activation objective",
    )?;
    for (i, gate) in gates.iter().enumerate() {
        let compare = event(g, gate, CLOSE, TRUE)?;
        let o = class(g, &compare, "CompareBoolEntityData")?;
        for (name, value) in [
            ("Bool", false),
            ("TriggerOnPropertyChange", true),
            ("TriggerOnStart", false),
            ("AlwaysSend", false),
        ] {
            flag(&o, name, value)?;
        }
        ports(g, &compare, &[BOOL])?;
        events(g, &compare, &[])?;
        let alternatives = property(g, &compare, BOOL, OUT)?;
        let alternatives = or(g, &alternatives)?;
        let expected: BTreeSet<_> = values
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, s)| Endpoint {
                site: s.clone(),
                port: VALUE,
            })
            .collect();
        check(
            alternatives.into_iter().collect::<BTreeSet<_>>() == expected,
            "power invitation exclusive latch closure",
        )?;
    }
    Invitation::new(
        crate::chapter_notifications::stable(g, stable, &objective.site)?,
        managers[0],
        hashes
            .try_into()
            .map_err(|_| bad("power invitation message count"))?,
        [low, high],
        delay,
    )
}
