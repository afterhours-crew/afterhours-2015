// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant pacing accessor. The host resolves the participant's current
//! pacing group and actor personality; neither is inferred from nearby cars.
use super::{Error, curve::Curve, finite, participant_properties::Property};

pub const NOS_REBUILD: u32 = 0xa870_fe05;
pub const MAX_PERSONALITIES: usize = 1024;

/// Four independently resolved services in the accessor's schematic context.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct Sources {
    pub pacing: Property,
    pub actors: Property,
    pub personalities: Property,
    pub curves: Property,
}
impl Sources {
    pub fn validate(&self) -> Result<(), Error> {
        for source in [
            &self.pacing,
            &self.actors,
            &self.personalities,
            &self.curves,
        ] {
            source.validate()?;
        }
        Ok(())
    }
}

/// Library order is the actor personality index, including empty overrides.
/// A missing actor personality produces zero; it does not select the fallback.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Library {
    pub personalities: Vec<Option<Curve>>,
    pub fallback: Option<Curve>,
}
impl Library {
    pub fn validate(&self) -> Result<(), Error> {
        if self.personalities.len() > MAX_PERSONALITIES {
            return Err(Error::Bound);
        }
        Ok(())
    }

    /// Evaluate a successful pacing lookup. Failure to find a pacing sample is
    /// handled separately by the accessor, retaining its previous output.
    pub fn evaluate(&self, personality: Option<u16>, pacing: f32) -> Result<f32, Error> {
        self.validate()?;
        finite(pacing)?;
        let Some(personality) = personality else {
            return Ok(0.);
        };
        let curve = self
            .personalities
            .get(usize::from(personality))
            .ok_or(Error::MissingInput)?
            .as_ref()
            .or(self.fallback.as_ref());
        curve.map_or(Ok(0.), |curve| curve.evaluate(pacing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::curve::{Interpolation, Knot};
    fn curve(low: f32, high: f32) -> Curve {
        Curve::new(
            [-1., 1.],
            [low, high],
            1.,
            1.,
            Interpolation::Linear,
            vec![
                Knot {
                    x: 0.,
                    y: 0.,
                    coefficient: 0.,
                },
                Knot {
                    x: 1.,
                    y: 1.,
                    coefficient: 0.,
                },
            ],
        )
        .unwrap()
    }
    #[test]
    fn personality_override_and_empty_override_have_distinct_results() {
        let library = Library {
            personalities: vec![Some(curve(2., 6.)), None],
            fallback: Some(curve(10., 20.)),
        };
        assert_eq!(library.evaluate(Some(0), 0.).unwrap(), 4.);
        assert_eq!(library.evaluate(Some(1), 0.).unwrap(), 15.);
        assert_eq!(library.evaluate(Some(0), -20.).unwrap(), 2.);
        assert_eq!(library.evaluate(Some(1), 20.).unwrap(), 20.);
        assert_eq!(library.evaluate(None, 0.).unwrap(), 0.);
        assert!(library.evaluate(Some(2), 0.).is_err());
        let empty = Library {
            personalities: vec![None],
            fallback: None,
        };
        assert_eq!(empty.evaluate(Some(0), 0.).unwrap(), 0.);
        assert!(empty.evaluate(None, f32::NAN).is_err());
        assert!(library.evaluate(Some(0), f32::INFINITY).is_err());
        assert!(
            Library {
                personalities: vec![None; MAX_PERSONALITIES + 1],
                fallback: None
            }
            .validate()
            .is_err()
        );
    }
}
