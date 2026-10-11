// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Error;

#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub points: [[f32; 2]; 8],
}
impl Curve {
    fn at_zero(&self) -> Result<f32, Error> {
        if self
            .min
            .iter()
            .chain(&self.max)
            .chain(self.points.iter().flatten())
            .any(|v| !v.is_finite())
        {
            return Err(Error::Shape);
        }
        let points = self.points.map(|p| {
            std::array::from_fn::<_, 2, _>(|i| (self.max[i] - self.min[i]) * p[i] + self.min[i])
        });
        if points.iter().flatten().any(|v| !v.is_finite())
            || points.windows(2).any(|p| p[0][0] >= p[1][0])
        {
            return Err(Error::Shape);
        }
        if 0.0 < points[0][0] {
            return Ok(points[0][1]);
        }
        if 0.0 >= points[7][0] {
            return Ok(points[7][1]);
        }
        for pair in points.windows(2) {
            let [a, b] = pair else { unreachable!() };
            if 0.0 < b[0] {
                return Ok(((b[1] - a[1]) / (b[0] - a[0])) * (0.0 - a[0]) + a[1]);
            }
        }
        Err(Error::Shape)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Suspension {
    pub ride_height_inches: f32,
    pub spring_rate: f32,
    pub spring_progression: f32,
}
impl Suspension {
    fn height(self, load: f32) -> Result<f32, Error> {
        if !self.ride_height_inches.is_finite()
            || !self.spring_rate.is_finite()
            || !self.spring_progression.is_finite()
            || self.spring_rate <= 0.0
            || self.spring_progression <= 0.0
        {
            return Err(Error::Shape);
        }
        let spring = (self.spring_rate * 12.0) * 14.5939;
        let divisor = (self.spring_progression * 2.0) * spring;
        let term = ((self.spring_progression * -4.0) * spring) * load;
        let squared = spring * spring - term;
        let deflection = (squared.sqrt() - spring) / divisor;
        let result = self.ride_height_inches * 0.0254 - deflection;
        if !result.is_finite() {
            return Err(Error::Shape);
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RestingConfig {
    pub mass: f32,
    pub front_axle: f32,
    pub wheelbase: f32,
    pub front_weight_bias_percent: Curve,
    pub front: Suspension,
    pub rear: Suspension,
}
impl RestingConfig {
    pub fn position_at(&self, locator: [f32; 3]) -> Result<[Option<u32>; 3], Error> {
        if !self.mass.is_finite()
            || self.mass < 0.0
            || !self.front_axle.is_finite()
            || !self.wheelbase.is_finite()
            || self.wheelbase <= 0.0
            || !(0.0..=self.wheelbase).contains(&self.front_axle)
            || locator.iter().any(|v| !v.is_finite())
        {
            return Err(Error::Shape);
        }
        let bias = self.front_weight_bias_percent.at_zero()? * 0.01;
        if !bias.is_finite() || !(0.0..=1.0).contains(&bias) {
            return Err(Error::Shape);
        }
        let front = self.front.height(((self.mass * bias) * 0.5) * 9.81)?;
        let rear = self
            .rear
            .height((((1.0 - bias) * self.mass) * 0.5) * 9.81)?;
        let fraction = self.front_axle / self.wheelbase;
        let height = (1.0 - fraction) * front + fraction * rear;
        let position = [locator[0], locator[1] + height, locator[2]];
        if position.iter().any(|v| !v.is_finite()) {
            return Err(Error::Shape);
        }
        Ok(position.map(|v| (v != 0.0).then(|| v.to_bits())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RestingConfig {
        RestingConfig {
            mass: 0.0,
            front_axle: 1.0,
            wheelbase: 2.0,
            front_weight_bias_percent: Curve {
                min: [0.0, 0.0],
                max: [7.0, 100.0],
                points: std::array::from_fn(|i| [i as f32 / 7.0, 0.5]),
            },
            front: Suspension {
                ride_height_inches: 10.0,
                spring_rate: 100.0,
                spring_progression: 0.1,
            },
            rear: Suspension {
                ride_height_inches: 10.0,
                spring_rate: 100.0,
                spring_progression: 0.1,
            },
        }
    }

    #[test]
    fn unloaded_equal_axles_convert_inches_and_preserve_world_axes() {
        let c = config();
        assert_eq!(
            c.position_at([12.5, 0.0, -7.25]).unwrap(),
            [
                Some(12.5f32.to_bits()),
                Some(0.254f32.to_bits()),
                Some((-7.25f32).to_bits())
            ]
        );
        assert_eq!(c.position_at([-0.0, -0.254, 0.0]).unwrap(), [None; 3]);
    }

    #[test]
    fn weight_and_configuration_are_per_vehicle_and_do_not_mutate() {
        let light = config();
        let mut heavy = light.clone();
        heavy.mass = 1500.0;
        let before = heavy.clone();
        let a = light.position_at([1.0, 0.0, 2.0]).unwrap();
        let b = heavy.position_at([1.0, 0.0, 2.0]).unwrap();
        assert!(f32::from_bits(b[1].unwrap()) < f32::from_bits(a[1].unwrap()));
        assert_eq!(heavy, before);
        assert_eq!(light.position_at([1.0, 0.0, 2.0]).unwrap(), a);
    }

    #[test]
    fn curve_interpolates_and_clamps_at_zero() {
        let mut curve = config().front_weight_bias_percent;
        curve.min = [-7.0, 0.0];
        curve.max = [7.0, 100.0];
        curve.points = std::array::from_fn(|i| [i as f32 / 7.0, i as f32 / 7.0]);
        assert!((curve.at_zero().unwrap() - 50.0).abs() < 0.00001);
        curve.min[0] = 1.0;
        assert_eq!(curve.at_zero().unwrap(), 0.0);
        curve.min[0] = -14.0;
        curve.max[0] = -7.0;
        assert_eq!(curve.at_zero().unwrap(), 100.0);
    }

    #[test]
    fn malformed_physics_and_nonfinite_positions_are_rejected() {
        for index in 0..9 {
            let mut c = config();
            match index {
                0 => c.mass = f32::NAN,
                1 => c.wheelbase = 0.0,
                2 => c.front_axle = 3.0,
                3 => c.front.spring_rate = 0.0,
                4 => c.rear.spring_progression = 0.0,
                5 => c.front_weight_bias_percent.points[1][0] = 0.0,
                6 => c.front_weight_bias_percent.points[0][1] = 2.0,
                7 => c.mass = f32::MAX,
                _ => c.front.ride_height_inches = f32::INFINITY,
            }
            assert!(c.position_at([0.0; 3]).is_err(), "case {index}");
        }
        assert!(config().position_at([0.0, f32::NAN, 0.0]).is_err());
    }
}
