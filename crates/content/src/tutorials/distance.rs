// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A tutorial's server distance check (installed: accelerate). A server
//! CompareInt on the participant's driven distance starts a StopWatch once
//! the distance reaches `start`; when the watch reaches `seconds` a second
//! CompareInt sets the seen value if the distance reached `enough` and
//! otherwise fires the networked prompt rows (official E1026: created
//! 34.631 s, deleted 146.119 s without a fire after the first drive).
use super::*;

const COMPARE_INT: &str = "CompareIntEntityData";
const STOPWATCH: &str = "StopWatchEntityData";
const STOPWATCH_START: u32 = 230_748_069;
const STOPWATCH_TRIGGER: u32 = 1_975_570_652;
const STOPWATCH_TIME: u32 = 2_068_878_889;
const COMPARE_IN: u32 = 5_862_146;
const COMPARE_A: u32 = 177_636;
const AT_LEAST: u32 = 2_088_727_237;
const BELOW: u32 = 193_445_146;
const INT_ACCESSOR: &str = "IntParticipantPropertyAccessorEntityData";
const BOOL_SET_TRUE: u32 = 2_753_900_881;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Distance {
    /// Metres at which the watch starts.
    pub start: i32,
    /// Milliseconds the watch runs before the check.
    pub millis: u32,
    /// Metres that count as having driven.
    pub enough: i32,
    /// Networked rows fired when the check fails (the prompt).
    pub prompt: Vec<(u32, u32)>,
}

fn int_field(object: &nfs_frostbite::ebx::Object, name: &str) -> Result<i32, Error> {
    match object.fields.get(name) {
        Some(v) => Ok(bits32(v)? as i32),
        None => Ok(0),
    }
}

/// The distance check of one logic: a server StopWatch started by a
/// distance CompareInt. Other StopWatch shapes (copsintro, earningcash:
/// started by participant-state reporters) are not distance checks.
pub(super) fn distance<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    network: &[crate::logic_events::Event],
) -> Result<Option<Distance>, Error> {
    let watches: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.object.class == STOPWATCH && server(&n.object))
        .collect();
    let watch = match watches.as_slice() {
        [] => return Ok(None),
        [one] => *one,
        _ => return Err(bad("tutorial stopwatch count")),
    };
    let starts = graph.event_sources(&watch.site, STOPWATCH_START)?;
    let [(starter, AT_LEAST)] = starts.as_slice() else {
        return Ok(None);
    };
    let starter_object = graph.object(starter)?;
    if starter_object.class != COMPARE_INT || !server(&starter_object) {
        return Ok(None);
    }
    let input = graph.input(starter, COMPARE_A)?;
    if graph.object(&input.site)?.class != INT_ACCESSOR {
        return Ok(None);
    }
    let time = graph.input(&watch.site, STOPWATCH_TIME)?;
    let time_object = graph.object(&time.site)?;
    let Some(Value::Float32(seconds)) = time_object.fields.get("DefaultValue") else {
        return Err(bad("tutorial stopwatch time"));
    };
    if !(0.0..=600.0).contains(seconds) {
        return Err(bad("tutorial stopwatch time bound"));
    }
    let mut checks = vec![];
    for n in nodes
        .iter()
        .filter(|n| n.object.class == COMPARE_INT && server(&n.object))
    {
        if graph
            .event_sources(&n.site, COMPARE_IN)?
            .iter()
            .any(|(s, port)| *s == watch.site && *port == STOPWATCH_TRIGGER)
        {
            checks.push(n);
        }
    }
    let [check] = checks.as_slice() else {
        return Err(bad("tutorial stopwatch check"));
    };
    // Both compares read the same participant Int (the driven distance),
    // and the passing check sets a server Bool (the seen value).
    let (a, b) = (
        graph.input(starter, COMPARE_A)?,
        graph.input(&check.site, COMPARE_A)?,
    );
    if a.site != b.site || a.port != b.port {
        return Err(bad("tutorial distance input"));
    }
    let mut sets = false;
    for n in nodes
        .iter()
        .filter(|n| n.object.class == "BoolEntityData" && server(&n.object))
    {
        sets |= graph
            .event_sources(&n.site, BOOL_SET_TRUE)?
            .iter()
            .any(|(s, port)| *s == check.site && *port == AT_LEAST);
    }
    if !sets {
        return Err(bad("tutorial distance seen"));
    }
    let prompt: Vec<(u32, u32)> = network
        .iter()
        .filter(|e| {
            e.source.index == check.site.key.1
                && e.source.asset.eq_ignore_ascii_case(&check.site.key.0)
                && e.source.port == BELOW
        })
        .map(|e| (e.bus, e.ordinal))
        .collect();
    if prompt.is_empty() || prompt.len() > MAX_ROWS {
        return Err(bad("tutorial distance prompt rows"));
    }
    Ok(Some(Distance {
        start: int_field(&starter_object, "B")?,
        millis: (*seconds * 1000.0).round() as u32,
        enough: int_field(&check.object, "B")?,
        prompt,
    }))
}
