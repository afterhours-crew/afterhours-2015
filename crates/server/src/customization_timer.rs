// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_world::{
    logic::{EntityRef, Message},
    replication::Error,
};
use std::collections::BTreeMap;

pub const START: u32 = 31_524_290;
pub const STOP: u32 = 4_229_223;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Owner {
    pub participant: u16,
    pub ghost: u16,
    pub local_slot: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub item: Option<u64>,
    pub seconds: f32,
    pub stopped_at: f32,
    pub at_least_five: bool,
    pub at_most_five: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    owner: Owner,
    clock_ms: u64,
    started_at: Option<f32>,
    pub last: Option<Measurement>,
    pub completed: u64,
    pub threshold_value: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timers {
    participants: BTreeMap<u16, State>,
}

impl Timers {
    pub fn recognizes(message: &Message) -> bool {
        matches!(
            message,
            Message::Reached {
                event: START | STOP,
                ..
            }
        )
    }
    pub fn state(&self, participant: u16) -> Option<&State> {
        self.participants.get(&participant)
    }
    pub fn receive(
        &mut self,
        owner: Owner,
        item: Option<u64>,
        world_ms: u64,
        message: &Message,
    ) -> Result<Option<Measurement>, Error> {
        let Message::Reached {
            event,
            target,
            player,
        } = *message
        else {
            return Err(Error::Shape);
        };
        if !Self::recognizes(message) {
            return Err(Error::Unsupported);
        }
        if target
            != (EntityRef {
                ghost: owner.ghost,
                entity: 2,
            })
            || player != owner.local_slot
            || owner.local_slot > 7
            || owner.ghost == 0
            || owner.participant == 0
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
            threshold_value: false,
        });
        state.clock_ms = world_ms;
        let seconds = (world_ms as f64 / 1000.0) as f32;
        if event == START {
            state.threshold_value = false;
            state.started_at = Some(seconds);
            return Ok(None);
        }
        let Some(start) = state.started_at.take() else {
            return Ok(None);
        };
        let elapsed = seconds - start;
        let measurement = Measurement {
            item,
            seconds: elapsed,
            stopped_at: seconds,
            at_least_five: elapsed >= 5.0,
            at_most_five: elapsed <= 5.0,
        };
        if measurement.at_least_five {
            state.threshold_value = true;
        }
        if measurement.at_most_five {
            state.threshold_value = false;
        }
        state.last = Some(measurement);
        state.completed = state.completed.saturating_add(1);
        Ok(Some(measurement))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner(participant: u16, local_slot: u8) -> Owner {
        Owner {
            participant,
            ghost: participant + 20,
            local_slot,
        }
    }
    fn input(owner: Owner, event: u32) -> Message {
        Message::Reached {
            event,
            target: EntityRef {
                ghost: owner.ghost,
                entity: 2,
            },
            player: owner.local_slot,
        }
    }
    fn receive(t: &mut Timers, o: Owner, event: u32, ms: u64) -> Option<Measurement> {
        t.receive(o, Some(1), ms, &input(o, event)).unwrap()
    }
    #[test]
    fn gate_repeats_float_boundaries_and_current_item() {
        let mut t = Timers::default();
        let o = owner(1, 0);
        assert_eq!(receive(&mut t, o, STOP, 0), None);
        assert_eq!(receive(&mut t, o, STOP, 0), None);
        for (i, duration) in [4999, 5000, 5001].into_iter().enumerate() {
            let start = (i as u64 + 1) * 10000;
            receive(&mut t, o, START, start);
            let m = receive(&mut t, o, STOP, start + duration).unwrap();
            assert_eq!(m.at_least_five, duration >= 5000);
            assert_eq!(m.at_most_five, duration <= 5000);
            assert_eq!(t.state(1).unwrap().threshold_value, duration > 5000);
            assert_eq!(receive(&mut t, o, STOP, start + duration), None);
        }
        receive(&mut t, o, START, 40000);
        receive(&mut t, o, START, 47000);
        let m = t
            .receive(o, Some(2), 48000, &input(o, STOP))
            .unwrap()
            .unwrap();
        assert_eq!((m.item, m.seconds), (Some(2), 1.0));
        receive(&mut t, o, START, 50000);
        let m = t.receive(o, None, 60000, &input(o, STOP)).unwrap().unwrap();
        assert_eq!((m.item, m.seconds), (None, 10.0));
        assert_eq!(t.state(1).unwrap().completed, 5);
        assert!(Timers::default().state(1).is_none());
    }
    #[test]
    fn independent_owners_reject_wrong_bus_slot_identity_and_clock_without_mutation() {
        let mut t = Timers::default();
        let a = owner(1, 0);
        let b = owner(2, 1);
        receive(&mut t, a, START, 0);
        receive(&mut t, b, START, 1000);
        assert_eq!(receive(&mut t, a, STOP, 7000).unwrap().seconds, 7.0);
        assert_eq!(receive(&mut t, b, STOP, 7000).unwrap().seconds, 6.0);
        let before = t.clone();
        for bad in [
            Owner { local_slot: 1, ..a },
            Owner { ghost: 99, ..a },
            Owner {
                participant: 2,
                ..a
            },
        ] {
            assert!(t.receive(bad, Some(1), 8000, &input(a, START)).is_err());
        }
        let wrong_bus = Message::Reached {
            event: START,
            target: EntityRef {
                ghost: a.ghost,
                entity: 1,
            },
            player: 0,
        };
        assert!(t.receive(a, Some(1), 8000, &wrong_bus).is_err());
        assert!(t.receive(a, Some(1), 6999, &input(a, START)).is_err());
        assert_eq!(t, before);
        for participant in 3..=128 {
            receive(&mut t, owner(participant, 0), STOP, 0);
        }
        let full = t.clone();
        let extra = owner(129, 0);
        assert_eq!(
            t.receive(extra, Some(1), 0, &input(extra, START)),
            Err(Error::Bound)
        );
        assert_eq!(t, full);
    }
}
