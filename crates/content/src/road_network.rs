// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed road network for the road-driving query (E916). The partition
//! `levels/genesis01/roadnetwork_centersplines/layer6_roadnetwork_schematic`
//! holds one `RoadNetworkEntityData` with `LaneSegment` objects: an integer
//! `SpeedLimit` (mph), a `LaneWidth`, a `LaneDirection` enum and a `Spline`
//! whose `SegmentPoints` sample the lane centre line. The owned measurements
//! use segments only; server-side AI movement also reads the junction graph
//! (E987): `RoadNode` positions and `Lane` edges listing their segments.
use crate::{Error, assets::AssetIndex, graph::bad};
use nfs_frostbite::ebx::{Object, Pointer, Value};
use nfs_world::road_graph::{Graph, LaneInput};
use nfs_world::scoring::road_network::{Direction, MAX_SEGMENTS, Network, Segment};
use std::collections::BTreeMap;

mod distance;
pub use distance::parse as parse_distance;

pub const PARTITION: &str =
    "levels/genesis01/roadnetwork_centersplines/layer6_roadnetwork_schematic";

fn float(value: Option<&Value>) -> Result<f32, Error> {
    match value {
        Some(Value::Float32(v)) if v.is_finite() => Ok(*v),
        _ => Err(bad("road segment float")),
    }
}

fn point(value: &Value) -> Result<[f32; 3], Error> {
    let fields = value.as_fields().ok_or_else(|| bad("road point"))?;
    Ok([
        float(fields.get("x"))?,
        float(fields.get("y"))?,
        float(fields.get("z"))?,
    ])
}

fn direction(value: Option<&Value>) -> Result<Direction, Error> {
    match value {
        Some(Value::Enum { name: Some(n), .. }) => match n.as_str() {
            "LaneDirection_Any" => Ok(Direction::Any),
            "LaneDirection_Forward" => Ok(Direction::Forward),
            "LaneDirection_Backward" => Ok(Direction::Backward),
            _ => Err(bad("road lane direction")),
        },
        _ => Err(bad("road lane direction")),
    }
}

fn internal(value: Option<&Value>, what: &str) -> Result<usize, Error> {
    match value {
        Some(Value::Pointer(Pointer::Internal(index))) => Ok(*index),
        _ => Err(bad(what)),
    }
}

fn spline_points(object: &Object) -> Result<Vec<[f32; 3]>, Error> {
    object
        .fields
        .get("Spline")
        .and_then(Value::as_fields)
        .and_then(|s| s.get("SegmentPoints"))
        .and_then(Value::as_array)
        .ok_or_else(|| bad("road segment points"))?
        .iter()
        .map(point)
        .collect()
}

fn segment(object: &Object) -> Result<Segment, Error> {
    let fields = &object.fields;
    let speed_limit_mph = match fields.get("SpeedLimit") {
        Some(Value::Int32(v)) if (0..=300).contains(v) => *v as u16,
        _ => return Err(bad("road segment speed limit")),
    };
    let direction = direction(fields.get("LaneDirection"))?;
    let points = spline_points(object)?;
    Ok(Segment {
        speed_limit_mph,
        width: float(fields.get("LaneWidth"))?,
        direction,
        points,
    })
}

/// Every `LaneSegment` of a road network partition, in object order.
pub fn parse(objects: &[Object]) -> Result<Network, Error> {
    Network::new(parse_segments(objects)?).map_err(|e| bad(&format!("road network: {e}")))
}

/// Static constructor inputs, in authored order. Consumers must use `Network::new`.
pub fn parse_segments(objects: &[Object]) -> Result<Vec<Segment>, Error> {
    let mut segments = Vec::new();
    for object in objects.iter().filter(|o| o.class == "LaneSegment") {
        if segments.len() >= MAX_SEGMENTS {
            return Err(bad("road segment count"));
        }
        segments.push(segment(object)?);
    }
    if !objects.iter().any(|o| o.class == "RoadNetworkEntityData") {
        return Err(bad("road network entity missing"));
    }
    Ok(segments)
}

/// Static junction graph inputs. Derived spatial grids and live AI state are
/// excluded; `Graph::new` remains the validation/instantiation boundary.
pub struct GraphInput {
    pub nodes: Vec<[f32; 3]>,
    pub lanes: Vec<LaneInput>,
}

fn squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// The junction graph of a road network partition: every `RoadNode` and every
/// `Lane` with its segments' points joined in order. Four installed lanes list
/// their segments from the end node; those are reversed to run start to end.
pub fn parse_graph(objects: &[Object]) -> Result<Graph, Error> {
    let input = parse_graph_input(objects)?;
    Graph::new(input.nodes, input.lanes).map_err(|e| bad(&format!("road graph: {e}")))
}

pub fn parse_graph_input(objects: &[Object]) -> Result<GraphInput, Error> {
    let mut node_of = BTreeMap::new();
    let mut nodes = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        if object.class == "RoadNode" {
            let position = object
                .fields
                .get("Position")
                .ok_or_else(|| bad("road node position"))?;
            node_of.insert(index, nodes.len());
            nodes.push(point(position)?);
        }
    }
    let mut lanes = Vec::new();
    for object in objects.iter().filter(|o| o.class == "Lane") {
        let fields = &object.fields;
        let node = |name: &str| -> Result<usize, Error> {
            let target = internal(fields.get(name), "road lane node")?;
            node_of
                .get(&target)
                .copied()
                .ok_or_else(|| bad("road lane node"))
        };
        let (start, end) = (node("Start")?, node("End")?);
        let direction = direction(fields.get("LaneDirection"))?;
        let mut points: Vec<[f32; 3]> = Vec::new();
        // Width of each interval: the segment that contributes its end point.
        let mut widths: Vec<f32> = Vec::new();
        for entry in fields
            .get("Segments")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("road lane segments"))?
        {
            let index = internal(Some(entry), "road lane segment")?;
            let segment = objects
                .get(index)
                .filter(|o| o.class == "LaneSegment")
                .ok_or_else(|| bad("road lane segment"))?;
            let width = float(segment.fields.get("LaneWidth"))?;
            for p in spline_points(segment)? {
                if points.last() != Some(&p) {
                    if !points.is_empty() {
                        widths.push(width);
                    }
                    points.push(p);
                }
            }
        }
        let (a, b) = (nodes[start], nodes[end]);
        let (first, last) = match (points.first(), points.last()) {
            (Some(f), Some(l)) => (*f, *l),
            _ => return Err(bad("road lane segments")),
        };
        if squared(first, b) + squared(last, a) < squared(first, a) + squared(last, b) {
            points.reverse();
            widths.reverse();
        }
        lanes.push(LaneInput {
            start,
            end,
            direction,
            points,
            widths,
        });
    }
    Ok(GraphInput { nodes, lanes })
}

impl AssetIndex {
    pub fn road_graph_input(&mut self) -> Result<GraphInput, Error> {
        parse_graph_input(self.assets.objects(PARTITION)?)
    }

    pub fn road_segments(&mut self) -> Result<Vec<Segment>, Error> {
        parse_segments(self.assets.objects(PARTITION)?)
    }

    /// The installed road network of the open world.
    pub fn road_network(&mut self) -> Result<Network, Error> {
        let objects = self.assets.objects(PARTITION)?;
        parse(objects)
    }

    /// The installed junction graph used for server-side AI movement.
    pub fn road_graph(&mut self) -> Result<Graph, Error> {
        let objects = self.assets.objects(PARTITION)?;
        parse_graph(objects)
    }

    /// Direction-independent authored road distances for race pacing and ranks.
    pub fn road_distance(&mut self) -> Result<nfs_world::road_distance::Network, Error> {
        let objects = self.assets.objects(PARTITION)?;
        parse_distance(objects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_frostbite::ebx::Fields;

    fn value_point(x: f32, y: f32, z: f32) -> Value {
        Value::Struct(Fields(vec![
            ("x".into(), Value::Float32(x)),
            ("y".into(), Value::Float32(y)),
            ("z".into(), Value::Float32(z)),
        ]))
    }
    fn lane_segment(limit: i32, direction: &str, points: Vec<Value>) -> Object {
        Object {
            class: "LaneSegment".into(),
            guid: None,
            fields: Fields(vec![
                ("ID".into(), Value::Int32(7)),
                ("SpeedLimit".into(), Value::Int32(limit)),
                (
                    "Spline".into(),
                    Value::Struct(Fields(vec![("SegmentPoints".into(), Value::Array(points))])),
                ),
                ("LaneWidth".into(), Value::Float32(12.0)),
                (
                    "LaneDirection".into(),
                    Value::Enum {
                        value: 0,
                        name: Some(direction.into()),
                    },
                ),
            ]),
        }
    }
    fn network_entity() -> Object {
        Object {
            class: "RoadNetworkEntityData".into(),
            guid: None,
            fields: Fields(vec![]),
        }
    }

    #[test]
    fn lane_segments_become_network_segments_and_bad_fields_fail() {
        let objects = vec![
            network_entity(),
            lane_segment(
                55,
                "LaneDirection_Forward",
                vec![value_point(0.0, 1.0, 0.0), value_point(0.0, 1.0, 30.0)],
            ),
            lane_segment(
                35,
                "LaneDirection_Any",
                vec![value_point(50.0, 1.0, 0.0), value_point(50.0, 1.0, 30.0)],
            ),
            Object {
                class: "Lane".into(),
                guid: None,
                fields: Fields(vec![]),
            },
        ];
        let network = parse(&objects).unwrap();
        assert_eq!(network.segments().len(), 2);
        assert_eq!(network.segments()[0].speed_limit_mph, 55);
        assert_eq!(network.segments()[1].direction, Direction::Any);
        let sample = network
            .sample([1.0, 1.0, 15.0], [0.0, 0.0, 10.0])
            .unwrap()
            .unwrap();
        assert_eq!(sample.speed_limit, 55.0);

        assert!(parse(&objects[1..]).is_err());
        let mut bad_limit = objects.clone();
        bad_limit[1].fields.0[1].1 = Value::Int32(-1);
        assert!(parse(&bad_limit).is_err());
        let mut bad_direction = objects.clone();
        bad_direction[1].fields.0[4].1 = Value::Enum {
            value: 9,
            name: Some("LaneDirection_Sideways".into()),
        };
        assert!(parse(&bad_direction).is_err());
        let mut one_point = objects.clone();
        one_point[1].fields.0[2].1 = Value::Struct(Fields(vec![(
            "SegmentPoints".into(),
            Value::Array(vec![value_point(0.0, 0.0, 0.0)]),
        )]));
        assert!(parse(&one_point).is_err());
    }

    fn road_node(x: f32, z: f32) -> Object {
        Object {
            class: "RoadNode".into(),
            guid: None,
            fields: Fields(vec![
                ("ID".into(), Value::Int32(1)),
                ("Position".into(), value_point(x, 0.0, z)),
            ]),
        }
    }

    fn lane(start: usize, end: usize, direction: &str, segments: &[usize]) -> Object {
        let pointer = |i| Value::Pointer(Pointer::Internal(i));
        Object {
            class: "Lane".into(),
            guid: None,
            fields: Fields(vec![
                (
                    "Segments".into(),
                    Value::Array(segments.iter().map(|i| pointer(*i)).collect()),
                ),
                ("Start".into(), pointer(start)),
                ("End".into(), pointer(end)),
                (
                    "LaneDirection".into(),
                    Value::Enum {
                        value: 0,
                        name: Some(direction.into()),
                    },
                ),
            ]),
        }
    }

    #[test]
    fn lanes_join_their_segments_between_nodes() {
        let mut objects = vec![
            network_entity(),
            road_node(0.0, 0.0),
            road_node(0.0, 60.0),
            lane_segment(
                30,
                "LaneDirection_Any",
                vec![value_point(0.0, 0.0, 0.0), value_point(0.0, 0.0, 30.0)],
            ),
            lane_segment(
                30,
                "LaneDirection_Any",
                vec![value_point(0.0, 0.0, 30.0), value_point(0.0, 0.0, 60.0)],
            ),
            lane(1, 2, "LaneDirection_Forward", &[3, 4]),
            // Authored from the end node: reversed to run start to end.
            lane(2, 1, "LaneDirection_Any", &[3, 4]),
        ];
        for (name, value) in &mut objects[4].fields.0 {
            if name == "LaneWidth" {
                *value = Value::Float32(20.0);
            }
        }
        let graph = parse_graph(&objects).unwrap();
        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.lane_count(), 2);
        // The shared joint point is not duplicated.
        assert_eq!(graph.length(0).unwrap(), 60.0);
        let edge = |lane, forward| nfs_world::road_graph::Edge { lane, forward };
        assert!(graph.allowed(edge(0, true)).unwrap());
        assert!(!graph.allowed(edge(0, false)).unwrap());
        assert_eq!(
            graph.pose(edge(0, true), 0.0).unwrap().position,
            [0.0, 0.0, 0.0]
        );
        assert_eq!(
            graph.pose(edge(1, true), 0.0).unwrap().position,
            [0.0, 0.0, 60.0]
        );
        assert_eq!(graph.head(edge(1, true)), Ok(0));
        // Each interval keeps its own segment's road width, also when the
        // lane is reversed.
        let width = |e, at| graph.pose(e, at).unwrap().width;
        assert_eq!(
            (width(edge(0, true), 10.0), width(edge(0, true), 50.0)),
            (12.0, 20.0)
        );
        assert_eq!(
            (width(edge(1, true), 10.0), width(edge(1, true), 50.0)),
            (20.0, 12.0)
        );
        assert_eq!(width(edge(0, false), 10.0), 20.0);

        let mut dangling = objects.clone();
        dangling[5] = lane(1, 3, "LaneDirection_Any", &[3]);
        assert!(parse_graph(&dangling).is_err());
        let mut not_segment = objects.clone();
        not_segment[5] = lane(1, 2, "LaneDirection_Any", &[2]);
        assert!(parse_graph(&not_segment).is_err());
        let mut no_segments = objects.clone();
        no_segments[5] = lane(1, 2, "LaneDirection_Any", &[]);
        assert!(parse_graph(&no_segments).is_err());
        let mut bad_direction = objects;
        bad_direction[5] = lane(1, 2, "LaneDirection_Up", &[3]);
        assert!(parse_graph(&bad_direction).is_err());
    }

    #[test]
    fn distance_query_reads_authored_tables_grid_and_parent_links() {
        let pointer = |i| Value::Pointer(Pointer::Internal(i));
        let mut objects = vec![
            network_entity(),
            road_node(0., 0.),
            road_node(0., 60.),
            lane_segment(
                30,
                "LaneDirection_Forward",
                vec![value_point(0., 0., 0.), value_point(0., 0., 60.)],
            ),
            lane(1, 2, "LaneDirection_Forward", &[3]),
        ];
        let matrix = Value::Struct(Fields(
            (1..=4)
                .flat_map(|row| {
                    (1..=4).map(move |column| {
                        (
                            format!("m{row}{column}"),
                            Value::Float32(if row == 2 && column == 3 { 60. } else { 0. }),
                        )
                    })
                })
                .collect(),
        ));
        let Value::Struct(spline) = &mut objects[3].fields.0[2].1 else {
            unreachable!()
        };
        spline.0.extend([
            (
                "MatrixFormData".into(),
                Value::Struct(Fields(vec![
                    ("Matrix".into(), matrix),
                    (
                        "BaseData".into(),
                        Value::Struct(Fields(
                            [0., 0., 60., 60.]
                                .into_iter()
                                .enumerate()
                                .map(|(i, z)| (format!("P{i}"), value_point(0., 0., z)))
                                .collect(),
                        )),
                    ),
                ])),
            ),
            (
                "SegmentEndTime".into(),
                Value::Array(vec![Value::Float32(1.)]),
            ),
            (
                "SegmentEndDistance".into(),
                Value::Array(vec![Value::Float32(60.)]),
            ),
        ]);
        objects[3].fields.0.push(("Lane".into(), pointer(4)));
        objects[4].fields.0.push((
            "LaneLayer".into(),
            Value::Enum {
                value: 1,
                name: Some("LaneLayer_Roads".into()),
            },
        ));
        let cell = |refs| Value::Struct(Fields(vec![("LaneSegments".into(), Value::Array(refs))]));
        let mut grid = vec![vec![cell(vec![]); 200]; 200];
        grid[100][100] = cell(vec![pointer(3)]);
        objects[0].fields.0.extend([
            (
                "RoadNodes".into(),
                Value::Array(vec![pointer(1), pointer(2)]),
            ),
            ("RoadLanes".into(), Value::Array(vec![pointer(4)])),
            (
                "Grid".into(),
                Value::Struct(Fields(vec![(
                    "X".into(),
                    Value::Array(
                        grid.into_iter()
                            .map(|column| {
                                Value::Struct(Fields(vec![("Z".into(), Value::Array(column))]))
                            })
                            .collect(),
                    ),
                )])),
            ),
        ]);
        let query = parse_distance(&objects).unwrap();
        let a = query.locate([0., 0., 5.]).unwrap().unwrap();
        let b = query.locate([0., 0., 50.]).unwrap().unwrap();
        let target = query.prepare_target(a).unwrap();
        // Distances may run backwards on an authored forward-only driving lane.
        assert_eq!(query.distance(b, &target).unwrap(), Some(45.));
        objects[3].fields.0.last_mut().unwrap().1 = pointer(1);
        assert!(parse_distance(&objects).is_err());
        objects[3].fields.0.last_mut().unwrap().1 = pointer(4);
        objects[0].fields.0[1].1 = Value::Array(vec![pointer(3)]);
        assert!(parse_distance(&objects).is_err());
    }
}
