// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

/// A(0,0) -L0 any- B(100,0) -L1 forward-> C(100,100); B -L2 any- D(200,0);
/// D -L3 backward- C (drivable only C -> D).
pub(crate) fn sample() -> Graph {
    let nodes = vec![
        [0.0, 0.0, 0.0],
        [100.0, 0.0, 0.0],
        [100.0, 0.0, 100.0],
        [200.0, 0.0, 0.0],
    ];
    let lane = |start, end, direction, points: Vec<[f32; 3]>| LaneInput {
        start,
        end,
        direction,
        widths: vec![10.0; points.len() - 1],
        points,
    };
    Graph::new(
        nodes,
        vec![
            lane(
                0,
                1,
                Direction::Any,
                vec![[0.0, 0.0, 0.0], [50.0, 0.0, 0.0], [100.0, 0.0, 0.0]],
            ),
            lane(
                1,
                2,
                Direction::Forward,
                vec![[100.0, 0.0, 0.0], [100.0, 0.0, 100.0]],
            ),
            lane(
                1,
                3,
                Direction::Any,
                vec![[100.0, 0.0, 0.0], [200.0, 0.0, 0.0]],
            ),
            lane(
                3,
                2,
                Direction::Backward,
                vec![[200.0, 0.0, 0.0], [150.0, 0.0, 50.0], [100.0, 0.0, 100.0]],
            ),
        ],
    )
    .unwrap()
}

#[test]
fn poses_interpolate_along_and_against_the_lane() {
    let graph = sample();
    assert_eq!(graph.length(0).unwrap(), 100.0);
    let forward = graph
        .pose(
            Edge {
                lane: 0,
                forward: true,
            },
            25.0,
        )
        .unwrap();
    assert_eq!(forward.position, [25.0, 0.0, 0.0]);
    assert_eq!(forward.tangent, [1.0, 0.0, 0.0]);
    let backward = graph
        .pose(
            Edge {
                lane: 0,
                forward: false,
            },
            25.0,
        )
        .unwrap();
    assert_eq!(backward.position, [75.0, 0.0, 0.0]);
    assert_eq!(backward.tangent, [-1.0, 0.0, 0.0]);
    // Clamped at both ends.
    let end = Edge {
        lane: 0,
        forward: true,
    };
    assert_eq!(graph.pose(end, 500.0).unwrap().position, [100.0, 0.0, 0.0]);
    assert_eq!(graph.pose(end, -5.0).unwrap().position, [0.0, 0.0, 0.0]);
    assert_eq!(graph.pose(end, f32::NAN), Err(Error::NonFinite));
}

#[test]
fn exits_respect_authored_lane_directions() {
    let graph = sample();
    let lanes = |node| {
        let mut e: Vec<_> = graph
            .exits(node)
            .unwrap()
            .into_iter()
            .map(|(e, _)| (e.lane, e.forward))
            .collect();
        e.sort();
        e
    };
    // B: back along L0, along forward-only L1, along L2.
    assert_eq!(lanes(1), vec![(0, false), (1, true), (2, true)]);
    // C: L1 cannot be driven backwards; backward-only L3 leaves C towards D.
    assert_eq!(lanes(2), vec![(3, false)]);
    // D: L2 back towards B; L3 cannot be driven forwards.
    assert_eq!(lanes(3), vec![(2, false)]);
    let (_, tangent) = graph
        .exits(1)
        .unwrap()
        .into_iter()
        .find(|(e, _)| e.lane == 1)
        .unwrap();
    assert_eq!(tangent, [0.0, 0.0, 1.0]);
    assert_eq!(
        graph.head(Edge {
            lane: 3,
            forward: false
        }),
        Ok(3)
    );
}

#[test]
fn nearest_road_point_is_found_within_the_search_radius() {
    let graph = sample();
    let near = graph.nearest([150.0, 4.0, 3.0]).unwrap().unwrap();
    assert_eq!(near.lane, 2);
    assert!((near.offset - 50.0).abs() < 1e-3);
    assert!((near.distance - 5.0).abs() < 1e-3);
    assert_eq!(near.point, [150.0, 0.0, 0.0]);
    assert!(graph.nearest([5_000.0, 0.0, 5_000.0]).unwrap().is_none());
    assert_eq!(
        graph.nearest([f32::INFINITY, 0.0, 0.0]),
        Err(Error::NonFinite)
    );
}

#[test]
fn shortest_paths_follow_allowed_directions() {
    let graph = sample();
    let path = graph.shortest_path(0, 2).unwrap().unwrap();
    assert_eq!(
        path,
        vec![
            Edge {
                lane: 0,
                forward: true
            },
            Edge {
                lane: 1,
                forward: true
            }
        ]
    );
    // From C the only way out is backward-only L3 to D, then L2 and L0.
    let back = graph.shortest_path(2, 0).unwrap().unwrap();
    assert_eq!(
        back.iter().map(|e| e.lane).collect::<Vec<_>>(),
        vec![3, 2, 0]
    );
    assert_eq!(graph.shortest_path(1, 1).unwrap(), Some(Vec::new()));
    assert_eq!(graph.shortest_path(0, 9), Err(Error::Shape));
}

#[test]
fn unreachable_targets_are_none() {
    let graph = Graph::new(
        vec![[0.0; 3], [10.0, 0.0, 0.0]],
        vec![LaneInput {
            start: 0,
            end: 1,
            direction: Direction::Forward,
            points: vec![[0.0; 3], [10.0, 0.0, 0.0]],
            widths: vec![10.0],
        }],
    )
    .unwrap();
    assert_eq!(graph.shortest_path(1, 0).unwrap(), None);
    assert!(graph.shortest_path(0, 1).unwrap().is_some());
}

#[test]
fn invalid_graphs_are_rejected() {
    let lane = |start, end, points: Vec<[f32; 3]>| LaneInput {
        start,
        end,
        direction: Direction::Any,
        widths: vec![10.0; points.len().saturating_sub(1)],
        points,
    };
    assert_eq!(Graph::new(vec![], vec![]).unwrap_err(), Error::Bound);
    assert_eq!(
        Graph::new(vec![[0.0; 3]], vec![lane(0, 1, vec![[0.0; 3], [1.0; 3]])]).unwrap_err(),
        Error::Shape
    );
    assert_eq!(
        Graph::new(vec![[0.0; 3]], vec![lane(0, 0, vec![[0.0; 3], [0.0; 3]])]).unwrap_err(),
        Error::Shape
    );
    assert_eq!(
        Graph::new(
            vec![[0.0; 3]],
            vec![lane(0, 0, vec![[0.0; 3], [f32::NAN, 0.0, 0.0]])]
        )
        .unwrap_err(),
        Error::NonFinite
    );
    assert_eq!(
        Graph::new(vec![[0.0; 3]], vec![lane(0, 0, vec![[0.0; 3]])]).unwrap_err(),
        Error::Bound
    );
    let crowded: Vec<_> = (0..=MAX_NODE_LANES)
        .map(|i| lane(0, 0, vec![[0.0; 3], [1.0 + i as f32, 0.0, 0.0]]))
        .collect();
    assert_eq!(
        Graph::new(vec![[0.0; 3]], crowded).unwrap_err(),
        Error::Bound
    );
}

#[test]
fn nearest_road_point_keeps_the_level_of_a_bridge() {
    // A bridge 10 m above a road along the same line.
    let graph = Graph::new(
        vec![
            [0.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [0.0, 10.0, 0.0],
            [100.0, 10.0, 0.0],
        ],
        vec![
            LaneInput {
                start: 0,
                end: 1,
                direction: Direction::Any,
                points: vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0]],
                widths: vec![10.0],
            },
            LaneInput {
                start: 2,
                end: 3,
                direction: Direction::Any,
                points: vec![[0.0, 10.0, 0.0], [100.0, 10.0, 0.0]],
                widths: vec![10.0],
            },
        ],
    )
    .unwrap();
    for (height, lane) in [(0.4, 0), (4.9, 0), (5.1, 1), (10.4, 1)] {
        let near = graph.nearest([50.0, height, 2.0]).unwrap().unwrap();
        assert_eq!(near.lane, lane, "{height}");
    }
    // A slope is measured along the road, not against its horizontal shadow.
    let ramp = Graph::new(
        vec![[0.0, 0.0, 0.0], [100.0, 100.0, 0.0]],
        vec![LaneInput {
            start: 0,
            end: 1,
            direction: Direction::Any,
            points: vec![[0.0, 0.0, 0.0], [100.0, 100.0, 0.0]],
            widths: vec![10.0],
        }],
    )
    .unwrap();
    let near = ramp.nearest([50.0, 51.0, 0.0]).unwrap().unwrap();
    assert!((near.point[0] - 50.5).abs() < 1e-3 && (near.point[1] - 50.5).abs() < 1e-3);
}
