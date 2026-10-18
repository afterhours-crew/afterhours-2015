// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Road-query transitions from owned measurements. Road projection, native
//! allocation/startup, scheduling and network publication are separate contracts.
use super::{Error, events::PropertyHandler, finite};

pub const SPEED_LIMIT: u32 = 0xf359_bb57;
pub const SPEED_LIMIT_OVERRIDE: u32 = 0x6fb0_4a23;
pub const SPEEDING: u32 = 0xe2d7_4162;
pub const EXCESSIVE_SPEEDING: u32 = 0xf6c1_a883;
pub const NOT_SPEEDING: u32 = 0x0cfd_7886;
pub const WRONG_WAY_BEGIN: u32 = 0x25a2_81e7;
pub const WRONG_WAY_END: u32 = 0xb275_89ef;
pub const METRES_PER_SECOND_TO_MPH: f32 = f32::from_bits(0x400f_2a06);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Configuration {
    pub max_distance_to_segment: f32,
    pub speeding_min_time: f32,
    pub speeding_over_limit: f32,
    pub excessive_speeding_over_limit: f32,
    pub wrong_way_begin_distance: f32,
    pub wrong_way_end_distance: f32,
    pub wrong_way_min_speed: f32,
    pub speeding_debounce_time: f32,
}
impl Configuration {
    pub fn validate(self) -> Result<(), Error> {
        for v in [
            self.max_distance_to_segment,
            self.speeding_min_time,
            self.speeding_over_limit,
            self.excessive_speeding_over_limit,
            self.wrong_way_begin_distance,
            self.wrong_way_end_distance,
            self.wrong_way_min_speed,
            self.speeding_debounce_time,
        ] {
            nonnegative(v)?;
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapshot {
    pub last_speeding_time: f64,
    pub last_not_speeding_time: f64,
    pub speeding_duration: f32,
    pub excessive_duration: f32,
    pub not_speeding_duration: f32,
    pub wrong_way_distance: f32,
    pub speed_limit: f32,
    pub wrong_way: bool,
    /// Native immediate-check state, cleared after a valid on-road speed pass.
    /// Its fresh-construction initialization has not been established.
    pub immediate_speeding_check: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub now: f64,
    pub elapsed: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Installed road value, in mph. An absent road is represented by None for
    /// the whole sample, not a fabricated speed limit of zero.
    pub speed_limit: f32,
    /// Already resolved bound input; only a strictly positive override applies.
    pub speed_limit_override: Option<f32>,
    pub distance_to_segment: f32,
    pub speed_metres_per_second: f32,
    /// Result of the verified road-direction/geometric classification. This is
    /// an owned server measurement, never a client assertion of an award.
    pub wrong_direction: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Event {
    pub id: u32,
    pub integer: Option<i32>,
}
#[derive(Clone, Copy)]
pub(crate) struct Reading {
    pub frame: Frame,
    pub limit: f32,
    pub limit_changed: bool,
    pub on_road: bool,
    pub speed_mph: f32,
    pub speed_mps: f32,
    pub wrong_direction: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    configuration: Configuration,
    snapshot: Snapshot,
    last_tick: Option<f64>,
    pub(crate) limit_bound: bool,
}
impl Node {
    /// Explicit state is required: this does not invent an engine fresh-start or
    /// saved-state initialization policy. The caller also resolves the output's
    /// existence and initializes its Store cell in verified binding order.
    pub fn from_state(
        configuration: Configuration,
        snapshot: Snapshot,
        limit_bound: bool,
    ) -> Result<Self, Error> {
        configuration.validate()?;
        for t in [snapshot.last_speeding_time, snapshot.last_not_speeding_time] {
            clock(t)?;
        }
        for v in [
            snapshot.speeding_duration,
            snapshot.excessive_duration,
            snapshot.not_speeding_duration,
            snapshot.wrong_way_distance,
            snapshot.speed_limit,
        ] {
            nonnegative(v)?;
        }
        Ok(Self {
            configuration,
            snapshot,
            last_tick: None,
            limit_bound,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
    }

    pub(crate) fn prepare(
        &mut self,
        frame: Frame,
        sample: Option<Sample>,
    ) -> Result<Option<Reading>, Error> {
        clock(frame.now)?;
        nonnegative(frame.elapsed)?;
        if self.last_tick.is_some_and(|t| frame.now < t)
            || frame.now
                < self
                    .snapshot
                    .last_speeding_time
                    .max(self.snapshot.last_not_speeding_time)
        {
            return Err(Error::InvalidGraph);
        }
        self.last_tick = Some(frame.now);
        let Some(sample) = sample else {
            return Ok(None);
        };
        nonnegative(sample.speed_limit)?;
        finite(sample.distance_to_segment)?;
        nonnegative(sample.speed_metres_per_second)?;
        if let Some(v) = sample.speed_limit_override {
            finite(v)?;
        }
        let limit = sample
            .speed_limit_override
            .filter(|v| *v > 0.0)
            .unwrap_or(sample.speed_limit);
        let speed_mph = finite(sample.speed_metres_per_second * METRES_PER_SECOND_TO_MPH)?;
        let limit_changed = limit != self.snapshot.speed_limit;
        if limit_changed {
            self.snapshot.speed_limit = limit;
        }
        Ok(Some(Reading {
            frame,
            limit,
            limit_changed,
            speed_mph,
            speed_mps: sample.speed_metres_per_second,
            on_road: sample.distance_to_segment.abs() < self.configuration.max_distance_to_segment,
            wrong_direction: sample.wrong_direction,
        }))
    }
    fn clear_durations(&mut self) {
        self.snapshot.speeding_duration = 0.0;
        self.snapshot.excessive_duration = 0.0;
        self.snapshot.not_speeding_duration = 0.0;
    }
    pub(crate) fn leave_road(&mut self) -> Option<Event> {
        self.clear_durations();
        self.snapshot.wrong_way_distance = 0.0;
        std::mem::take(&mut self.snapshot.wrong_way).then_some(Event {
            id: WRONG_WAY_END,
            integer: None,
        })
    }
    pub(crate) fn speeding(&mut self, r: Reading) -> Result<Option<Event>, Error> {
        let c = self.configuration;
        self.snapshot.wrong_way_distance =
            finite(self.snapshot.wrong_way_distance + finite(r.speed_mps * r.frame.elapsed)?)?;
        let can_start =
            r.frame.now - self.snapshot.last_speeding_time > f64::from(c.speeding_debounce_time);
        let can_end = r.frame.now - self.snapshot.last_not_speeding_time
            > f64::from(c.speeding_debounce_time);
        if !can_start && !can_end {
            return Ok(None);
        }
        let above = r.speed_mph > finite(r.limit + c.speeding_over_limit)?;
        let excessive_limit = finite(r.limit + c.excessive_speeding_over_limit)?;
        let excessive = r.speed_mph > excessive_limit;
        let immediate = self.snapshot.immediate_speeding_check;
        if !immediate && can_start {
            self.snapshot.speeding_duration =
                duration(self.snapshot.speeding_duration, r.frame.elapsed, above)?;
            self.snapshot.excessive_duration =
                duration(self.snapshot.excessive_duration, r.frame.elapsed, excessive)?;
        }
        let trigger_excessive = if immediate {
            excessive
        } else {
            can_start && self.snapshot.excessive_duration > c.speeding_min_time
        };
        let trigger_speeding = if immediate {
            above
        } else {
            can_start && self.snapshot.speeding_duration > c.speeding_min_time
        };
        if trigger_excessive || trigger_speeding {
            self.snapshot.last_speeding_time = r.frame.now;
            self.snapshot.last_not_speeding_time = r.frame.now;
            self.clear_durations();
            return Ok(Some(if trigger_excessive {
                let amount = finite(finite(r.speed_mph - excessive_limit)? * 0.1)?;
                if !(i32::MIN as f32..2_147_483_648.0).contains(&amount) {
                    return Err(Error::ConversionRange);
                }
                Event {
                    id: EXCESSIVE_SPEEDING,
                    integer: Some(amount.trunc() as i32),
                }
            } else {
                Event {
                    id: SPEEDING,
                    integer: None,
                }
            }));
        }
        if !immediate && can_end {
            self.snapshot.not_speeding_duration =
                duration(self.snapshot.not_speeding_duration, r.frame.elapsed, !above)?;
            if self.snapshot.not_speeding_duration > c.speeding_min_time {
                self.snapshot.last_not_speeding_time = r.frame.now;
                self.clear_durations();
                return Ok(Some(Event {
                    id: NOT_SPEEDING,
                    integer: None,
                }));
            }
        }
        Ok(None)
    }
    pub(crate) fn wrong_way(&mut self, r: Reading) -> Option<Event> {
        self.snapshot.immediate_speeding_check = false;
        let c = self.configuration;
        let wrong = r.speed_mph > c.wrong_way_min_speed && r.wrong_direction;
        let event = if self.snapshot.wrong_way {
            if wrong {
                self.snapshot.wrong_way_distance = 0.0;
                return None;
            }
            if self.snapshot.wrong_way_distance > c.wrong_way_end_distance
                || r.speed_mph < c.wrong_way_min_speed
            {
                self.snapshot.wrong_way = false;
                Some(WRONG_WAY_END)
            } else {
                None
            }
        } else if !wrong {
            self.snapshot.wrong_way_distance = 0.0;
            None
        } else if self.snapshot.wrong_way_distance > c.wrong_way_begin_distance {
            self.snapshot.wrong_way = true;
            Some(WRONG_WAY_BEGIN)
        } else {
            None
        };
        event.map(|id| {
            self.snapshot.wrong_way_distance = 0.0;
            Event { id, integer: None }
        })
    }
}
fn nonnegative(v: f32) -> Result<(), Error> {
    finite(v)?;
    if v < 0.0 {
        Err(Error::InvalidGraph)
    } else {
        Ok(())
    }
}
fn clock(t: f64) -> Result<(), Error> {
    if !t.is_finite() {
        Err(Error::NonFinite)
    } else if t < 0.0 {
        Err(Error::InvalidGraph)
    } else {
        Ok(())
    }
}
fn duration(previous: f32, elapsed: f32, active: bool) -> Result<f32, Error> {
    if active {
        finite(previous + elapsed)
    } else {
        Ok(0.0)
    }
}
pub trait Handler: PropertyHandler {
    fn road_query(&mut self, node: u16) -> Result<&mut Node, Error>;
}
