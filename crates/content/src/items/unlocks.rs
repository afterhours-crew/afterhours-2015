// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored REP comparisons that grant visual category bits. Debug-all events
//! and award-item branches are deliberately outside this projection.
use crate::{Error, graph::bad, scan::Assets};
use nfs_frostbite::ebx::{Fields, Object, Pointer, Value};
use nfs_services::reputation::Thresholds;
use serde_json::{Value as Json, json};

fn get<'a>(f: &'a Fields, key: &str) -> Result<&'a Value, Error> {
    f.get(key).ok_or_else(|| bad("unlock graph field"))
}
fn number(f: &Fields, key: &str) -> Result<u32, Error> {
    get(f, key)?.bits32().ok_or_else(|| bad("unlock integer"))
}
fn ptr(f: &Fields, key: &str) -> Result<usize, Error> {
    match get(f, key)?.as_pointer() {
        Some(Pointer::Internal(i)) => Ok(i),
        _ => Err(bad("unlock local edge")),
    }
}
fn port(f: &Fields, key: &str) -> Result<u32, Error> {
    number(
        get(f, key)?
            .as_fields()
            .ok_or_else(|| bad("unlock event"))?,
        "Id",
    )
}
fn array<'a>(f: &'a Fields, key: &str) -> Result<&'a [Value], Error> {
    get(f, key)?
        .as_array()
        .filter(|a| a.len() <= 4096)
        .ok_or_else(|| bad("unlock array"))
}
fn unique<'a>(mut values: impl Iterator<Item = &'a Fields>) -> Result<&'a Fields, Error> {
    let value = values.next().ok_or_else(|| bad("missing unlock edge"))?;
    if values.next().is_some() {
        return Err(bad("ambiguous unlock edge"));
    }
    Ok(value)
}
fn source<'a>(
    objects: &'a [Object],
    edges: &[&Fields],
    target: usize,
    field: u32,
) -> Result<&'a Object, Error> {
    let edge = unique(edges.iter().copied().filter(|e| {
        ptr(e, "Target").ok() == Some(target) && number(e, "TargetFieldId").ok() == Some(field)
    }))?;
    objects
        .get(ptr(edge, "Source")?)
        .ok_or_else(|| bad("unlock object index"))
}
pub(super) fn rules(objects: &[Object], thresholds: &Thresholds) -> Result<Json, Error> {
    let root = objects
        .first()
        .filter(|o| o.class == "ParticipantLogicBlueprint")
        .ok_or_else(|| bad("unlock blueprint"))?;
    let properties = array(&root.fields, "PropertyConnections")?
        .iter()
        .map(|v| v.as_fields().ok_or_else(|| bad("unlock property")))
        .collect::<Result<Vec<_>, _>>()?;
    let events = array(&root.fields, "EventConnections")?
        .iter()
        .map(|v| v.as_fields().ok_or_else(|| bad("unlock event")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut rows = Vec::new();
    for (i, object) in objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.class == "UnlockItemCategoryEntityData")
    {
        let matches = events
            .iter()
            .copied()
            .filter(|e| {
                ptr(e, "Target").ok() == Some(i) && port(e, "SourceEvent").ok() == Some(1653725917)
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            continue;
        }
        let edge = unique(matches.into_iter())?;
        if port(edge, "TargetEvent")? != 287098660 {
            return Err(bad("unsupported unlock event"));
        }
        let compare = ptr(edge, "Source")?;
        let cmp = objects
            .get(compare)
            .filter(|o| o.class == "CompareIntRangeEntityData")
            .ok_or_else(|| bad("unlock comparison"))?;
        if get(&cmp.fields, "TriggerOnStart")?.as_bool() != Some(false) {
            return Err(bad("unlock start policy"));
        }
        // Compare Value is the authored level; its upper bound is the current
        // participant level. The lower bound is the cached previous level +1.
        let integer = source(objects, &properties, compare, 225375086)?;
        let upper = source(objects, &properties, compare, 193446865)?;
        let lower = source(objects, &properties, compare, 193446607)?;
        if integer.class != "IntEntityData"
            || upper.class != "IntParticipantPropertyAccessorEntityData"
            || lower.class != "MathEntityData"
        {
            return Err(bad("unlock REP wiring"));
        }
        let level = number(&integer.fields, "DefaultValue")?;
        let score = thresholds
            .score_for_level(level)
            .map_err(|_| bad("unlock REP level"))?;
        for value in array(&object.fields, "Categories")? {
            let Value::Enum {
                value,
                name: Some(name),
            } = value
            else {
                return Err(bad("unlock mask enum"));
            };
            if !name.starts_with("CategoryUnlockMask_")
                || *value <= 0
                || !(*value as u32).is_power_of_two()
            {
                return Err(bad("unlock mask bit"));
            }
            rows.push(json!({"mask":*value as u32,"minimum_rep_score":score}));
            if rows.len() > 32 {
                return Err(bad("unlock rule bound"));
            }
        }
    }
    if rows.is_empty() {
        return Err(bad("missing REP unlock rules"));
    }
    Ok(json!(rows))
}
pub(super) fn build(assets: &mut Assets, thresholds: &Thresholds) -> Result<Json, Error> {
    let controllers = assets.named("CategoryUnlockControllerItemData");
    if controllers.is_empty() {
        return Ok(Json::Null);
    }
    if controllers.len() != 1 {
        return Err(bad("ambiguous category controller"));
    }
    let controller = assets
        .objects(&controllers[0])?
        .first()
        .and_then(|o| o.guid)
        .ok_or_else(|| bad("category controller GUID"))?;
    let requirements = rules(
        assets.objects("gameplay/progression/unlocksparticipantlogic")?,
        thresholds,
    )?;
    Ok(json!({"controller":nfs_frostbite::hex(&controller),"requirements":requirements}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(entries: Vec<(&str, Value)>) -> Fields {
        Fields(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    fn object(class: &str, fields: Fields) -> Object {
        Object {
            class: class.into(),
            guid: None,
            fields,
        }
    }
    fn p(i: usize) -> Value {
        Value::Pointer(Pointer::Internal(i))
    }
    fn field(source: usize, target: usize, port: u32) -> Value {
        Value::Struct(f(vec![
            ("Source", p(source)),
            ("Target", p(target)),
            ("TargetFieldId", Value::UInt32(port)),
        ]))
    }
    fn fixture() -> Vec<Object> {
        let event = Value::Struct(f(vec![
            ("Source", p(2)),
            ("Target", p(1)),
            (
                "SourceEvent",
                Value::Struct(f(vec![("Id", Value::UInt32(1653725917))])),
            ),
            (
                "TargetEvent",
                Value::Struct(f(vec![("Id", Value::UInt32(287098660))])),
            ),
        ]));
        vec![
            object(
                "ParticipantLogicBlueprint",
                f(vec![
                    (
                        "PropertyConnections",
                        Value::Array(vec![
                            field(3, 2, 225375086),
                            field(4, 2, 193446865),
                            field(5, 2, 193446607),
                        ]),
                    ),
                    ("EventConnections", Value::Array(vec![event])),
                ]),
            ),
            object(
                "UnlockItemCategoryEntityData",
                f(vec![(
                    "Categories",
                    Value::Array(vec![Value::Enum {
                        value: 8,
                        name: Some("CategoryUnlockMask_Synthetic".into()),
                    }]),
                )]),
            ),
            object(
                "CompareIntRangeEntityData",
                f(vec![("TriggerOnStart", Value::Bool(false))]),
            ),
            object("IntEntityData", f(vec![("DefaultValue", Value::UInt32(3))])),
            object("IntParticipantPropertyAccessorEntityData", f(vec![])),
            object("MathEntityData", f(vec![])),
        ]
    }
    #[test]
    fn authored_level_resolves_score_and_unknown_or_ambiguous_wiring_fails() {
        let thresholds = Thresholds::new(vec![0, 10, 25]).unwrap();
        let objects = fixture();
        assert_eq!(
            rules(&objects, &thresholds).unwrap(),
            json!([{"mask":8,"minimum_rep_score":25}])
        );
        let mut invalid = objects.clone();
        invalid[3].fields = f(vec![("DefaultValue", Value::UInt32(4))]);
        assert!(rules(&invalid, &thresholds).is_err());
        let mut invalid = objects.clone();
        invalid[4].class = "IntEntityData".into();
        assert!(rules(&invalid, &thresholds).is_err());
        let mut invalid = objects.clone();
        let Value::Array(edges) = &mut invalid[0].fields.0[0].1 else {
            panic!()
        };
        edges.push(edges[0].clone());
        assert!(rules(&invalid, &thresholds).is_err());
        let mut invalid = objects;
        invalid[1].fields = f(vec![(
            "Categories",
            Value::Array(vec![Value::Enum {
                value: 3,
                name: Some("CategoryUnlockMask_Invalid".into()),
            }]),
        )]);
        assert!(rules(&invalid, &thresholds).is_err());
    }
}
