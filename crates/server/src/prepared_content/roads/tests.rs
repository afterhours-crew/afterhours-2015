// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::road_network::GraphInput;
use nfs_world::road_graph::{Direction, Edge, LaneInput};
use nfs_world::scoring::road_network::Segment;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "afterhours-roads-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn cache(&self) -> Cache {
        Cache {
            root: self.0.clone(),
            installation: "a".repeat(64),
            executable: "b".repeat(64),
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn input() -> GraphInput {
    GraphInput {
        nodes: vec![[0.0, 0.0, 0.0], [20.0, 0.0, 0.0]],
        lanes: vec![LaneInput {
            start: 0,
            end: 1,
            direction: Direction::Forward,
            points: vec![[0.0, 0.0, 0.0], [20.0, 0.0, 0.0]],
            widths: vec![4.5],
        }],
    }
}
fn segments() -> Vec<Segment> {
    vec![Segment {
        speed_limit_mph: 40,
        width: 4.5,
        direction: Direction::Forward,
        points: vec![[0.0, 0.0, 0.0], [20.0, 0.0, 0.0]],
    }]
}

#[test]
fn persisted_roads_keep_direction_routes_and_scoring_without_recompilation() {
    let dir = Directory::new();
    let raw = input();
    let graph = Graph::new(raw.nodes.clone(), raw.lanes.clone()).unwrap();
    let cold = dir
        .cache()
        .prepare("rival-road-graph", b"", decode_graph, || {
            Ok(codec::encode_graph(&raw).unwrap())
        })
        .unwrap();
    let warm = dir
        .cache()
        .prepare("rival-road-graph", b"", decode_graph, || {
            panic!("warm graph compiled")
        })
        .unwrap();
    for actual in [cold, warm] {
        assert_eq!(
            actual.shortest_path(0, 1).unwrap(),
            graph.shortest_path(0, 1).unwrap()
        );
        assert_eq!(actual.shortest_path(1, 0).unwrap(), None);
        let edge = Edge {
            lane: 0,
            forward: true,
        };
        assert_eq!(
            actual.pose(edge, 7.5).unwrap(),
            graph.pose(edge, 7.5).unwrap()
        );
        assert_eq!(
            actual.nearest([4.0, 0.0, 1.0]).unwrap(),
            graph.nearest([4.0, 0.0, 1.0]).unwrap()
        );
    }
    let expected = Network::new(segments()).unwrap();
    dir.cache()
        .prepare("scoring-road-network", b"", decode_network, || {
            Ok(codec::encode_network(&segments()).unwrap())
        })
        .unwrap();
    let warm = dir
        .cache()
        .prepare("scoring-road-network", b"", decode_network, || {
            panic!("warm network compiled")
        })
        .unwrap();
    for velocity in [[10.0, 0.0, 0.0], [-10.0, 0.0, 0.0]] {
        assert_eq!(
            warm.sample([5.0, 0.0, 1.0], velocity).unwrap(),
            expected.sample([5.0, 0.0, 1.0], velocity).unwrap()
        );
    }
}

#[test]
fn malformed_road_topology_and_geometry_never_publish() {
    let dir = Directory::new();
    let mut bad = input();
    bad.lanes[0].end = 8;
    assert!(
        dir.cache()
            .prepare("rival-road-graph", b"", decode_graph, || Ok(
                codec::encode_graph(&bad).unwrap()
            ))
            .is_err()
    );
    let rebuilt = dir
        .cache()
        .prepare("rival-road-graph", b"", decode_graph, || {
            Ok(codec::encode_graph(&input()).unwrap())
        })
        .unwrap();
    assert_eq!(rebuilt.node_count(), 2);
    for kind in 0..4 {
        let mut bad = segments();
        match kind {
            0 => bad[0].width = -1.0,
            1 => bad[0].points.truncate(1),
            2 => bad[0].speed_limit_mph = 301,
            _ => {
                bad[0].points[0][0] = -f32::MAX;
                bad[0].points[1][0] = f32::MAX;
            }
        }
        assert!(decode_network(&codec::encode_network(&bad).unwrap()).is_err());
    }
    let mut huge = input();
    huge.nodes = vec![
        [f32::MAX, 0.0, 0.0],
        [f32::MAX, 10.0, 0.0],
        [-f32::MAX, 0.0, 0.0],
        [-f32::MAX, 10.0, 0.0],
    ];
    huge.lanes = (0..2)
        .map(|i| LaneInput {
            start: i * 2,
            end: i * 2 + 1,
            direction: Direction::Any,
            points: huge.nodes[i * 2..i * 2 + 2].to_vec(),
            widths: vec![4.5],
        })
        .collect();
    assert!(decode_graph(&codec::encode_graph(&huge).unwrap()).is_err());
}

#[test]
fn road_cache_frames_reject_truncation_trailing_data_counts_and_invalid_values() {
    let graph = codec::encode_graph(&input()).unwrap();
    let network = codec::encode_network(&segments()).unwrap();
    for (bytes, graph_kind) in [(&graph, true), (&network, false)] {
        let accepted = |bytes: &[u8]| {
            if graph_kind {
                decode_graph(bytes).is_ok()
            } else {
                decode_network(bytes).is_ok()
            }
        };
        assert!(accepted(bytes));
        for end in 0..bytes.len() {
            assert!(!accepted(&bytes[..end]), "prefix {end}");
        }
        let mut trailing = bytes.clone();
        trailing.extend_from_slice(bytes);
        assert!(!accepted(&trailing));
        let mut wrong_version = bytes.clone();
        wrong_version[3] = b'2';
        assert!(!accepted(&wrong_version));
        let mut oversized = bytes.clone();
        oversized[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!accepted(&oversized));
        // Exercise each single-byte corruption against constructor validation.
        // Some values remain valid models; none may panic or allocate unboundedly.
        for at in 0..bytes.len() {
            let mut mutation = bytes.clone();
            mutation[at] = 255;
            let _ = accepted(&mutation);
        }
    }
    let mut direction = network.clone();
    direction[14] = 3; // magic + segment count + speed + width
    assert!(decode_network(&direction).is_err());
    let mut nonfinite = network;
    nonfinite[10..14].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
    assert!(decode_network(&nonfinite).is_err());
    let mut signed_zero = segments();
    signed_zero[0].points[0][2] = -0.0;
    let decoded = codec::decode_network(&codec::encode_network(&signed_zero).unwrap()).unwrap();
    assert_eq!(decoded[0].points[0][2].to_bits(), (-0.0_f32).to_bits());
}
