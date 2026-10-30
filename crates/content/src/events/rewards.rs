// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::{
    Binding, Rewards,
    values::{Evaluator, Number},
};
use crate::{Error, graph::*};
use std::collections::{BTreeMap, BTreeSet};

const CASH: &str = "gameplay/racerewards";
const REP: &str = "gameplay/scoring/eventreprewards";
// The result hubs select first/second/third/other with the one-based position
// clamped to four, with last place redirected to "other". The channel ports
// are distinct for cash and event REP.
const CASH_PORTS: [u32; 4] = [
    (-162_923_685i32) as u32,
    (-1_586_307_467i32) as u32,
    1_642_915_848,
    (-557_191_063i32) as u32,
];
const REP_PORTS: [u32; 4] = [
    (-1_485_681_749i32) as u32,
    (-182_181_947i32) as u32,
    240_864_984,
    261_720_921,
];

fn channel<S: Source>(
    graph: &mut Graph<S>,
    bindings: &BTreeMap<Site, (Site, Binding)>,
    asset: &str,
) -> Result<Site, Error> {
    let mut found = BTreeSet::new();
    for (site, _) in bindings.values() {
        if graph.object(site)?.class == "LinkedSchematicChannelEntityData"
            && graph.target_field(site, "ChannelAsset")?.0 == asset
        {
            found.insert(site.clone());
        }
    }
    let [site] = <[Site; 1]>::try_from(found.into_iter().collect::<Vec<_>>()).map_err(|_| {
        bad(&format!(
            "missing or ambiguous bound reward channel {asset}"
        ))
    })?;
    Ok(site)
}

fn amount<S: Source>(graph: &mut Graph<S>, channel: &Site, port: u32) -> Result<i32, Error> {
    let sources = graph.property_sources(channel, port)?;
    let mut calculated = Vec::new();
    let mut defaults = Vec::new();
    for source in sources {
        let object = graph.object(&source.site)?;
        match object.class.as_str() {
            "MathEntityData"
                if object
                    .fields
                    .get("EvaluateOnEvent")
                    .and_then(nfs_frostbite::ebx::Value::as_bool)
                    == Some(false) =>
            {
                calculated.push(source)
            }
            "IntHubEntityData" => calculated.push(source),
            "InterfaceDescriptorData" => defaults.push(source),
            _ => return Err(bad("unsupported reward channel source")),
        }
    }
    // The authored initial fallback and the start-evaluated calculator both
    // feed the channel. Math startup evaluation / hub selection propagates the
    // calculated reward after property initialization. Do not impose this rule
    // on arbitrary multi-source property inputs.
    let [source] = <[Endpoint; 1]>::try_from(calculated)
        .map_err(|_| bad("reward needs one startup calculator"))?;
    if defaults.len() > 1 {
        return Err(bad("ambiguous initial reward fallback"));
    }
    let mut values = Evaluator::new(graph);
    for fallback in defaults {
        if !matches!(values.output(&fallback.site, fallback.port)?, Number::Integer(v) if v >= 0) {
            return Err(bad("invalid initial reward fallback"));
        }
    }
    match values.output(&source.site, source.port)? {
        Number::Integer(v) if v >= 0 => Ok(v),
        _ => Err(bad("reward is not a nonnegative integer")),
    }
}

pub(super) fn extract<S: Source>(
    graph: &mut Graph<S>,
    bindings: &BTreeMap<Site, (Site, Binding)>,
    first: Rewards,
) -> Result<[Rewards; 4], Error> {
    let cash = channel(graph, bindings, CASH)?;
    let rep = channel(graph, bindings, REP)?;
    let mut tiers = [first; 4];
    for (index, reward) in tiers.iter_mut().enumerate() {
        *reward = Rewards {
            cash: amount(graph, &cash, CASH_PORTS[index])?,
            rep: amount(graph, &rep, REP_PORTS[index])?,
        };
    }
    if tiers[0] != first {
        return Err(bad(
            "first-place channel rewards disagree with the start area",
        ));
    }
    Ok(tiers)
}

#[cfg(test)]
mod tests;
