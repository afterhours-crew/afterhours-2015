// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed road network for the road-driving query (E916). The partition
//! `levels/genesis01/roadnetwork_centersplines/layer6_roadnetwork_schematic`
//! holds one `RoadNetworkEntityData` with `LaneSegment` objects: an integer
//! `SpeedLimit` (mph), a `LaneWidth`, a `LaneDirection` enum and a `Spline`
//! whose `SegmentPoints` sample the lane centre line. Lanes, nodes and the
//! authored grid are not needed by the owned measurements and are skipped.
use crate::{Error, assets::AssetIndex, graph::bad};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::scoring::road_network::{Direction, MAX_SEGMENTS, Network, Segment};

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

fn segment(object: &Object) -> Result<Segment, Error> {
    let fields = &object.fields;
    let speed_limit_mph = match fields.get("SpeedLimit") {
        Some(Value::Int32(v)) if (0..=300).contains(v) => *v as u16,
        _ => return Err(bad("road segment speed limit")),
    };
    let direction = match fields.get("LaneDirection") {
        Some(Value::Enum { name: Some(n), .. }) => match n.as_str() {
            "LaneDirection_Any" => Direction::Any,
            "LaneDirection_Forward" => Direction::Forward,
            "LaneDirection_Backward" => Direction::Backward,
            _ => return Err(bad("road segment direction")),
        },
        _ => return Err(bad("road segment direction")),
    };
    let spline = fields
        .get("Spline")
        .and_then(Value::as_fields)
        .ok_or_else(|| bad("road segment spline"))?;
    let points = spline
        .get("SegmentPoints")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("road segment points"))?
        .iter()
        .map(point)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Segment {
        speed_limit_mph,
        width: float(fields.get("LaneWidth"))?,
        direction,
        points,
    })
}

/// Every `LaneSegment` of a road network partition, in object order.
pub fn parse(objects: &[Object]) -> Result<Network, Error> {
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
    Network::new(segments).map_err(|e| bad(&format!("road network: {e}")))
}

impl AssetIndex {
    /// The installed road network of the open world.
    pub fn road_network(&mut self) -> Result<Network, Error> {
        let objects = self.assets.objects(PARTITION)?;
        parse(objects)
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
}
