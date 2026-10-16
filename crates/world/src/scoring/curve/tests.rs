// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn knots() -> Vec<Knot> {
    vec![
        Knot {
            x: 0.0,
            y: 0.0,
            coefficient: 0.0,
        },
        Knot {
            x: 1.0,
            y: 1.0,
            coefficient: 6.0,
        },
    ]
}
#[test]
fn linear_uses_input_scale_ranges_and_output_scale() {
    let curve = Curve::new(
        [10.0, 30.0],
        [100.0, 200.0],
        2.0,
        3.0,
        Interpolation::Linear,
        knots(),
    )
    .unwrap();
    // Below/above the source range saturate; the midpoint maps to 150 * 3.
    for (input, expected) in [
        (-10.0, 300.0),
        (5.0, 300.0),
        (10.0, 450.0),
        (15.0, 600.0),
        (500.0, 600.0),
    ] {
        assert_eq!(curve.evaluate(input), Ok(expected));
    }
}
#[test]
fn cubic_coefficients_reproduce_a_cubic_and_clip_overshoot() {
    // Natural second derivatives for y=x^3 on [0,1]: 0 and 6.
    let cubic = Curve::new(
        [0.0, 1.0],
        [0.0, 1.0],
        1.0,
        1.0,
        Interpolation::Cubic,
        knots(),
    )
    .unwrap();
    for x in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
        assert!((cubic.evaluate(x).unwrap() - x.powi(3)).abs() < 0.000001);
    }
    let mut rows = knots();
    rows[0].coefficient = -24.0;
    rows[1].coefficient = -24.0;
    let overshoot = Curve::new(
        [0.0, 1.0],
        [20.0, 30.0],
        1.0,
        2.0,
        Interpolation::Cubic,
        rows,
    )
    .unwrap();
    assert_eq!(overshoot.evaluate(0.5), Ok(60.0));
}
#[test]
fn interior_knots_choose_the_correct_piece() {
    let rows = vec![
        Knot {
            x: 0.0,
            y: 0.0,
            coefficient: 0.0,
        },
        Knot {
            x: 0.25,
            y: 0.75,
            coefficient: 0.0,
        },
        Knot {
            x: 0.75,
            y: 0.25,
            coefficient: 0.0,
        },
        Knot {
            x: 1.0,
            y: 1.0,
            coefficient: 0.0,
        },
    ];
    let curve = Curve::new(
        [0.0, 4.0],
        [0.0, 100.0],
        1.0,
        1.0,
        Interpolation::Linear,
        rows,
    )
    .unwrap();
    for (x, y) in [
        (0.5, 37.5),
        (1.0, 75.0),
        (2.0, 50.0),
        (3.0, 25.0),
        (3.5, 62.5),
    ] {
        assert_eq!(curve.evaluate(x), Ok(y));
    }
}
#[test]
fn malformed_and_nonfinite_curves_do_not_produce_scores() {
    let make = |rows| Curve::new([0.0, 1.0], [0.0, 1.0], 1.0, 1.0, Interpolation::Cubic, rows);
    assert_eq!(make(vec![]), Err(Error::Bound));
    assert_eq!(make(vec![knots()[0]; MAX_KNOTS + 1]), Err(Error::Bound));
    let mut duplicate = knots();
    duplicate[1].x = 0.0;
    assert_eq!(make(duplicate), Err(Error::InvalidCurve));
    let mut outside_domain = knots();
    outside_domain[1].x = 1.5;
    assert_eq!(make(outside_domain), Err(Error::InvalidCurve));
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut rows = knots();
        rows[0].coefficient = value;
        assert_eq!(make(rows), Err(Error::NonFinite));
        assert_eq!(
            make(knots()).unwrap().evaluate(value),
            Err(Error::NonFinite)
        );
    }
    assert_eq!(
        Curve::new(
            [0.0, 0.0],
            [0.0, 1.0],
            1.0,
            1.0,
            Interpolation::Linear,
            knots()
        ),
        Err(Error::InvalidCurve)
    );
    let overflow = Curve::new(
        [0.0, 1.0],
        [0.0, 1.0],
        f32::MAX,
        1.0,
        Interpolation::Linear,
        knots(),
    )
    .unwrap();
    assert_eq!(overflow.evaluate(2.0), Err(Error::NonFinite));
}

#[test]
fn outer_segments_extrapolate_to_header_bounds() {
    let rows = vec![
        Knot {
            x: 0.25,
            y: 0.375,
            coefficient: 0.0,
        },
        Knot {
            x: 0.75,
            y: 0.625,
            coefficient: 0.0,
        },
    ];
    for interpolation in [Interpolation::Linear, Interpolation::Cubic] {
        let curve = Curve::new(
            [-2.0, 2.0],
            [0.0, 1.0],
            1.0,
            1.0,
            interpolation,
            rows.clone(),
        )
        .unwrap();
        for (input, expected) in [
            (-100.0, 0.25),
            (-2.0, 0.25),
            (0.0, 0.5),
            (2.0, 0.75),
            (100.0, 0.75),
        ] {
            assert_eq!(curve.evaluate(input), Ok(expected));
        }
    }
}
