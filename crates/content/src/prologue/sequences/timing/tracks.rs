// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

/// The server owns only the clock for these tracks. Client UI alpha curves
/// cannot feed server state; any other target or unsupported track is refused.
pub(super) fn client_only<S: Source>(
    graph: &mut Graph<S>,
    source: &Node,
    c: &Composition,
    node: u16,
    millis: u32,
) -> Result<(), Error> {
    let tracks = source
        .object
        .fields
        .get("PropertyTracks")
        .and_then(Value::as_array)
        .filter(|a| a.len() <= 64)
        .ok_or_else(|| bad("sequence property track bound"))?;
    let mut ids = BTreeSet::new();
    for pointer in tracks {
        let key = graph
            .source
            .target(
                &source.site.key.0,
                pointer
                    .as_pointer()
                    .ok_or_else(|| bad("sequence track pointer"))?,
            )?
            .ok_or_else(|| bad("null sequence track"))?;
        let track = graph.source.object(&key)?;
        let f = &track.fields;
        let id = f
            .get("Id")
            .and_then(Value::bits32)
            .ok_or_else(|| bad("sequence track id"))?;
        if track.class != "FloatPropertyTrackData"
            || !ids.insert(id)
            || f.get("InterpolationType").and_then(Value::as_enum_name)
                != Some("InterpolationType_Linear")
        {
            return Err(bad("unsupported sequence property track"));
        }
        let times = f
            .get("Times")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty() && a.len() <= 1024)
            .ok_or_else(|| bad("sequence track times"))?;
        let values = f
            .get("Values")
            .and_then(Value::as_array)
            .filter(|a| a.len() == times.len())
            .ok_or_else(|| bad("sequence track values"))?;
        let mut previous = None;
        for (time, value) in times.iter().zip(values) {
            let time = time
                .bits32()
                .filter(|n| *n <= millis)
                .ok_or_else(|| bad("sequence track time"))?;
            if previous.is_some_and(|p| p >= time)
                || !matches!(value,Value::Float32(v) if v.is_finite())
            {
                return Err(bad("sequence track curve"));
            }
            previous = Some(time);
        }
        let rows = c
            .properties
            .iter()
            .filter(|e| e.source.node == node && e.source.field == id)
            .collect::<Vec<_>>();
        if rows.is_empty()
            || rows.iter().any(|e| {
                e.flags != Some(2)
                    || c.nodes.get(usize::from(e.target.node)).is_none_or(|n| {
                        n.class != "UIScreenRenderEntityData"
                            || n.forward.is_some()
                            || n.listeners.is_none_or(|r| {
                                r.properties != crate::scoring::realms::Realm::Client
                            })
                    })
            })
        {
            return Err(bad("sequence curve is not exclusively client UI"));
        }
    }
    if c.properties
        .iter()
        .any(|e| e.source.node == node && !ids.contains(&e.source.field))
    {
        return Err(bad("unknown sequence property output"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, local};
    use crate::scoring::{
        events::DataConnection,
        realms::{Listeners, Realm},
    };
    use nfs_world::scoring::properties::Port;
    fn fixture() -> (Memory, Node, Composition) {
        let mut m = Memory::default();
        m.put(
            "a",
            1,
            "SequenceEntityData",
            &[("PropertyTracks", Value::Array(vec![local(4)]))],
        );
        m.put(
            "a",
            4,
            "FloatPropertyTrackData",
            &[
                ("Id", Value::UInt32(9)),
                (
                    "Times",
                    Value::Array(vec![Value::UInt32(0), Value::UInt32(2000)]),
                ),
                (
                    "Values",
                    Value::Array(vec![Value::Float32(1.), Value::Float32(0.)]),
                ),
                (
                    "InterpolationType",
                    Value::Enum {
                        value: 1,
                        name: Some("InterpolationType_Linear".into()),
                    },
                ),
            ],
        );
        let site = Site {
            key: ("a".into(), 1),
            ..Site::root("a")
        };
        let source = Node {
            site: site.clone(),
            object: m.0[&site.key].clone(),
        };
        let c = Composition {
            nodes: vec![crate::scoring::events::Node {
                identity: Identity::from(&site),
                class: "UIScreenRenderEntityData".into(),
                listeners: Some(Listeners {
                    events: Realm::Client,
                    properties: Realm::Client,
                }),
                forward: None,
            }],
            buses: vec![],
            bus_entities: vec![],
            connections: vec![],
            links: vec![],
            properties: vec![DataConnection {
                source: Port { node: 1, field: 9 },
                target: Port {
                    node: 0,
                    field: 205677681,
                },
                bus: 0,
                index: 0,
                flags: Some(2),
            }],
        };
        (m, source, c)
    }
    #[test]
    fn only_bounded_client_ui_curves_can_accompany_a_server_timer() {
        let (mut m, node, c) = fixture();
        assert!(client_only(&mut Graph::new(&mut m), &node, &c, 1, 2000).is_ok());
        for case in 0..5 {
            let (mut m, node, mut c) = fixture();
            match case {
                0 => c.properties[0].flags = Some(3),
                1 => c.nodes[0].listeners.as_mut().unwrap().properties = Realm::Server,
                2 => {
                    m.0.get_mut(&("a".into(), 4)).unwrap().fields.0[2].1 =
                        Value::Array(vec![Value::Float32(f32::NAN), Value::Float32(0.)])
                }
                3 => c.properties.clear(),
                _ => c.properties[0].source.field = 10,
            }
            assert!(
                client_only(&mut Graph::new(&mut m), &node, &c, 1, 2000).is_err(),
                "case {case}"
            );
        }
    }
}
