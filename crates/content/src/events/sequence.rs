// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Wander;
use crate::{
    Error,
    graph::{Graph, Node, Source, bad},
};
use nfs_frostbite::ebx::Value;

const COMMANDS: u32 = (-1_858_790_887i32) as u32;
const STARTED: u32 = 216_863_469;
const STOP: u32 = 2_089_401_213;
const IN: u32 = 5_862_146;
const OUT: u32 = 193_453_899;

pub(super) fn wander<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
) -> Result<Option<Wander>, Error> {
    let controls: Vec<_> = nodes
        .iter()
        .filter(|n| n.object.class == "ControlAiInGameModeStateEntityData")
        .collect();
    let mut found = Vec::new();
    for control in controls {
        let commands = graph.links(&control.site, COMMANDS)?;
        let mut wander = None;
        let mut speed = None;
        let mut unsupported = false;
        for command in commands {
            let object = graph.object(&command.site)?;
            if command.port != 0 {
                unsupported = true;
                continue;
            }
            match object.class.as_str() {
                "WanderCommandEntityData" => {
                    if wander.replace(object).is_some() {
                        return Err(bad("several sequence wander commands"));
                    }
                }
                "SpeedLimitCommandEntityData" => {
                    let value = super::finite(object.fields.get("SpeedLimit"), "wander speed")?;
                    if speed.replace(value).is_some() {
                        return Err(bad("several sequence speed limits"));
                    }
                }
                _ => unsupported = true,
            }
        }
        let Some(command) = wander else { continue };
        if unsupported
            || control
                .object
                .fields
                .get("ApplyCommandsOneByOne")
                .and_then(Value::as_bool)
                != Some(false)
            || control
                .object
                .fields
                .get("ApplyCommandsToParticipantsAlreadyInState")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err(bad("unsupported sequence wander composition"));
        }
        let bool_field = |field: &str| {
            command
                .fields
                .get(field)
                .and_then(Value::as_bool)
                .ok_or_else(|| bad("sequence wander flag"))
        };
        let speed_limit = speed
            .filter(|v| *v > 0.)
            .ok_or_else(|| bad("sequence wander speed limit"))?;
        found.push((
            bool_field("IsUrgent")?,
            bool_field("PreferDriveStraightAtIntersection")?,
            speed_limit,
        ));
    }
    if found.is_empty() {
        return Ok(None);
    }
    let [(urgent, prefer_straight, speed_limit)] = found.as_slice() else {
        return Err(bad("ambiguous sequence wander controller"));
    };
    let roots: Vec<_> = nodes
        .iter()
        .filter(|n| n.object.class == "ParticipantStateSequenceEntityData")
        .collect();
    let [root] = roots.as_slice() else {
        return Err(bad("wander sequence root"));
    };
    let mut timeouts = Vec::new();
    for (source, event) in graph.event_sources(&root.site, STOP)? {
        let object = graph.object(&source)?;
        if object.class != "DelayEntityData" || event != OUT {
            continue;
        }
        if object.fields.get("Realm").and_then(Value::as_enum_name) != Some("Realm_Server")
            || object
                .fields
                .get("TimeDeltaType")
                .and_then(Value::as_enum_name)
                != Some("TimeDeltaType_World")
            || object.fields.get("AutoStart").and_then(Value::as_bool) != Some(false)
        {
            return Err(bad("wander timeout realm or clock"));
        }
        let triggers = graph.event_sources(&source, IN)?;
        if triggers != vec![(root.site.clone(), STARTED)] {
            return Err(bad("wander timeout is not driven by sequence start"));
        }
        timeouts.push(super::finite(object.fields.get("Delay"), "wander timeout")?);
    }
    let [timeout] = timeouts.as_slice() else {
        return Err(bad("ambiguous wander timeout"));
    };
    if *timeout <= 0. {
        return Err(bad("wander timeout must be positive"));
    }
    Ok(Some(Wander {
        urgent: *urgent,
        prefer_straight: *prefer_straight,
        speed_limit: *speed_limit,
        timeout: *timeout,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        Site,
        tests::{Memory, edge, local},
    };
    use nfs_frostbite::ebx::Fields;

    fn event(from: usize, out: u32, to: usize, input: u32) -> Value {
        Value::Struct(Fields(vec![
            ("Source".into(), local(from)),
            ("Target".into(), local(to)),
            (
                "SourceEvent".into(),
                Value::Struct(Fields(vec![("Id".into(), Value::UInt32(out))])),
            ),
            (
                "TargetEvent".into(),
                Value::Struct(Fields(vec![("Id".into(), Value::UInt32(input))])),
            ),
        ]))
    }
    fn enumeration(name: &str) -> Value {
        Value::Enum {
            value: 0,
            name: Some(name.into()),
        }
    }
    fn fixture() -> Memory {
        let mut m = Memory::default();
        m.put(
            "a",
            0,
            "Blueprint",
            &[
                (
                    "LinkConnections",
                    Value::Array(vec![
                        edge(local(2), COMMANDS, local(3), 0),
                        edge(local(2), COMMANDS, local(4), 0),
                    ]),
                ),
                (
                    "EventConnections",
                    Value::Array(vec![event(1, STARTED, 5, IN), event(5, OUT, 1, STOP)]),
                ),
            ],
        );
        m.put("a", 1, "ParticipantStateSequenceEntityData", &[]);
        m.put(
            "a",
            2,
            "ControlAiInGameModeStateEntityData",
            &[
                ("ApplyCommandsOneByOne", Value::Bool(false)),
                (
                    "ApplyCommandsToParticipantsAlreadyInState",
                    Value::Bool(false),
                ),
            ],
        );
        m.put(
            "a",
            3,
            "WanderCommandEntityData",
            &[
                ("IsUrgent", Value::Bool(false)),
                ("PreferDriveStraightAtIntersection", Value::Bool(true)),
            ],
        );
        m.put(
            "a",
            4,
            "SpeedLimitCommandEntityData",
            &[("SpeedLimit", Value::Float32(31.))],
        );
        m.put(
            "a",
            5,
            "DelayEntityData",
            &[
                ("Delay", Value::Float32(19.)),
                ("Realm", enumeration("Realm_Server")),
                ("TimeDeltaType", enumeration("TimeDeltaType_World")),
                ("AutoStart", Value::Bool(false)),
            ],
        );
        // Unconnected commands and delays must not affect this controller.
        m.put(
            "a",
            6,
            "SpeedLimitCommandEntityData",
            &[("SpeedLimit", Value::Float32(99.))],
        );
        m
    }
    fn extract(m: &mut Memory) -> Result<Option<Wander>, Error> {
        let nodes: Vec<_> = (1..=6)
            .map(|i| Node {
                site: Site {
                    key: ("a".into(), i),
                    ..Site::root("a")
                },
                object: m.object(&("a".into(), i)).unwrap(),
            })
            .collect();
        wander(&mut Graph::new(m), &nodes)
    }

    #[test]
    fn commands_and_timeout_follow_their_linked_controller_and_sequence() {
        let mut m = fixture();
        assert_eq!(
            extract(&mut m).unwrap(),
            Some(Wander {
                urgent: false,
                prefer_straight: true,
                speed_limit: 31.,
                timeout: 19.
            })
        );
        m.0.get_mut(&("a".into(), 0))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(name, _)| name == "EventConnections")
            .unwrap()
            .1 = Value::Array(vec![event(6, STARTED, 5, IN), event(5, OUT, 1, STOP)]);
        assert!(extract(&mut m).is_err());
    }

    #[test]
    fn ambiguous_commands_and_invalid_timeout_are_refused() {
        let mut m = fixture();
        m.0.get_mut(&("a".into(), 0))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(name, _)| name == "LinkConnections")
            .unwrap()
            .1 = Value::Array(vec![
            edge(local(2), COMMANDS, local(3), 0),
            edge(local(2), COMMANDS, local(4), 0),
            edge(local(2), COMMANDS, local(6), 0),
        ]);
        assert!(extract(&mut m).is_err());
        let mut m = fixture();
        m.0.get_mut(&("a".into(), 5))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(name, _)| name == "Delay")
            .unwrap()
            .1 = Value::Float32(f32::NAN);
        assert!(extract(&mut m).is_err());
        let mut m = fixture();
        m.0.get_mut(&("a".into(), 2))
            .unwrap()
            .fields
            .0
            .iter_mut()
            .find(|(name, _)| name == "ApplyCommandsOneByOne")
            .unwrap()
            .1 = Value::Bool(true);
        assert!(extract(&mut m).is_err());
    }
}
