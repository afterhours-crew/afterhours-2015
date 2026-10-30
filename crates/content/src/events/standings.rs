// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use crate::{
    Error,
    graph::{Graph, Node, Source, bad},
};
use nfs_frostbite::ebx::Value;

const REFRESH: u32 = 1_783_371_838;
const OUT: u32 = 193_453_899;
const IN: u32 = 5_862_146;

/// The collector's prefab owns the channel that loads the rank cache. Follow
/// that channel's event source and require its authored repeating server timer.
pub(super) fn interval<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
) -> Result<Option<f32>, Error> {
    let collectors: Vec<_> = nodes
        .iter()
        .filter(|n| n.object.class == "EventInfoPresentationDataCollectorEntityData")
        .collect();
    if collectors.is_empty() {
        return Ok(None);
    }
    let [collector] = collectors.as_slice() else {
        return Err(bad("standings collector count"));
    };
    let channels: Vec<_> = nodes
        .iter()
        .filter(|n| {
            n.object.class == "LinkedSchematicChannelEntityData"
                && n.site.bus == collector.site.bus
                && n.site.parents == collector.site.parents
        })
        .collect();
    let [channel] = channels.as_slice() else {
        return Err(bad("standings refresh channel count"));
    };
    let sources = graph.event_sources(&channel.site, REFRESH)?;
    let [(timer, OUT)] = sources.as_slice() else {
        return Err(bad("standings refresh timer source"));
    };
    if graph.event_sources(timer, IN)? != [(timer.clone(), OUT)] {
        return Err(bad("standings timer is not self-repeating"));
    }
    let object = graph.object(timer)?;
    if object.class != "DelayEntityData"
        || object.fields.get("Realm").and_then(Value::as_enum_name) != Some("Realm_Server")
        || object
            .fields
            .get("TimeDeltaType")
            .and_then(Value::as_enum_name)
            != Some("TimeDeltaType_World")
        || object.fields.get("AutoStart").and_then(Value::as_bool) != Some(true)
        || object.fields.get("RunOnce").and_then(Value::as_bool) != Some(false)
        || object
            .fields
            .get("RemoveDuplicateEvents")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(bad("unsupported standings timer"));
    }
    let delay = super::finite(object.fields.get("Delay"), "standings refresh interval")?;
    if !(0.01..=60.).contains(&delay) {
        return Err(bad("standings refresh interval bound"));
    }
    Ok(Some(delay))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        Site,
        tests::{Memory, local},
    };
    use nfs_frostbite::ebx::Fields;

    fn fields(values: &[(&str, Value)]) -> Value {
        Value::Struct(Fields(
            values
                .iter()
                .map(|(k, v)| ((*k).into(), v.clone()))
                .collect(),
        ))
    }
    fn edge(target: usize, event: u32) -> Value {
        fields(&[
            ("Source", local(3)),
            ("Target", local(target)),
            ("SourceEvent", fields(&[("Id", Value::UInt32(OUT))])),
            ("TargetEvent", fields(&[("Id", Value::UInt32(event))])),
        ])
    }
    fn fixture(delay: f32) -> Memory {
        let mut m = Memory::default();
        m.put(
            "a",
            0,
            "Blueprint",
            &[(
                "EventConnections",
                Value::Array(vec![edge(2, REFRESH), edge(3, IN)]),
            )],
        );
        m.put("a", 1, "EventInfoPresentationDataCollectorEntityData", &[]);
        m.put("a", 2, "LinkedSchematicChannelEntityData", &[]);
        let enumeration = |name: &str| Value::Enum {
            value: 0,
            name: Some(name.into()),
        };
        m.put(
            "a",
            3,
            "DelayEntityData",
            &[
                ("Realm", enumeration("Realm_Server")),
                ("TimeDeltaType", enumeration("TimeDeltaType_World")),
                ("AutoStart", Value::Bool(true)),
                ("RunOnce", Value::Bool(false)),
                ("RemoveDuplicateEvents", Value::Bool(true)),
                ("Delay", Value::Float32(delay)),
            ],
        );
        m
    }
    fn read(m: &mut Memory) -> Result<Option<f32>, Error> {
        let nodes = (1..=3)
            .map(|i| Node {
                site: Site {
                    key: ("a".into(), i),
                    ..Site::root("a")
                },
                object: m.0[&("a".into(), i)].clone(),
            })
            .collect::<Vec<_>>();
        interval(&mut Graph::new(m), &nodes)
    }
    #[test]
    fn follows_refresh_channel_and_rejects_missing_repeat_or_invalid_timing() {
        assert_eq!(read(&mut fixture(0.3)).unwrap(), Some(0.3));
        assert_eq!(read(&mut fixture(0.125)).unwrap(), Some(0.125));
        for delay in [0., -1., f32::NAN, f32::INFINITY, 60.1] {
            assert!(read(&mut fixture(delay)).is_err());
        }
        let mut m = fixture(0.3);
        m.put(
            "a",
            0,
            "Blueprint",
            &[("EventConnections", Value::Array(vec![edge(2, REFRESH)]))],
        );
        assert!(read(&mut m).is_err());
        let mut m = fixture(0.3);
        m.put(
            "a",
            0,
            "Blueprint",
            &[(
                "EventConnections",
                Value::Array(vec![edge(2, REFRESH + 1), edge(3, IN)]),
            )],
        );
        assert!(read(&mut m).is_err());
    }

    #[test]
    fn mode_without_position_collector_has_no_position_refresh_timer() {
        let mut m = fixture(0.3);
        m.put("a", 1, "OtherCollector", &[]);
        assert_eq!(read(&mut m).unwrap(), None);
    }
}
