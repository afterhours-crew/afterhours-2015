// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::road_graph::{Direction, LaneInput, tests::sample};

const MODULUS: u64 = (1 << 31) - 1;

#[test]
fn generator_matches_the_native_lehmer_sequence() {
    // Each native step is x * m mod (2^31 - 1); the pair multiplies by
    // 0x5e30 * 0x661f = 630,360,016.
    let mut random = NativeRandom::new(12345);
    assert_eq!(random.next(), 1_461_144_439);
    assert_eq!(random.next(), 1_646_755_962);
    assert_eq!(random.next(), 423_395_703);
    let mut x = 1u64;
    let mut random = NativeRandom::new(1);
    for _ in 0..10_000 {
        x = x * 630_360_016 % MODULUS;
        assert_eq!(random.next() as u64, x);
    }
    // A zero state is replaced by one, as in the native routine.
    assert_eq!(NativeRandom::new(0).next(), NativeRandom::new(1).next());
}

#[test]
fn generator_ranges_stay_inside_their_bounds() {
    let mut random = NativeRandom::new(7);
    for _ in 0..5_000 {
        let r = random.range(0, 100);
        assert!((0..=100).contains(&r));
        let u = random.unit();
        assert!(u > 0.0 && u <= 1.0);
        let v = random.uniform(100.0, 200.0);
        assert!((100.0..=200.0).contains(&v));
    }
    assert_eq!(random.range(3, 3), 3);
}

#[test]
fn junction_choice_prefers_the_straightest_exit() {
    let graph = sample();
    // Arriving at B heading +x: L2 (towards D) is straight, L1 turns, L0 is a U-turn.
    let mut random = NativeRandom::new(1);
    for _ in 0..50 {
        let exit = choose_exit(&graph, 1, [1.0, 0.0, 0.0], true, &mut random)
            .unwrap()
            .unwrap();
        assert_eq!(exit.lane, 2);
    }
}

#[test]
fn junction_choice_is_random_a_quarter_of_the_time_without_prefer_straight() {
    let graph = sample();
    let mut random = NativeRandom::new(99);
    let mut turns = 0;
    let mut uturns = 0;
    for _ in 0..4_000 {
        let exit = choose_exit(&graph, 1, [1.0, 0.0, 0.0], false, &mut random)
            .unwrap()
            .unwrap();
        match exit.lane {
            1 => turns += 1,
            0 => uturns += 1,
            _ => {}
        }
    }
    // Random picks (25/101) split between the straight and the turning exit;
    // the U-turn has dot -1 and is never in the random list.
    assert_eq!(uturns, 0);
    let share = turns as f32 / 4_000.0;
    assert!((0.09..0.16).contains(&share), "{share}");
    // Reproducible for a seed.
    let mut a = NativeRandom::new(5);
    let mut b = NativeRandom::new(5);
    for _ in 0..100 {
        assert_eq!(
            choose_exit(&graph, 1, [1.0, 0.0, 0.0], false, &mut a),
            choose_exit(&graph, 1, [1.0, 0.0, 0.0], false, &mut b)
        );
    }
}

/// A long straight road A-B-C-D with a junction to E at B.
fn long_road() -> Graph {
    let nodes = vec![
        [0.0, 0.0, 0.0],
        [500.0, 0.0, 0.0],
        [1_000.0, 0.0, 0.0],
        [1_500.0, 0.0, 0.0],
        [500.0, 0.0, 500.0],
    ];
    let lane = |start, end, a: [f32; 3], b: [f32; 3]| LaneInput {
        start,
        end,
        direction: Direction::Any,
        points: vec![a, b],
        widths: vec![10.0],
    };
    Graph::new(
        nodes.clone(),
        vec![
            lane(0, 1, nodes[0], nodes[1]),
            lane(1, 2, nodes[1], nodes[2]),
            lane(2, 3, nodes[2], nodes[3]),
            lane(1, 4, nodes[1], nodes[4]),
        ],
    )
    .unwrap()
}

#[test]
fn wandering_starts_on_the_nearest_road_in_the_heading_direction() {
    let graph = long_road();
    let flags = Flags::from_goal([false, true, true]);
    let east = Wander::start(
        &graph,
        [100.0, 0.0, 4.0],
        [1.0, 0.0, 0.0],
        flags,
        NativeRandom::new(3),
    )
    .unwrap()
    .unwrap();
    let pose = east.pose(&graph).unwrap();
    assert_eq!(pose.position, [100.0, 0.0, 0.0]);
    assert_eq!(pose.tangent, [1.0, 0.0, 0.0]);
    // Target 300-400 m ahead along the straight road.
    let target = east.target(&graph).unwrap().position;
    assert!((400.0..=500.0).contains(&target[0]), "{target:?}");
    let west = Wander::start(
        &graph,
        [100.0, 0.0, 4.0],
        [-1.0, 0.0, 0.0],
        flags,
        NativeRandom::new(3),
    )
    .unwrap()
    .unwrap();
    assert_eq!(west.pose(&graph).unwrap().tangent, [-1.0, 0.0, 0.0]);
    assert!(
        Wander::start(
            &graph,
            [100.0, 0.0, 900.0],
            [1.0, 0.0, 0.0],
            flags,
            NativeRandom::new(3)
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn wandering_moves_at_the_native_speed_through_junctions() {
    let graph = long_road();
    // Prefer straight: through B onto B-C, then C-D.
    let flags = Flags::from_goal([false, true, true]);
    let mut wander = Wander::start(
        &graph,
        [400.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        flags,
        NativeRandom::new(11),
    )
    .unwrap()
    .unwrap();
    wander.advance(&graph, 10.0).unwrap();
    let pose = wander.pose(&graph).unwrap();
    assert!(
        (pose.position[0] - (400.0 + WANDER_SPEED * 10.0)).abs() < 1e-2,
        "{pose:?}"
    );
    assert_eq!(wander.edge().lane, 1);
    // Many seconds later it has travelled the whole road and turned back from
    // the dead end at D rather than stopping there for good.
    let mut moved = Vec::new();
    for _ in 0..200 {
        let before = wander.pose(&graph).unwrap().position;
        wander.advance(&graph, 1.0).unwrap();
        let after = wander.pose(&graph).unwrap().position;
        moved.push(((after[0] - before[0]).powi(2) + (after[2] - before[2]).powi(2)).sqrt());
    }
    let stuck = moved.iter().filter(|m| **m < 1e-3).count();
    assert!(stuck < 5, "stuck {stuck} of {}", moved.len());
    assert!(wander.advance(&graph, f32::NAN).is_err());
    assert!(wander.advance(&graph, -1.0).is_err());
}

#[test]
fn relocation_keeps_the_generator_and_flags() {
    let graph = long_road();
    let flags = Flags::from_goal([false, false, true]);
    let wander = Wander::start(
        &graph,
        [100.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        flags,
        NativeRandom::new(4),
    )
    .unwrap()
    .unwrap();
    let moved = wander
        .relocate(&graph, [500.0, 0.0, 250.0], [0.0, 0.0, 1.0])
        .unwrap()
        .unwrap();
    assert_eq!(moved.edge().lane, 3);
    assert_eq!(moved.pose(&graph).unwrap().tangent, [0.0, 0.0, 1.0]);
    assert_eq!(moved.flags, wander.flags);
}
