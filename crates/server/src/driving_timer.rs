// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local driving-time telemetry state. Authored Start opens the gate and stores
//! race time; Stop samples elapsed time without closing the gate. Only a current
//! owned car supplies a measurement. No external telemetry is sent.

use nfs_world::{logic::Message, replication::Error};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Signal {
    pub entity: u32,
    pub event: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    pub role: u16,
    pub start: Signal,
    pub stop: Signal,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Start,
    Stop,
}
impl Binding {
    pub fn action(&self, message: &Message) -> Option<Action> {
        let Message::Reached { target, event, .. } = message else {
            return None;
        };
        let signal = Signal {
            entity: target.entity,
            event: *event,
        };
        if signal == self.start {
            Some(Action::Start)
        } else if signal == self.stop {
            Some(Action::Stop)
        } else {
            None
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Owner {
    pub participant: u16,
    pub ghost: u16,
    pub local_slot: u8,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub item: u64,
    pub seconds: f32,
    pub stopped_at: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    owner: Owner,
    clock_ms: u64,
    started_at: Option<f32>,
    pub last: Option<Measurement>,
    pub completed: u64,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timers {
    participants: BTreeMap<u16, State>,
}
impl Timers {
    pub fn state(&self, participant: u16) -> Option<&State> {
        self.participants.get(&participant)
    }
    pub fn retain_ghosts(&mut self, live: impl Fn(u16) -> bool) {
        self.participants.retain(|_, state| live(state.owner.ghost));
    }
    pub fn receive(
        &mut self,
        owner: Owner,
        action: Action,
        item: Option<u64>,
        world_ms: u64,
    ) -> Result<Option<Measurement>, Error> {
        if owner.participant == 0
            || owner.participant > 8191
            || owner.ghost == 0
            || owner.ghost > 8191
            || owner.local_slot > 7
            || item == Some(0)
        {
            return Err(Error::UnknownObject);
        }
        if let Some(old) = self.participants.get(&owner.participant) {
            if old.owner != owner {
                return Err(Error::UnknownObject);
            }
            if world_ms < old.clock_ms {
                return Err(Error::Shape);
            }
        } else if self.participants.len() >= 128 {
            return Err(Error::Bound);
        }
        let state = self.participants.entry(owner.participant).or_insert(State {
            owner,
            clock_ms: world_ms,
            started_at: None,
            last: None,
            completed: 0,
        });
        state.clock_ms = world_ms;
        let seconds = (world_ms as f64 / 1000.0) as f32;
        if action == Action::Start {
            state.started_at = Some(seconds);
            return Ok(None);
        }
        let (Some(started), Some(item)) = (state.started_at, item) else {
            return Ok(None);
        };
        let measurement = Measurement {
            item,
            seconds: seconds - started,
            stopped_at: seconds,
        };
        state.last = Some(measurement);
        state.completed = state.completed.saturating_add(1);
        Ok(Some(measurement))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner(p: u16) -> Owner {
        Owner {
            participant: p,
            ghost: p + 200,
            local_slot: 0,
        }
    }
    #[test]
    fn initial_stop_open_gate_repeated_samples_and_start_reset_follow_authored_graph() {
        let mut timers = Timers::default();
        let o = owner(1);
        assert_eq!(timers.receive(o, Action::Stop, Some(3), 100).unwrap(), None);
        timers.receive(o, Action::Start, Some(3), 1000).unwrap();
        let sample = timers
            .receive(o, Action::Stop, Some(3), 7250)
            .unwrap()
            .unwrap();
        assert_eq!(
            sample,
            Measurement {
                item: 3,
                seconds: 6.25,
                stopped_at: 7.25
            }
        );
        assert_eq!(
            timers
                .receive(o, Action::Stop, Some(3), 8000)
                .unwrap()
                .unwrap()
                .seconds,
            7.0
        );
        timers.receive(o, Action::Start, Some(3), 9000).unwrap();
        assert_eq!(
            timers
                .receive(o, Action::Stop, Some(4), 10_500)
                .unwrap()
                .unwrap()
                .seconds,
            1.5
        );
        assert_eq!(timers.state(1).unwrap().completed, 3);
        assert_eq!(timers.state(1).unwrap().last.unwrap().item, 4);
    }
    #[test]
    fn borrowed_cars_wrong_lifetimes_and_backwards_clocks_cannot_add_measurements() {
        let mut timers = Timers::default();
        let o = owner(1);
        timers.receive(o, Action::Start, None, 100).unwrap();
        assert_eq!(timers.receive(o, Action::Stop, None, 200).unwrap(), None);
        let before = timers.clone();
        assert_eq!(
            timers.receive(o, Action::Stop, Some(1), 199),
            Err(Error::Shape)
        );
        assert_eq!(
            timers.receive(Owner { ghost: 202, ..o }, Action::Stop, Some(1), 300),
            Err(Error::UnknownObject)
        );
        assert_eq!(timers, before);
        timers
            .receive(owner(2), Action::Start, Some(2), 300)
            .unwrap();
        timers.retain_ghosts(|g| g != o.ghost);
        assert!(timers.state(1).is_none());
        assert!(timers.state(2).is_some());
        assert_eq!(
            timers
                .receive(Owner { ghost: 203, ..o }, Action::Stop, Some(1), 400)
                .unwrap(),
            None
        );
    }
}
