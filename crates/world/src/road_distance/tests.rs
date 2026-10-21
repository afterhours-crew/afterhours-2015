use super::*;

fn straight(a: [f32; 3], b: [f32; 3], length: f32) -> Spline {
    Spline {
        controls: [a, a, b, b],
        coefficients: [a, sub(b, a), [0.; 3], [0.; 3]],
        points: vec![a, b],
        end_times: vec![1.],
        end_distances: vec![length],
    }
}
fn grid(count: usize) -> Vec<Vec<Vec<usize>>> {
    let mut grid = vec![vec![Vec::new(); GRID_SIZE]; GRID_SIZE];
    grid[100][100] = (0..count).collect();
    grid
}
fn lane(start: usize, end: usize, segments: Vec<usize>) -> Lane {
    Lane {
        start,
        end,
        segments,
        layer: 1,
    }
}

#[test]
fn authored_length_projection_and_polynomial_evaluation() {
    let mut spline = straight([0.; 3], [10., 0., 0.], 20.);
    // A deliberately different authored length proves this is not chord projection.
    assert_eq!(spline.project([8., 3., 0.]), 4.);
    assert_eq!(spline.at(4.), [2., 0., 0.]);
    spline.coefficients[2] = [0., 4., 0.];
    spline.coefficients[3] = [0., -4., 0.];
    assert_eq!(spline.at(10.), [5., 0.5, 0.]);
    spline.end_distances = vec![5., 20.];
    spline.end_times = vec![0.25, 1.];
    spline.points.insert(1, [2.5, 0., 0.]);
    spline.validate().unwrap();
    assert_eq!(spline.parameter(10.), 0.5);
}

#[test]
fn authored_grid_height_weight_and_disabled_layer() {
    let splines = vec![
        straight([0., 2., 0.], [20., 2., 0.], 20.),
        straight([0., 0., 5.], [20., 0., 5.], 20.),
    ];
    let mut lanes = vec![lane(0, 1, vec![0]), lane(2, 3, vec![1])];
    let network = Network::new(4, lanes.clone(), splines.clone(), grid(2)).unwrap();
    assert_eq!(network.locate([10., 0., 0.]).unwrap().unwrap().segment(), 1);
    assert!(network.locate([1000., 0., 1000.]).unwrap().is_none());
    lanes[1].layer = 0;
    let network = Network::new(4, lanes, splines, grid(2)).unwrap();
    assert_eq!(network.locate([10., 0., 0.]).unwrap().unwrap().segment(), 0);
    assert_eq!(
        network.locate([f32::NAN, 0., 0.]).unwrap_err(),
        Error::NonFinite
    );
}

#[test]
fn bidirectional_distances_multiple_segments_and_disconnected_roads() {
    let splines = vec![
        straight([0.; 3], [10., 0., 0.], 10.),
        straight([10., 0., 0.], [20., 0., 0.], 10.),
        straight([20., 0., 0.], [30., 0., 0.], 10.),
        straight([0., 0., 50.], [10., 0., 50.], 10.),
    ];
    let network = Network::new(
        5,
        vec![
            lane(0, 1, vec![0, 1]),
            lane(1, 2, vec![2]),
            lane(3, 4, vec![3]),
        ],
        splines,
        grid(4),
    )
    .unwrap();
    let at = |x, z| network.locate([x, 0., z]).unwrap().unwrap();
    let target = network.prepare_target(at(25., 0.)).unwrap();
    assert_eq!(network.distance(at(5., 0.), &target).unwrap(), Some(20.));
    let reverse = network.prepare_target(at(5., 0.)).unwrap();
    assert_eq!(network.distance(at(25., 0.), &reverse).unwrap(), Some(20.));
    assert_eq!(network.distance(at(18., 0.), &reverse).unwrap(), Some(13.));
    assert_eq!(network.distance(at(8., 0.), &reverse).unwrap(), Some(3.));
    assert_eq!(network.distance(at(5., 50.), &reverse).unwrap(), None);
}

#[test]
fn rejects_malformed_splines_membership_and_grid_references() {
    let good = straight([0.; 3], [10., 0., 0.], 10.);
    let mut bad = good.clone();
    bad.end_times[0] = 0.9;
    assert_eq!(bad.validate(), Err(Error::Shape));
    bad = good.clone();
    bad.points[0][0] = f32::INFINITY;
    assert_eq!(bad.validate(), Err(Error::NonFinite));
    let duplicate = vec![lane(0, 1, vec![0]), lane(0, 1, vec![0])];
    assert!(matches!(
        Network::new(2, duplicate, vec![good.clone()], grid(1)),
        Err(Error::Shape)
    ));
    assert!(matches!(
        Network::new(2, vec![lane(0, 1, vec![0])], vec![good], grid(2)),
        Err(Error::Shape)
    ));
}

#[test]
fn candidate_bounds_precede_weighted_nearest_comparison() {
    let upper = straight([0., 20., 0.], [10., 20., 0.], 10.);
    let lower = straight([0., 0., 30.], [10., 0., 30.], 10.);
    let position = [-10., 0., 0.];
    assert!(lower.lower_bound(position) > upper.upper_bound(position));
    // The broad-phase bound is unweighted. It can remove a road which would
    // win if height weighting were applied to every neighbouring candidate.
    let network = Network::new(
        4,
        vec![lane(0, 1, vec![0]), lane(2, 3, vec![1])],
        vec![upper, lower],
        grid(2),
    )
    .unwrap();
    assert_eq!(network.locate(position).unwrap().unwrap().segment(), 0);
}
