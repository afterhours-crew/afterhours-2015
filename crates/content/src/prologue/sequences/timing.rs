// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Static server completion paths driven directly by sequence Start. Client
//! presentation remains in the client; dynamic timelines are rejected here.
use super::*;
use crate::scoring::events::{Composition, Identity};
use nfs_world::scoring::events::Delivery;
const STARTED: u32 = 216_863_469;
const STOP: u32 = 2_089_401_213;
const PLAY: u32 = 230_748_069;
const FINISHED: u32 = 650_265_526;
const IN: u32 = 5_862_146;
const OUT: u32 = 193_453_899;

fn server(delivery: Delivery) -> bool {
    matches!(
        delivery,
        Delivery::Local | Delivery::NetworkedClientAndServer(_)
    )
}

pub fn extract(index: &mut AssetIndex, name: &str) -> Result<Option<f64>, Error> {
    let composition = index.scoring_events(name)?;
    let mut graph = Graph::new(&mut index.assets);
    let nodes = graph.read(Site::root(name))?;
    read(&mut graph, &nodes, &composition)
}

fn read<S: Source>(
    graph: &mut Graph<S>,
    nodes: &[Node],
    c: &Composition,
) -> Result<Option<f64>, Error> {
    let roots = c
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.class == "ParticipantStateSequenceEntityData")
        .map(|(i, _)| i as u16)
        .collect::<Vec<_>>();
    let [root] = roots.as_slice() else {
        return Err(bad("timed sequence root"));
    };
    let mut deadlines = vec![];
    for edge in c
        .connections
        .iter()
        .filter(|e| e.target.node == *root && e.target.event == STOP && server(e.delivery))
    {
        if edge.source.node == *root && edge.source.event == STARTED {
            deadlines.push(0.);
            continue;
        }
        let node = c
            .nodes
            .get(usize::from(edge.source.node))
            .ok_or_else(|| bad("timed sequence source"))?;
        let (input, output) = match node.class.as_str() {
            "DelayEntityData" => (IN, OUT),
            "SequenceEntityData" => (PLAY, FINISHED),
            _ => continue,
        };
        if edge.source.event != output {
            continue;
        }
        let source = nodes
            .iter()
            .find(|n| Identity::from(&n.site) == node.identity)
            .ok_or_else(|| bad("timed sequence occurrence"))?;
        let starts = c
            .connections
            .iter()
            .filter(|e| e.target.node == edge.source.node && server(e.delivery))
            .collect::<Vec<_>>();
        if starts.len() != 1
            || starts[0].source.node != *root
            || starts[0].source.event != STARTED
            || starts[0].target.event != input
        {
            return Err(bad("timed sequence must start only with its root"));
        }
        if !graph.input_ports(&source.site)?.is_empty() {
            return Err(bad("dynamic sequence timing"));
        }
        let f = &source.object.fields;
        let flag = |name| f.get(name).and_then(Value::as_bool);
        let number = |name| {
            f.get(name).and_then(|v| match v {
                Value::Float32(n) => Some(*n),
                _ => None,
            })
        };
        if !matches!(
            f.get("Realm").and_then(Value::as_enum_name),
            Some("Realm_Server" | "Realm_ClientAndServer")
        ) || flag("AutoStart") != Some(false)
        {
            return Err(bad("timed sequence realm or activation"));
        }
        let seconds = if node.class == "DelayEntityData" {
            if f.get("TimeDeltaType").and_then(Value::as_enum_name) != Some("TimeDeltaType_World") {
                return Err(bad("sequence delay clock"));
            }
            f64::from(number("Delay").ok_or_else(|| bad("sequence delay"))?)
        } else {
            if ["Looping", "AutoPlayFirstFrame", "PlayInReverse"]
                .iter()
                .any(|name| flag(name) != Some(false))
                || number("ExternalTime") != Some(0.)
                || number("PlaybackSpeed") != Some(1.)
                || f.get("SequenceStartTime").and_then(Value::bits32) != Some(0)
                || ["PropertyTracks", "CustomSequenceTracks"]
                    .iter()
                    .any(|name| {
                        f.get(name)
                            .and_then(Value::as_array)
                            .is_none_or(|a| !a.is_empty())
                    })
                || c.connections.iter().any(|e| {
                    e.source.node == edge.source.node
                        && server(e.delivery)
                        && e.source.event != FINISHED
                })
            {
                return Err(bad("unsupported sequence timeline"));
            }
            let millis = f
                .get("SequenceLength")
                .and_then(Value::bits32)
                .filter(|n| *n <= 600_000)
                .ok_or_else(|| bad("sequence length"))?;
            f64::from(millis as f32 * 0.001)
        };
        if !seconds.is_finite() || !(0. ..=600.).contains(&seconds) {
            return Err(bad("sequence timer bound"));
        }
        deadlines.push(seconds);
    }
    Ok(deadlines.into_iter().min_by(f64::total_cmp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::Memory;
    use nfs_world::scoring::events::{Connection, Endpoint, Remote};
    #[test]
    fn timeline_requires_root_start_server_completion_and_static_forward_clock() {
        let mut m = Memory::default();
        m.put(
            "a",
            0,
            "ParticipantStateSequenceBlueprint",
            &[("PropertyConnections", Value::Array(vec![]))],
        );
        m.put("a", 1, "ParticipantStateSequenceEntityData", &[]);
        m.put(
            "a",
            2,
            "SequenceEntityData",
            &[
                (
                    "Realm",
                    Value::Enum {
                        name: Some("Realm_ClientAndServer".into()),
                        value: 3,
                    },
                ),
                ("AutoStart", Value::Bool(false)),
                ("Looping", Value::Bool(false)),
                ("AutoPlayFirstFrame", Value::Bool(false)),
                ("PlayInReverse", Value::Bool(false)),
                ("ExternalTime", Value::Float32(0.)),
                ("PlaybackSpeed", Value::Float32(1.)),
                ("SequenceStartTime", Value::Int32(0)),
                ("SequenceLength", Value::Int32(2000)),
                ("PropertyTracks", Value::Array(vec![])),
                ("CustomSequenceTracks", Value::Array(vec![])),
            ],
        );
        let nodes = (1..=2)
            .map(|i| Node {
                site: Site {
                    key: ("a".into(), i),
                    ..Site::root("a")
                },
                object: m.0[&("a".into(), i)].clone(),
            })
            .collect::<Vec<_>>();
        let mut c = Composition {
            nodes: nodes
                .iter()
                .map(|n| crate::scoring::events::Node {
                    identity: Identity::from(&n.site),
                    class: n.object.class.clone(),
                    listeners: None,
                    forward: None,
                })
                .collect(),
            buses: vec![],
            bus_entities: vec![],
            properties: vec![],
            links: vec![],
            connections: vec![
                Connection {
                    source: Endpoint {
                        node: 0,
                        event: STARTED,
                    },
                    target: Endpoint {
                        node: 1,
                        event: PLAY,
                    },
                    delivery: Delivery::Local,
                },
                Connection {
                    source: Endpoint {
                        node: 1,
                        event: FINISHED,
                    },
                    target: Endpoint {
                        node: 0,
                        event: STOP,
                    },
                    delivery: Delivery::Local,
                },
            ],
        };
        assert_eq!(read(&mut Graph::new(&mut m), &nodes, &c).unwrap(), Some(2.));
        c.connections[1].delivery = Delivery::NetworkedClient(Remote { bus: 0, index: 1 });
        assert_eq!(read(&mut Graph::new(&mut m), &nodes, &c).unwrap(), None);
        c.connections[1].delivery = Delivery::Local;
        c.connections[0].source.event = 9;
        assert!(read(&mut Graph::new(&mut m), &nodes, &c).is_err());
        c.connections[0].source.event = STARTED;
        let mut looping = nodes.clone();
        looping[1]
            .object
            .fields
            .0
            .iter_mut()
            .find(|(k, _)| k == "Looping")
            .unwrap()
            .1 = Value::Bool(true);
        assert!(read(&mut Graph::new(&mut m), &looping, &c).is_err());
    }
}
