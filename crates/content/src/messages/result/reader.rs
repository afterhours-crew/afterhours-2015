// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::super::conditional::*;
use super::*;
use crate::events::recommendations::{NarrativeEventMap, metadata_key};
use nfs_frostbite::ebx::Pointer;

const FALSE: u32 = (-81_881_639i32) as u32;
const REQUEST: u32 = 274_252_513;
const COMPLETE: u32 = 1_123_355_969;
const RECOMMENDED: u32 = (-158_930_774i32) as u32;
const DETAILS: u32 = 109_263_670;
const MATCHED: u32 = 519_031_723;
const FILTERS: u32 = 1_108_556_351;
const OBJECTIVE_COMPLETE: u32 = 448_895_852;
const REPORTED: u32 = 89_858_563;
const POSITION: &str = "gameplay/scoring/scoretags/positionscoretag";

/// Unlike the power fork, this graph has deliberate event fan-in. Preserve
/// every source and realm; a duplicate connection is not another valid input.
fn incoming<S: Source>(g: &mut Graph<S>, s: &Site) -> Result<Vec<(u32, Site, u32)>, Error> {
    let bus = g.source.object(&s.bus)?;
    let rows = bus
        .fields
        .get("EventConnections")
        .and_then(Value::as_array)
        .filter(|r| r.len() <= crate::scenes::MAX_GRAPH_OBJECTS)
        .ok_or_else(|| bad("result invitation event table"))?;
    let mut out = Vec::new();
    for row in rows {
        let f = row
            .as_fields()
            .ok_or_else(|| bad("result invitation event row"))?;
        let target = f
            .get("Target")
            .and_then(Value::as_pointer)
            .ok_or_else(|| bad("result invitation target"))?;
        if g.source.target(&s.bus.0, target)?.as_ref() != Some(&s.key) {
            continue;
        }
        check(
            f.get("TargetType").and_then(Value::as_enum_name)
                == Some("EventConnectionTargetType_Server"),
            "result invitation event realm",
        )?;
        let port = |field| {
            f.get(field)
                .and_then(Value::as_fields)
                .and_then(|f| f.get("Id"))
                .and_then(Value::bits32)
                .ok_or_else(|| bad("result invitation event id"))
        };
        let key = g
            .source
            .target(
                &s.bus.0,
                f.get("Source")
                    .and_then(Value::as_pointer)
                    .ok_or_else(|| bad("result invitation source"))?,
            )?
            .ok_or_else(|| bad("result invitation null source"))?;
        out.push((
            port("TargetEvent")?,
            Site { key, ..s.clone() },
            port("SourceEvent")?,
        ));
    }
    Ok(out)
}
fn wiring<S: Source>(
    g: &mut Graph<S>,
    s: &Site,
    expected: &[(u32, Site, u32)],
) -> Result<(), Error> {
    let actual = incoming(g, s)?;
    check(
        actual.len() == expected.len()
            && actual.into_iter().collect::<BTreeSet<_>>() == expected.iter().cloned().collect(),
        "result invitation event wiring",
    )
}
fn boolean<S: Source>(g: &mut Graph<S>, s: &Site) -> Result<(), Error> {
    flag(&class(g, s, "BoolEntityData")?, "DefaultValue", false)?;
    ports(g, s, &[])
}
fn compare<S: Source>(g: &mut Graph<S>, s: &Site, triggered: bool) -> Result<(), Error> {
    let o = class(g, s, "CompareBoolEntityData")?;
    for (name, value) in [
        ("Bool", false),
        ("TriggerOnPropertyChange", triggered),
        ("TriggerOnStart", false),
        ("AlwaysSend", false),
    ] {
        flag(&o, name, value)?;
    }
    ports(g, s, &[BOOL])
}
fn result_latch<S: Source>(g: &mut Graph<S>, s: &Site, equal: bool) -> Result<(Site, Site), Error> {
    boolean(g, s)?;
    events(g, s, &[SET_TRUE])?;
    let and = event(g, s, SET_TRUE, OUT)?;
    let o = class(g, &and, "EventAndGateEntityData")?;
    check(
        o.fields.get("EventCount").and_then(Value::bits32) == Some(2),
        "result invitation AND count",
    )?;
    ports(g, &and, &[])?;
    events(g, &and, &OR_INPUTS)?;
    let cmp = event(g, &and, OR_INPUTS[1], TRUE)?;
    compare(g, &cmp, true)?;
    events(g, &cmp, &[])?;
    let nonrecommended = property(g, &cmp, BOOL, VALUE)?;
    let filter = event(g, &and, OR_INPUTS[0], MATCHED)?;
    let o = g.object(&filter)?;
    check(
        o.class == "ScoreResultFilterEntityData"
            && o.fields
                .get("CompareOperation")
                .and_then(Value::as_enum_name)
                == Some(if equal {
                    "CompareOp_Equal"
                } else {
                    "CompareOp_NotEqual"
                })
            && o.fields.get("Score").and_then(Value::bits32) == Some(1),
        "result invitation position filter",
    )?;
    ports(g, &filter, &[])?;
    events(g, &filter, &[])?;
    let tag = g.target_field(&filter, "ScoreId")?;
    let tag = g.source.object(&tag)?;
    check(
        tag.class == "ScoreTag"
            && tag
                .fields
                .get("Name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(POSITION)),
        "result invitation position tag",
    )?;
    Ok((nonrecommended, filter))
}

fn recommendation<S: Source>(
    g: &mut Graph<S>,
    positive: &Site,
    negative: &Site,
    objective: &Site,
    contexts: &[Endpoint],
    map: &NarrativeEventMap,
) -> Result<(u32, Duration), Error> {
    check(
        positive != negative,
        "result invitation distinct recommendation latches",
    )?;
    boolean(g, positive)?;
    boolean(g, negative)?;
    events(g, positive, &[SET_TRUE])?;
    let cmp = event(g, positive, SET_TRUE, TRUE)?;
    compare(g, &cmp, false)?;
    events(g, &cmp, &[IN])?;
    let checker = event(g, &cmp, IN, COMPLETE)?;
    check(
        property(g, &cmp, BOOL, RECOMMENDED)? == checker,
        "result invitation checker output",
    )?;
    check(
        g.object(&checker)?.class == "ServerIsEventRecommendedCheckerEntityData",
        "result invitation checker class",
    )?;
    ports(g, &checker, &[])?;
    check(
        g.links(&checker, CONTEXT)? == contexts,
        "result invitation checker participant",
    )?;
    let sources = incoming(g, negative)?;
    check(
        sources.len() == 2,
        "result invitation negative latch inputs",
    )?;
    let timeout = sources
        .iter()
        .find(|(input, source, output)| *input == SET_TRUE && *output == OUT && *source != cmp)
        .map(|(_, s, _)| s.clone())
        .ok_or_else(|| bad("result invitation timeout gate"))?;
    wiring(
        g,
        negative,
        &[
            (SET_TRUE, cmp.clone(), FALSE),
            (SET_TRUE, timeout.clone(), OUT),
        ],
    )?;
    flag(&class(g, &timeout, "EventGateEntityData")?, "Default", true)?;
    ports(g, &timeout, &[])?;
    let delay = event(g, &timeout, IN, OUT)?;
    wiring(
        g,
        &timeout,
        &[
            (IN, delay.clone(), OUT),
            (CLOSE, cmp.clone(), TRUE),
            (CLOSE, cmp, FALSE),
        ],
    )?;
    let o = class(g, &delay, "DelayEntityData")?;
    for (n, v) in [
        ("AutoStart", false),
        ("RunOnce", false),
        ("RemoveDuplicateEvents", true),
    ] {
        flag(&o, n, v)?;
    }
    check(
        o.fields.get("TimeDeltaType").and_then(Value::as_enum_name) == Some("TimeDeltaType_World"),
        "result invitation clock",
    )?;
    let duration = match o.fields.get("Delay") {
        Some(Value::Float32(v)) if v.is_finite() && *v > 0. && *v <= 60. => {
            Duration::from_secs_f32(*v)
        }
        _ => return Err(bad("result invitation delay bound")),
    };
    ports(g, &delay, &[])?;
    let mut request_sources = Vec::new();
    let mut activations = 0;
    for (input, source, output) in incoming(g, &checker)? {
        check(input == REQUEST, "result invitation checker input")?;
        match g.object(&source)?.class.as_str() {
            "CompareBoolEntityData" => {
                check(
                    output == TRUE && activation(g, &source)? == *objective,
                    "result invitation activation",
                )?;
                activations += 1;
            }
            "InterfaceDescriptorData" => {
                // This reusable prefab exports Request, but this selected
                // occurrence has no caller. Do not discard a connected caller.
                check(output == REQUEST, "result invitation external request")?;
                let root = Site {
                    key: source.bus.clone(),
                    ..source.clone()
                };
                check(
                    g.target_field(&root, "Interface")? == source.key
                        && incoming(g, &source)?.is_empty(),
                    "result invitation request interface",
                )?;
                let mut parent = source.clone();
                let frame = parent
                    .parents
                    .pop()
                    .ok_or_else(|| bad("result invitation root request"))?;
                parent.key = frame.reference;
                parent.bus = frame.bus;
                check(
                    incoming(g, &parent)?.is_empty(),
                    "result invitation unmodeled external request",
                )?;
            }
            _ => return Err(bad("result invitation request source")),
        }
        request_sources.push((REQUEST, source, output));
    }
    check(
        activations == 1 && request_sources.len() <= 2,
        "result invitation request count",
    )?;
    let requests = request_sources
        .iter()
        .map(|(_, s, p)| (IN, s.clone(), *p))
        .collect::<Vec<_>>();
    wiring(g, &delay, &requests)?;
    let links = g.links(&checker, DETAILS)?;
    let [details] = links.as_slice() else {
        return Err(bad("result invitation details count"));
    };
    check(
        details.port == 0 && g.object(&details.site)?.class == "DetailsRefererEntityData",
        "result invitation details class",
    )?;
    ports(g, &details.site, &[])?;
    events(g, &details.site, &[])?;
    let o = g.object(&details.site)?;
    check(
        o.fields.get("Id").and_then(Value::as_pointer) == Some(Pointer::Null),
        "result invitation details id",
    )?;
    // This reduction accepts exactly one category; the native checker uses the
    // first category, not a sorted-set minimum when several are authored.
    check(
        o.fields
            .get("Categories")
            .and_then(Value::as_array)
            .is_some_and(|a| a.len() == 1),
        "result invitation category count",
    )?;
    let categories = crate::events::intro::results::categories(g, &details.site)?;
    let category = *categories
        .first()
        .ok_or_else(|| bad("result invitation category"))?;
    let metadata = g.target_field(&checker, "EventMetaData")?;
    let key = metadata_key(
        g,
        &Site {
            key: metadata,
            ..checker
        },
        category,
        map,
    )?;
    Ok((key, duration))
}

pub(super) fn read<S: Source>(
    g: &mut Graph<S>,
    nodes: &[Node],
    world: &[Node],
    bindings: &BTreeMap<Key, usize>,
    stable: u32,
    selected: [&str; 3],
    map: &NarrativeEventMap,
) -> Result<Invitation, Error> {
    let Group {
        senders,
        objective,
        contexts,
        manager,
        hashes,
    } = group(g, nodes, world, bindings, selected)?;
    let good = property(g, &senders[0], SEND, OUT)?;
    let inputs = or(g, &good)?;
    let (good, timeout) = match (&inputs[0], &inputs[1]) {
        (a, b) if a.port == VALUE && b.port == BOOL_OUT => (a, b),
        (a, b) if b.port == VALUE && a.port == BOOL_OUT => (b, a),
        _ => return Err(bad("result invitation default alternatives")),
    };
    let default_after = delayed(g, timeout, &objective)?;
    let bad_result = property(g, &senders[1], SEND, VALUE)?;
    check(
        good.site != bad_result,
        "result invitation distinct result latches",
    )?;
    let (negative, win) = result_latch(g, &good.site, true)?;
    let (other, lose) = result_latch(g, &bad_result, false)?;
    check(
        negative == other && win != lose,
        "result invitation shared recommendation",
    )?;
    let positive = property(g, &senders[2], SEND, VALUE)?;
    let (key, after) = recommendation(g, &positive, &negative, &objective, &contexts, map)?;
    let expected = BTreeSet::from([
        Endpoint { site: win, port: 0 },
        Endpoint {
            site: lose,
            port: 0,
        },
    ]);
    let mut reporters = Vec::new();
    for n in nodes
        .iter()
        .filter(|n| n.object.class == "ParticipantResultReporterEntityData")
    {
        let filters = g.links(&n.site, FILTERS)?;
        if !filters.iter().any(|f| expected.contains(f)) {
            continue;
        }
        check(
            filters.len() == 2 && filters.into_iter().collect::<BTreeSet<_>>() == expected,
            "result invitation filters",
        )?;
        check(
            g.links(&n.site, CONTEXT)? == contexts,
            "result invitation result participant",
        )?;
        reporters.push(n.site.clone());
    }
    let [reporter] = reporters.as_slice() else {
        return Err(bad("result invitation reporter ownership"));
    };
    let mut objectives = Vec::new();
    for n in nodes
        .iter()
        .filter(|n| n.object.class == "RaceObjectiveEntityData")
    {
        if g.event_sources(&n.site, OBJECTIVE_COMPLETE)?
            .iter()
            .any(|(s, p)| s == reporter && *p == REPORTED)
        {
            let (source, port) =
                crate::progression::delays::server_source(g, &n.site, OBJECTIVE_COMPLETE)?;
            check(
                source == *reporter && port == REPORTED,
                "result invitation result completion",
            )?;
            objectives.push(crate::chapter_notifications::stable(g, stable, &n.site)?);
        }
    }
    let [source] = objectives.as_slice() else {
        return Err(bad("result invitation source objective"));
    };
    Invitation::new(
        crate::chapter_notifications::stable(g, stable, &objective)?,
        *source,
        manager,
        hashes,
        key,
        after,
        default_after,
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
