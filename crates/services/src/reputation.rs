// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Reputation presentation from the connection's committed tables and authored
//! score thresholds. No captured account values belong to this model.
use crate::persistent::{Loaded, key};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Shape,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "reputation {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Field {
    Level,
    Build,
    Crew,
    Outlaw,
    Speed,
    Style,
    Total,
    NextThreshold,
    CurrentThreshold,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scores {
    pub total: i32,
    pub build: i32,
    pub crew: i32,
    pub outlaw: i32,
    pub speed: i32,
    pub style: i32,
}
impl Scores {
    pub fn load(loaded: &Loaded) -> Result<Self, Error> {
        let value = |column: &str| {
            loaded
                .stores()
                .get(&(key("RepValuesTable"), key(column)))
                .copied()
                .ok_or(Error::Shape)
        };
        Ok(Self {
            total: value("RepScore")?,
            build: value("BuildScore")?,
            crew: value("CrewScore")?,
            outlaw: value("OutlawScore")?,
            speed: value("SpeedScore")?,
            style: value("StyleScore")?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Thresholds(Vec<i32>);
impl Thresholds {
    pub fn new(values: Vec<i32>) -> Result<Self, Error> {
        if values.is_empty()
            || values.len() > 256
            || values[0] != 0
            || values.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(Error::Shape);
        }
        Ok(Self(values))
    }
    pub fn evaluate(&self, scores: Scores) -> Values {
        // Count every threshold <= the signed score. Below
        // the first level the lower bound is zero; at maximum both bounds
        // are the final threshold. The saved RepLevel is not the input.
        let level = self
            .0
            .partition_point(|threshold| *threshold <= scores.total);
        Values {
            scores,
            level: level as i32, // validated at most 256 entries
            current: level.checked_sub(1).map_or(0, |i| self.0[i]),
            next: self.0[level.min(self.0.len() - 1)],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Values {
    scores: Scores,
    level: i32,
    current: i32,
    next: i32,
}
impl Values {
    pub fn get(self, field: Field) -> i32 {
        match field {
            Field::Level => self.level,
            Field::Build => self.scores.build,
            Field::Crew => self.scores.crew,
            Field::Outlaw => self.scores.outlaw,
            Field::Speed => self.scores.speed,
            Field::Style => self.scores.style,
            Field::Total => self.scores.total,
            Field::NextThreshold => self.next,
            Field::CurrentThreshold => self.current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_signed_scores_and_cap_follow_thresholds() {
        let thresholds = Thresholds::new(vec![0, 10, 25]).unwrap();
        for (score, level, current, next) in [
            (-1, 0, 0, 0),
            (0, 1, 0, 10),
            (9, 1, 0, 10),
            (10, 2, 10, 25),
            (25, 3, 25, 25),
            (i32::MAX, 3, 25, 25),
        ] {
            let values = thresholds.evaluate(Scores {
                total: score,
                build: 1,
                crew: 2,
                outlaw: 3,
                speed: 4,
                style: 5,
            });
            assert_eq!(
                (
                    values.get(Field::Level),
                    values.get(Field::CurrentThreshold),
                    values.get(Field::NextThreshold)
                ),
                (level, current, next)
            );
            assert_eq!(values.get(Field::Total), score);
            assert_eq!(values.get(Field::Style), 5);
        }
    }
    #[test]
    fn rejects_ambiguous_or_unbounded_thresholds() {
        for values in [vec![], vec![1], vec![0, 0], vec![0, -1], (0..257).collect()] {
            assert!(Thresholds::new(values).is_err());
        }
    }
}
