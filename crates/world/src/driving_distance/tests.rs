// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn sample(position: [f32; 3]) -> Sample {
    Sample {
        position,
        airborne: false,
        crashed: false,
    }
}

#[test]
fn fractional_distance_is_accumulated_before_integer_conversion() {
    let mut session = Session::new(999, 0., 1.).unwrap();
    assert_eq!(session.advance(0., Some(sample([20., 0., 0.]))), Ok(None));
    assert_eq!(session.advance(0.5, Some(sample([100., 0., 0.]))), Ok(None));
    assert_eq!(
        session.advance(1., Some(sample([20.5, 0., 0.]))),
        Ok(Some(999))
    );
    assert_eq!(
        session.advance(2., Some(sample([21., 0., 0.]))),
        Ok(Some(1000))
    );
    assert_eq!(
        session.advance(3., Some(sample([24., 4., 12.]))),
        Ok(Some(1013))
    );
    assert_eq!(session.metres(), Ok(1013));
    let before = session.clone();
    assert_eq!(session.advance(3., Some(sample([200., 0., 0.]))), Ok(None));
    assert_eq!(session, before);
}

#[test]
fn airborne_and_crash_drop_vertical_displacement_without_dropping_ground_distance() {
    for (airborne, crashed, expected) in [
        (false, false, 13),
        (true, false, 5),
        (false, true, 5),
        (true, true, 5),
    ] {
        let mut session = Session::new(7, 0., 1.).unwrap();
        session.advance(0., Some(sample([0.; 3]))).unwrap();
        let input = Sample {
            position: [3., 12., 4.],
            airborne,
            crashed,
        };
        assert_eq!(session.advance(1., Some(input)), Ok(Some(7 + expected)));
    }
}

#[test]
fn relocation_missing_vehicle_and_new_logic_do_not_count_position_jumps() {
    let mut session = Session::new(12, 0., 1.).unwrap();
    session.advance(0., Some(sample([0.; 3]))).unwrap();
    session.advance(1., Some(sample([0.5, 0., 0.]))).unwrap();
    session.rebase(1.5, Some(sample([1000., 0., 0.]))).unwrap();
    assert_eq!(
        session.advance(2.5, Some(sample([1000.5, 0., 0.]))),
        Ok(Some(13))
    );
    session.advance(3., None).unwrap();
    assert_eq!(session.advance(4., Some(sample([9000., 0., 0.]))), Ok(None));
    assert_eq!(
        session.advance(5., Some(sample([9003., 0., 0.]))),
        Ok(Some(16))
    );
    let mut next = Session::new(session.metres().unwrap(), 5., 1.).unwrap();
    assert_eq!(next.advance(5., Some(sample([-2000., 0., 0.]))), Ok(None));
    assert_eq!(
        next.advance(6., Some(sample([-1996., 0., 0.]))),
        Ok(Some(20))
    );
    assert_eq!(session.metres(), Ok(16));
}

#[test]
fn late_tick_samples_once_and_zero_motion_does_not_accrue_distance() {
    let mut session = Session::new(3, 0., 1.).unwrap();
    session.advance(0., Some(sample([0.; 3]))).unwrap();
    assert_eq!(
        session.advance(10., Some(sample([3., 4., 0.]))),
        Ok(Some(8))
    );
    assert_eq!(session.advance(10.1, Some(sample([3., 4., 0.]))), Ok(None));
    assert_eq!(
        session.advance(11., Some(sample([3., 4., 0.]))),
        Ok(Some(8))
    );
}

#[test]
fn conversion_uses_integer_kilometres_and_float_imperial_thresholds() {
    let factor = 1609.344_f32;
    for (metres, kilometres, miles) in [
        (0, 0, 0),
        (999, 0, 0),
        (1000, 1, 0),
        (1609, 1, 0),
        (1610, 1, 1),
        (3218, 3, 1),
        (3219, 3, 2),
        (43_000, 43, 26),
    ] {
        assert_eq!(
            Metrics::from_metres(metres, factor),
            Ok(Metrics { kilometres, miles })
        );
    }
    for factor in [0., -1., f32::NAN, f32::INFINITY] {
        assert_eq!(Metrics::from_metres(1, factor), Err(Error::Invalid));
    }
    assert_eq!(Metrics::from_metres(-1, factor), Err(Error::Invalid));
}

#[test]
fn invalid_or_overflowing_samples_preserve_the_entire_session() {
    assert!(Session::new(0, 1e100, 1.).is_err());
    for period in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(Session::new(0, 0., period).is_err());
    }
    let mut session = Session::new(i32::MAX - 1, 0., 1.).unwrap();
    session.advance(0., Some(sample([0.; 3]))).unwrap();
    for (now, position) in [
        (1., [2., 0., 0.]),
        (1., [f32::MAX; 3]),
        (1., [f32::NAN; 3]),
        (-1., [0.; 3]),
        (f64::INFINITY, [0.; 3]),
    ] {
        let before = session.clone();
        assert!(session.advance(now, Some(sample(position))).is_err());
        assert_eq!(session, before);
    }
    assert_eq!(
        session.advance(1., Some(sample([1., 0., 0.]))),
        Ok(Some(i32::MAX))
    );
    assert_eq!(
        session.advance(2., Some(sample([1., 0., 0.]))),
        Ok(Some(i32::MAX))
    );
}

#[test]
fn decoded_chassis_controls_are_the_flags_used_by_distance() {
    use crate::replication::vehicle::chassis::{Controls, Motion};
    let motion = Motion {
        identity: 1,
        revision: 1,
        position: [3., 4., 5.],
        quaternion: [0., 0., 0., 1.],
        velocity: [0.; 3],
        angular_velocity: [0.; 3],
    };
    for state in 0..16 {
        for crashed in [false, true] {
            let mut controls = Controls::new(0).unwrap();
            controls.counters[0] = state;
            controls.flags[0] = crashed;
            controls.flags[1..].fill(!crashed);
            let physics = motion.encode([0.; 3], &controls).unwrap();
            let query = crate::scoring::interpreter::Vehicle::from_physics(7, &physics).unwrap();
            assert_eq!(query.identity, 7);
            assert_eq!(query.airborne, Some(state == 1));
            assert_eq!(query.state_3c230e35, Some(crashed));
            assert_eq!(query.state_cd5ae2f0, None);
            assert_eq!(
                Sample::from_physics(&physics),
                Ok(Sample {
                    position: motion.position,
                    airborne: state == 1,
                    crashed
                })
            );
        }
    }
}
