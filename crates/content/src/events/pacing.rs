// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Binding;
use crate::{
    Error,
    graph::{Graph, Node, Site, Source, bad},
    scoring::compile_curve_rows,
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::race_pacing::{AiOnlyMode, Config, MAX_OPPONENTS, Schedule};
use std::collections::BTreeMap;

const PROVIDER: u32 = 1_588_117_075;
const ORDINALS: [&str; MAX_OPPONENTS] = [
    "First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh", "Eighth", "Ninth", "Tenth",
    "Eleventh",
];

fn schedule(object: &Object) -> Result<Config, Error> {
    if object.class != "RacePacingScheduleGroup" {
        return Err(bad("unsupported race pacing schedule"));
    }
    let field = |name: &str| {
        object
            .fields
            .get(name)
            .ok_or_else(|| bad("race pacing field"))
    };
    let curve = |name: &str| {
        let value = field(name)?;
        if value.as_array().is_some_and(|a| a.is_empty()) {
            Ok(None)
        } else {
            compile_curve_rows(value, 1., 1.).map(Some)
        }
    };
    let mut groups = Vec::new();
    for count in 1..=MAX_OPPONENTS {
        let mut group = Vec::new();
        for ordinal in ORDINALS.iter().take(count) {
            let suffix = if count == 1 {
                "1".into()
            } else {
                format!("{count}_{ordinal}")
            };
            group.push(Schedule {
                target: curve(&format!("TargetSchedule_{suffix}"))?,
                skill: curve(&format!("SkillModifier_{suffix}"))?,
            });
        }
        groups.push(group);
    }
    let mode = match field("AiOnlyPacingMode")?.as_enum_name() {
        Some("AiPacingMode_Front") => AiOnlyMode::First,
        Some("AiPacingMode_Back") => AiOnlyMode::Last,
        Some("AiPacingMode_Average") => AiOnlyMode::Average,
        _ => return Err(bad("race pacing AI-only mode")),
    };
    Config::new(
        groups,
        super::finite(Some(field("ExtraTargetOffset")?), "pacing extra offset")?,
        super::finite(Some(field("TargetOffsetScale")?), "pacing offset scale")?,
        mode,
    )
    .map_err(|e| bad(&e.to_string()))
}

/// Resolve the command through the launched game mode's bound channel, never a
/// matching provider elsewhere in the scene or another event variant.
pub(super) fn extract<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    bindings: &BTreeMap<Site, (Site, Binding)>,
) -> Result<Option<Config>, Error> {
    let commands: Vec<_> = nodes
        .iter()
        .filter(|n| n.object.class == "WaypointSeriesPacingGroupCommandEntityData")
        .collect();
    if commands.is_empty() {
        return Ok(None);
    }
    let [command] = commands.as_slice() else {
        return Err(bad("race pacing command count"));
    };
    // Native command creation uses the first controlled car when this locator
    // is unbound. An authored override needs to be carried explicitly.
    if !graph.links(&command.site, 0x4c84_a858)?.is_empty() {
        return Err(bad("authored pacing start locator is unsupported"));
    }
    let reference = graph.links(&command.site, PROVIDER)?;
    let [reference] = reference.as_slice() else {
        return Err(bad("race pacing channel count"));
    };
    if graph.object(&reference.site)?.class != "LinkedSchematicChannelReferenceEntityData" {
        return Err(bad("race pacing channel class"));
    }
    let (channel, _) = bindings
        .get(&reference.site)
        .ok_or_else(|| bad("race pacing channel is not bound"))?;
    let providers = graph.links(channel, reference.port)?;
    let [provider] = providers.as_slice() else {
        return Err(bad("race pacing provider count"));
    };
    let object = graph.object(&provider.site)?;
    if provider.port != 0
        || object.class != "PacingScheduleProviderEntityData"
        || object
            .fields
            .get("PacingRelativeToPlayer")
            .and_then(Value::as_bool)
            != Some(true)
        || object.fields.get("IsDriftTrain").and_then(Value::as_bool) != Some(false)
    {
        return Err(bad("unsupported race pacing provider"));
    }
    let key = graph.target_field(&provider.site, "Schedule")?;
    Ok(Some(schedule(&graph.source.object(&key)?)?))
}

#[cfg(test)]
mod tests;
