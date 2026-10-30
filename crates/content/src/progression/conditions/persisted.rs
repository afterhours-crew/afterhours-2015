// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A Boolean completely driven by a persisted integer's >0 / <=0 outputs.
//! Arbitrary event-written latches cannot be reduced to their startup default.
use super::*;

const TRUE: u32 = (-1_541_066_415i32) as u32;
const FALSE: u32 = 668_205_626;
const GREATER: u32 = 193_445_336;
const LESS_EQUAL: u32 = 2_088_725_063;
const A: u32 = 177_636;
const INT_VALUE: u32 = 993_810_272;

fn check(ok: bool) -> Result<(), Error> {
    if ok {
        Ok(())
    } else {
        Err(bad("unsupported persisted Boolean projection"))
    }
}
fn events<S: Source>(g: &mut Graph<S>, site: &Site, ports: &[u32]) -> Result<(), Error> {
    let bus = g.source.object(&site.bus)?;
    let rows = bus
        .fields
        .get("EventConnections")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= crate::scenes::MAX_GRAPH_OBJECTS)
        .ok_or_else(|| bad("persisted Boolean event bound"))?;
    let mut actual = BTreeSet::new();
    for row in rows {
        let f = row
            .as_fields()
            .ok_or_else(|| bad("persisted Boolean event row"))?;
        let target = f
            .get("Target")
            .and_then(Value::as_pointer)
            .ok_or_else(|| bad("persisted Boolean target"))?;
        if g.source.target(&site.bus.0, target)?.as_ref() != Some(&site.key) {
            continue;
        }
        check(
            f.get("TargetType").and_then(Value::as_enum_name)
                == Some("EventConnectionTargetType_Server"),
        )?;
        let port = f
            .get("TargetEvent")
            .and_then(Value::as_fields)
            .and_then(|f| f.get("Id"))
            .and_then(Value::bits32)
            .ok_or_else(|| bad("persisted Boolean event port"))?;
        check(actual.insert(port))?;
    }
    check(actual == ports.iter().copied().collect())
}

pub(super) fn positive_integer<S: Source>(
    g: &mut Graph<S>,
    site: &Site,
    stable: u32,
) -> Result<u64, Error> {
    let latch = g.object(site)?;
    check(
        latch.fields.get("DefaultValue").and_then(Value::as_bool) == Some(false)
            && latch.fields.get("Realm").and_then(Value::as_enum_name) == Some("Realm_Server"),
    )?;
    check(g.input_ports(site)?.is_empty())?;
    events(g, site, &[TRUE, FALSE])?;
    let positive = g.event_sources(site, TRUE)?;
    let negative = g.event_sources(site, FALSE)?;
    let [(compare, GREATER)] = positive.as_slice() else {
        return Err(bad("persisted Boolean positive writer"));
    };
    check(negative == [(compare.clone(), LESS_EQUAL)])?;
    let object = g.object(compare)?;
    check(
        object.class == "CompareIntEntityData"
            && object.fields.get("Realm").and_then(Value::as_enum_name) == Some("Realm_Server")
            && object
                .fields
                .get("TriggerOnPropertyChange")
                .and_then(Value::as_bool)
                == Some(true)
            && object.fields.get("TriggerOnStart").and_then(Value::as_bool) == Some(false)
            && object.fields.get("A").and_then(Value::bits32) == Some(0)
            && object.fields.get("B").and_then(Value::bits32) == Some(0),
    )?;
    check(g.input_ports(compare)? == [A])?;
    events(g, compare, &[])?;
    let input = g.input(compare, A)?;
    let value = g.object(&input.site)?;
    check(
        input.port == INT_VALUE
            && value.class == "IntValueData"
            && value.fields.get("Realm").and_then(Value::as_enum_name) == Some("Realm_Server")
            && value
                .fields
                .get("StableIdIsRequired")
                .and_then(Value::as_bool)
                == Some(true),
    )?;
    crate::chapter_notifications::stable(g, stable, &input.site)
}
