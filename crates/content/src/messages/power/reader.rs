// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::super::conditional::*;
use super::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

const LESS: u32 = 193_445_146;
const GE: u32 = 2_088_727_237;
const A: u32 = 177_636;
const B: u32 = 177_639;
const INT_IN: u32 = 214_528_497;
const INT_OUT: u32 = (-1_510_504_872i32) as u32;
const CURRENT: u32 = nfs_world::scoring::stats::MAX_HP;
const INPUTS: [u32; 2] = [193_450_866, 193_450_867];
const STOCK: u32 = nfs_world::scoring::stats::STOCK_MAX_HP;

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
pub(super) fn read<S: Source>(
    g: &mut Graph<S>,
    nodes: &[Node],
    world: &[Node],
    bindings: &BTreeMap<Key, usize>,
    stable: u32,
    selected: [&str; 3],
) -> Result<Invitation, Error> {
    let Group {
        senders,
        objective,
        contexts,
        manager,
        hashes,
    } = group(g, nodes, world, bindings, selected)?;
    let basic_or = property(g, &senders[0], SEND, OUT)?;
    let basic_inputs = or(g, &basic_or)?;
    let (basic, timeout) = match (&basic_inputs[0], &basic_inputs[1]) {
        (a, b) if a.port == VALUE && b.port == BOOL_OUT => (a, b),
        (a, b) if b.port == VALUE && a.port == BOOL_OUT => (b, a),
        _ => return Err(bad("power invitation default alternatives")),
    };
    let delay = delayed(g, timeout, &objective)?;
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
        g.links(&stock, CONTEXT)? == contexts,
        "power invitation vehicle participant",
    )?;
    let compare = event(g, &latched, OPEN, TRUE)?;
    check(
        activation(g, &compare)? == objective,
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
        crate::chapter_notifications::stable(g, stable, &objective)?,
        manager,
        hashes,
        [low, high],
        delay,
    )
}
