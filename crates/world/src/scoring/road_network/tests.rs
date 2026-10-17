// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn straight(direction: Direction, limit: u16, z0: f32, z1: f32, x: f32) -> Segment {
    Segment {
        speed_limit_mph: limit,
        width: 10.0,
        direction,
        points: vec![[x, 100.0, z0], [x, 100.0, (z0 + z1) / 2.0], [x, 100.0, z1]],
    }
}

#[test]
fn nearest_segment_lateral_sign_and_limits() {
    // A forward lane along +Z at x = 0 and an Any lane along +Z at x = 50.
    let network = Network::new(vec![
        straight(Direction::Forward, 55, 0.0, 100.0, 0.0),
        straight(Direction::Any, 35, 0.0, 100.0, 50.0),
    ])
    .unwrap();
    // Right of the +Z tangent (Y up) is +X: cross(up, tangent) = (+1, 0, 0).
    let sample = network
        .sample([2.0, 100.0, 40.0], [0.0, 0.0, 20.0])
        .unwrap()
        .unwrap();
    assert_eq!(sample.speed_limit, 55.0);
    assert!((sample.distance_to_segment - 2.0).abs() < 1e-4);
    assert!((sample.speed_metres_per_second - 20.0).abs() < 1e-5);
    assert!(!sample.wrong_direction);
    let left = network
        .sample([-3.0, 100.0, 40.0], [0.0, 0.0, 20.0])
        .unwrap()
        .unwrap();
    assert!((left.distance_to_segment + 3.0).abs() < 1e-4);
    // Nearer to the second lane: its limit, never wrong way on an Any lane.
    let any = network
        .sample([47.0, 100.0, 10.0], [0.0, 0.0, -20.0])
        .unwrap()
        .unwrap();
    assert_eq!(any.speed_limit, 35.0);
    assert!(!any.wrong_direction);
    // Beyond the search radius there is no road.
    assert!(
        network
            .sample([400.0, 100.0, 400.0], [0.0; 3])
            .unwrap()
            .is_none()
    );
    assert!(network.sample([f32::NAN, 0.0, 0.0], [0.0; 3]).is_err());
}

#[test]
fn wrong_direction_follows_the_lane_direction_and_the_native_cosine() {
    let forward = Network::new(vec![straight(Direction::Forward, 55, 0.0, 100.0, 0.0)]).unwrap();
    let backward = Network::new(vec![straight(Direction::Backward, 55, 0.0, 100.0, 0.0)]).unwrap();
    let against = [0.0, 0.0, -20.0];
    let with = [0.0, 0.0, 20.0];
    assert!(
        forward
            .sample([0.0, 100.0, 50.0], against)
            .unwrap()
            .unwrap()
            .wrong_direction
    );
    assert!(
        !forward
            .sample([0.0, 100.0, 50.0], with)
            .unwrap()
            .unwrap()
            .wrong_direction
    );
    assert!(
        backward
            .sample([0.0, 100.0, 50.0], with)
            .unwrap()
            .unwrap()
            .wrong_direction
    );
    assert!(
        !backward
            .sample([0.0, 100.0, 50.0], against)
            .unwrap()
            .unwrap()
            .wrong_direction
    );
    // A crossing at 90 degrees (cosine 0) is not wrong way; -0.5 is.
    assert!(
        !forward
            .sample([0.0, 100.0, 50.0], [20.0, 0.0, 0.0])
            .unwrap()
            .unwrap()
            .wrong_direction
    );
    assert!(
        forward
            .sample([0.0, 100.0, 50.0], [17.3, 0.0, -10.0])
            .unwrap()
            .unwrap()
            .wrong_direction
    );
    // Standing still is never wrong way.
    assert!(
        !forward
            .sample([0.0, 100.0, 50.0], [0.0; 3])
            .unwrap()
            .unwrap()
            .wrong_direction
    );
}

#[test]
fn projection_clamps_to_segment_ends_and_prefers_the_closer_piece() {
    let bent = Segment {
        speed_limit_mph: 55,
        width: 12.0,
        direction: Direction::Forward,
        points: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 10.0], [10.0, 0.0, 10.0]],
    };
    let network = Network::new(vec![bent]).unwrap();
    let before_start = network.nearest([0.0, 0.0, -5.0]).unwrap();
    assert_eq!(before_start.point, [0.0, 0.0, 0.0]);
    assert!((before_start.distance - 5.0).abs() < 1e-5);
    let corner = network.nearest([8.0, 0.0, 12.0]).unwrap();
    assert_eq!(corner.point, [8.0, 0.0, 10.0]);
    assert_eq!(corner.tangent, [1.0, 0.0]);
    assert!((corner.lateral + 2.0).abs() < 1e-5);
}

#[test]
fn bounds_and_invalid_geometry_are_rejected() {
    assert!(Network::new(vec![]).is_err());
    let mut single = straight(Direction::Any, 35, 0.0, 10.0, 0.0);
    single.points.truncate(1);
    assert!(Network::new(vec![single]).is_err());
    let mut nan = straight(Direction::Any, 35, 0.0, 10.0, 0.0);
    nan.points[1][1] = f32::NAN;
    assert!(Network::new(vec![nan]).is_err());
    let mut flat = straight(Direction::Any, 35, 0.0, 10.0, 0.0);
    flat.width = 0.0;
    assert!(Network::new(vec![flat]).is_err());
    let mut long = straight(Direction::Any, 35, 0.0, 10.0, 0.0);
    long.points = vec![[0.0; 3]; MAX_POINTS + 1];
    assert!(Network::new(vec![long]).is_err());
}
