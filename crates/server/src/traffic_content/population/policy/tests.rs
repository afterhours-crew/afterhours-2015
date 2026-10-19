// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn focus() -> Focus {
    Focus {
        id: 1,
        position: [0.; 3],
        velocity: [0.; 3],
        forward: [1., 0., 0.],
    }
}
fn candidate(x: f32, z: f32) -> Candidate {
    Candidate {
        road: 0,
        position: [x, 0., z],
        basis: road_basis([0.; 3], [-1., 0., 0.]).unwrap(),
    }
}
fn model(points: &[Candidate]) -> Policy {
    let mut p = Policy {
        cells: vec![Vec::new(); GRID * GRID],
        roads: vec![Road {
            quota: 3,
            spacing: 20.,
            personas: vec![0, 1, 2],
        }],
        speeds: vec![100.],
    };
    for c in points {
        p.cells[cell(c.position[0]) * GRID + cell(c.position[2])].push(*c);
    }
    p
}
fn car(position: [f32; 3], velocity: [f32; 3], forward: [f32; 3]) -> Car {
    Car {
        anchor: position,
        velocity,
        forward,
        road: 0,
        born: 0,
    }
}
fn choose(
    p: &Policy,
    f: Focus,
    cars: &BTreeMap<u64, Car>,
    counts: &BTreeMap<usize, u32>,
) -> Option<Candidate> {
    p.select(&p.query(&f, 100), &f, &[f], cars, counts, [0; 2])
}

#[test]
fn shaped_region_is_not_a_circle_and_other_focus_excludes_it() {
    let point = candidate(200., 150.);
    assert!(focus().eligible(&point));
    let p = model(&[point]);
    let q = p.query(&focus(), 100);
    assert_eq!(q.candidates.len(), 1);
    assert!(choose(&p, focus(), &BTreeMap::new(), &BTreeMap::new()).is_some());
    let other = Focus {
        id: 2,
        position: point.position,
        ..focus()
    };
    assert!(
        p.select(
            &q,
            &focus(),
            &[focus(), other],
            &BTreeMap::new(),
            &BTreeMap::new(),
            [0; 2]
        )
        .is_none()
    );
    for point in [
        candidate(300., 0.),
        candidate(472., 0.),
        candidate(0., 200.),
    ] {
        assert!(!focus().eligible(&point));
    }
    let raised = Candidate {
        position: [325., 100., 0.],
        ..candidate(325., 0.)
    };
    assert!(focus().eligible(&raised)); // No arbitrary floor-height veto.
}

#[test]
fn query_uses_two_cells_in_each_axis_and_preserves_cell_order() {
    let points = [
        candidate(350., 0.),
        candidate(-350., 0.),
        candidate(0., 350.),
        candidate(0., -350.),
        candidate(0., 0.),
    ];
    let p = model(&points);
    let q = p.query(&focus(), 100);
    let actual: Vec<_> = q.candidates.iter().map(|c| c.position).collect();
    assert_eq!(
        actual,
        vec![
            points[1].position,
            points[3].position,
            points[0].position,
            points[2].position
        ]
    );
    assert_eq!(q.target, 5); // One road, maximum factor9, truncated target5.
    assert_eq!(cell(-8192.9), 0); // Truncate before offset; do not floor.
    assert!(
        p.query(
            &Focus {
                position: [-20_000., 0., 0.],
                ..focus()
            },
            100
        )
        .candidates
        .is_empty()
    );
    assert!(
        p.query(
            &Focus {
                position: [20_000., 0., 0.],
                ..focus()
            },
            100
        )
        .candidates
        .is_empty()
    );
}

#[test]
fn first_zero_quota_spawn_and_retained_zero_count_are_distinct() {
    let mut p = model(&[candidate(325., 0.)]);
    p.roads[0].quota = 0;
    assert!(choose(&p, focus(), &BTreeMap::new(), &BTreeMap::new()).is_some());
    for count in [0, 1] {
        assert!(choose(&p, focus(), &BTreeMap::new(), &BTreeMap::from([(0, count)])).is_none());
    }
    p.roads[0].quota = 3;
    assert!(choose(&p, focus(), &BTreeMap::new(), &BTreeMap::from([(0, 2)])).is_some());
    assert!(choose(&p, focus(), &BTreeMap::new(), &BTreeMap::from([(0, 3)])).is_none());
}

#[test]
fn jam_is_around_actual_focus_and_spacing_is_horizontal_same_heading() {
    let point = candidate(325., 0.);
    let p = model(&[point]);
    let mut cars: BTreeMap<_, _> = (1..=3)
        .map(|i| (i, car([i as f32, 0., 0.], [0.; 3], [1., 0., 0.])))
        .collect();
    assert!(choose(&p, focus(), &cars, &BTreeMap::new()).is_none());
    for c in cars.values_mut() {
        c.velocity = [5., 0., 0.];
    }
    assert!(choose(&p, focus(), &cars, &BTreeMap::new()).is_some());
    cars = BTreeMap::from([(1, car([375., 100., 0.], [0.; 3], [-1., 0., 0.]))]);
    assert!(choose(&p, focus(), &cars, &BTreeMap::new()).is_none());
    cars.get_mut(&1).unwrap().forward = [1., 0., 0.];
    assert!(choose(&p, focus(), &cars, &BTreeMap::new()).is_some());
}

#[test]
fn approach_gate_uses_player_speed_and_flat_segment_table_index() {
    let point = Candidate {
        basis: road_basis([0.; 3], [1., 0., 0.]).unwrap(),
        ..candidate(325., 0.)
    };
    let p = model(&[point]);
    assert!(!p.approaching(&focus(), &point));
    assert!(p.approaching(
        &Focus {
            velocity: [10., 0., 0.],
            ..focus()
        },
        &point
    ));
    assert!(p.approaching(
        &Focus {
            position: [650., 0., 0.],
            ..focus()
        },
        &point
    ));
}

#[test]
fn retention_predicts_three_seconds_capped_at_three_hundred_metres() {
    let mut f = focus();
    assert!(!f.retains([450., 0., 0.]));
    assert!(f.retains([449., 500., 0.]));
    f.velocity = [100., 0., 0.];
    assert_eq!(f.predicted(), [300., 0., 0.]);
    assert!(f.retains([599., 0., 0.]));
    assert!(!f.retains([600., 0., 0.]));
    f.velocity = [1000., 0., 0.];
    assert_eq!(f.predicted(), [300., 0., 0.]);
    f.velocity = [0.1, 0., 0.];
    assert_eq!(f.predicted(), [0.; 3]);
}

#[test]
fn persona_cursor_preserves_provider_order_across_different_tag_sets() {
    let mut p = model(&[]);
    assert_eq!(p.persona(0, 2), Some((2, 0)));
    p.roads[0].personas = vec![1, 4];
    assert_eq!(p.persona(0, 4), Some((1, 1))); // Advance the previous cursor, not the fallback index.
    p.roads[0].personas.clear();
    assert_eq!(p.persona(0, 0), None);
}

#[test]
fn installed_sampling_uses_arc_distance_carry_and_level_spawn_heading() {
    let (_, _, mut c) = super::super::tests::fixture();
    let segment = |x: f32| nfs_content::traffic::Segment {
        speed: 10. + x,
        points: vec![[x, 0., 0.], [x + 30., 9., 0.]],
        curve: nfs_content::traffic::Curve::new(
            [
                [x, 0., 0.],
                [x + 10., 3., 0.],
                [x + 20., 6., 0.],
                [x + 30., 9., 0.],
            ],
            vec![1.],
            vec![30.],
        )
        .unwrap(),
    };
    c.splines[0].segments = vec![segment(0.), segment(30.)];
    c.splines[0].density = 20;
    let p = Policy::new(&c).unwrap();
    let points: Vec<_> = p.cells.iter().flatten().collect();
    assert_eq!(points.len(), 3);
    for (point, x) in points.iter().zip([0., 25., 50.1]) {
        assert!((point.position[0] - x).abs() < 0.001);
        assert_eq!(point.basis[1], [0., 1., 0.]);
        assert_eq!(point.basis[2], [1., 0., 0.]);
    }
    assert_eq!(p.roads[0].quota, 3);
    assert_eq!(
        p.speeds,
        vec![(10f32 * 0.447_04).powi(2), (40f32 * 0.447_04).powi(2)]
    );
    c.splines[0].tags.clear();
    assert!(Policy::new(&c).unwrap().roads[0].personas.is_empty());
}

#[test]
fn selection_wraps_random_scan_once_and_caps_the_shortlist_at_twenty() {
    let points: Vec<_> = (0..26)
        .map(|i| candidate(325. + i as f32 * 4., 0.))
        .collect();
    let p = model(&points);
    let q = p.query(&focus(), 100);
    assert_eq!(q.candidates.len(), 26);
    for (random, expected) in [([0, 19], 19), ([12, 19], 5), ([26, 20], 0)] {
        let selected = p
            .select(
                &q,
                &focus(),
                &[focus()],
                &BTreeMap::new(),
                &BTreeMap::new(),
                random,
            )
            .unwrap();
        assert_eq!(selected.position, points[expected].position);
    }
}

#[test]
fn segment_table_indexing_and_candidate_resource_bounds_are_explicit() {
    let (_, _, mut c) = super::super::tests::fixture();
    let segment = c.splines[0].segments[0].clone();
    c.splines[0].segments.push(segment.clone());
    c.splines[0].segments[0].speed = 10.;
    c.splines[0].segments[1].speed = 40.;
    c.splines.push(c.splines[0].clone());
    c.splines[1].segments = vec![segment.clone()];
    c.splines[1].segments[0].speed = 100.;
    let p = Policy::new(&c).unwrap();
    let point = Candidate {
        road: 1,
        basis: road_basis([0.; 3], [1., 0., 0.]).unwrap(),
        ..candidate(325., 0.)
    };
    assert!(p.approaching(
        &Focus {
            velocity: [30., 0., 0.],
            ..focus()
        },
        &point
    ));
    let mut long = segment;
    long.curve = nfs_content::traffic::Curve::new(
        [
            [-8000., 0., 0.],
            [-4000., 0., 0.],
            [4000., 0., 0.],
            [8000., 0., 0.],
        ],
        vec![1.],
        vec![100_000.],
    )
    .unwrap();
    c.splines.truncate(1);
    c.splines[0].segments = vec![long; 34];
    assert!(matches!(Policy::new(&c), Err(Error::Bound)));
}

#[test]
fn linked_empty_roads_keep_their_ordinal_without_sampling_or_speed_entries() {
    let (_, _, mut content) = super::super::tests::fixture();
    let mut empty = content.splines[0].clone();
    empty.segments.clear();
    let segment = content.splines[0].segments[0].clone();
    content.splines[0].segments.push(segment);
    content.splines.insert(0, empty);
    let policy = Policy::new(&content).unwrap();
    assert_eq!(policy.roads.len(), 2);
    assert_eq!(policy.roads[0].quota, 0);
    assert_eq!(policy.speeds.len(), 2);
    let candidates: Vec<_> = policy.cells.iter().flatten().collect();
    assert!(!candidates.is_empty());
    assert!(candidates.iter().all(|c| c.road == 1));
}

#[test]
fn traffic_amount_scales_the_target_and_each_road_quota() {
    let points = [candidate(325., 0.)];
    let mut p = model(&points);
    // The original target here is 5 (see the shaped-region test).
    let target = |percent| p.query(&focus(), percent).target;
    assert_eq!(
        (
            target(100),
            target(0),
            target(50),
            target(200),
            target(300),
            target(500)
        ),
        (5, 0, 3, 10, 15, 25)
    );
    p.roads[0].quota = 3;
    let pick = |p: &Policy, percent, count| {
        let f = focus();
        p.select(
            &p.query(&f, percent),
            &f,
            &[f],
            &BTreeMap::new(),
            &BTreeMap::from([(0, count)]),
            [0; 2],
        )
        .is_some()
    };
    // Quota 3 at 100 %, 6 at 200 %, 15 at 500 %, 2 at 50 %.
    assert!(pick(&p, 100, 2) && !pick(&p, 100, 3));
    assert!(pick(&p, 200, 5) && !pick(&p, 200, 6));
    assert!(pick(&p, 500, 14) && !pick(&p, 500, 15));
    assert!(pick(&p, 50, 1) && !pick(&p, 50, 2));
}
