// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! World-clock objective delays. Reconciliation reads committed account state;
//! expiry only queues a transaction and never marks an objective completed.
use super::{Failure, Objective};
use nfs_content::progression::delays::Delay;
use nfs_world::replication::Error;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn validate(definitions: &[Delay], known: &BTreeSet<u64>) -> Result<(), Failure> {
    if definitions.len() > 64 {
        return Err(Failure::BodyLimit);
    }
    let mut ids = BTreeSet::new();
    for definition in definitions {
        if !known.contains(&definition.objective)
            || !ids.insert(definition.objective)
            || definition.duration > std::time::Duration::from_secs(4800)
        {
            return Err(Failure::ProfileConfig);
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Timer {
    deadline_ms: u64,
    queued: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct State {
    generation: Option<u64>,
    last_ms: Option<u64>,
    timers: BTreeMap<u64, Timer>,
}
impl State {
    pub(crate) fn generation(&self) -> Option<u64> {
        self.generation
    }
    pub(crate) fn advance(
        &mut self,
        definitions: &[Delay],
        generation: u64,
        now_ms: u64,
        mut objective: impl FnMut(u64) -> Option<Objective>,
        capacity: usize,
    ) -> Result<Vec<u64>, Error> {
        if definitions.len() > 64
            || self.last_ms.is_some_and(|last| now_ms < last)
            || self
                .generation
                .is_some_and(|previous| generation < previous)
        {
            return Err(Error::Shape);
        }
        let mut next = self.clone();
        if next.generation != Some(generation) {
            for definition in definitions {
                let state = objective(definition.objective).ok_or(Error::UnknownObject)?;
                if state.active && !state.completed {
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        next.timers.entry(definition.objective)
                    {
                        let nanos = definition.duration.as_nanos();
                        let millis =
                            u64::try_from(nanos.div_ceil(1_000_000)).map_err(|_| Error::Bound)?;
                        let deadline_ms = now_ms.checked_add(millis).ok_or(Error::Bound)?;
                        entry.insert(Timer {
                            deadline_ms,
                            queued: false,
                        });
                    }
                } else {
                    next.timers.remove(&definition.objective);
                }
            }
            next.generation = Some(generation);
        }
        let mut due = Vec::new();
        for (&id, timer) in &mut next.timers {
            if due.len() >= capacity {
                break;
            }
            if !timer.queued && now_ms >= timer.deadline_ms {
                timer.queued = true;
                due.push(id);
            }
        }
        next.last_ms = Some(now_ms);
        *self = next;
        Ok(due)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn definitions() -> Vec<Delay> {
        vec![
            Delay {
                objective: 1,
                duration: Duration::from_millis(1500),
            },
            Delay {
                objective: 2,
                duration: Duration::from_millis(2500),
            },
        ]
    }
    fn active(_: u64) -> Option<Objective> {
        Some(Objective {
            active: true,
            completed: false,
        })
    }
    #[test]
    fn due_work_waits_for_capacity_and_completion_is_only_external() {
        let mut state = State::default();
        let definitions = definitions();
        assert!(
            state
                .advance(&definitions, 4, 100, active, 2)
                .unwrap()
                .is_empty()
        );
        assert!(
            state
                .advance(
                    &definitions,
                    4,
                    1599,
                    |_| panic!("unchanged snapshot re-read"),
                    2
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            state
                .advance(&definitions, 4, 1600, active, 0)
                .unwrap()
                .is_empty()
        );
        // Both elapsed, but the output queue can accept only one.
        assert_eq!(
            state.advance(&definitions, 4, 3000, active, 1).unwrap(),
            [1]
        );
        assert_eq!(
            state.advance(&definitions, 4, 3001, active, 1).unwrap(),
            [2]
        );
        assert!(
            state
                .advance(&definitions, 4, 4000, active, 2)
                .unwrap()
                .is_empty()
        );
        assert_eq!(state.timers.len(), 2); // queued is not completed
        state
            .advance(
                &definitions,
                5,
                4001,
                |_| {
                    Some(Objective {
                        active: false,
                        completed: true,
                    })
                },
                2,
            )
            .unwrap();
        assert!(state.timers.is_empty());
    }
    #[test]
    fn unrelated_commits_do_not_restart_delays_and_cancellation_does() {
        let mut state = State::default();
        let definitions = definitions();
        state.advance(&definitions, 1, 0, active, 2).unwrap();
        state.advance(&definitions, 2, 1000, active, 2).unwrap();
        assert_eq!(
            state.advance(&definitions, 2, 1500, active, 2).unwrap(),
            [1]
        );
        state
            .advance(
                &definitions,
                3,
                1600,
                |_| {
                    Some(Objective {
                        active: false,
                        completed: false,
                    })
                },
                2,
            )
            .unwrap();
        state.advance(&definitions, 4, 2000, active, 2).unwrap();
        assert!(
            state
                .advance(&definitions, 4, 3499, active, 2)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            state.advance(&definitions, 4, 3500, active, 2).unwrap(),
            [1]
        );
        // Another connection has its own activation clock; no wall-clock time
        // or elapsed state is invented from the persisted active flag.
        let mut other = State::default();
        assert!(
            other
                .advance(&definitions, 4, 3500, active, 2)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn invalid_state_or_clock_is_atomic_and_definitions_name_known_main_objectives() {
        let definitions = definitions();
        let mut state = State::default();
        let before = state.clone();
        assert!(
            state
                .advance(
                    &definitions,
                    1,
                    10,
                    |id| (id == 1).then(|| active(id).unwrap()),
                    2
                )
                .is_err()
        );
        assert_eq!(state, before);
        state.advance(&definitions, 2, 100, active, 2).unwrap();
        let before = state.clone();
        assert!(state.advance(&definitions, 2, 99, active, 2).is_err());
        assert_eq!(state, before);
        assert!(state.advance(&definitions, 1, 101, active, 2).is_err());
        assert_eq!(state, before);
        assert!(validate(&definitions, &BTreeSet::from([1, 2])).is_ok());
        assert!(validate(&definitions, &BTreeSet::from([1])).is_err());
        assert!(validate(&[definitions[0], definitions[0]], &BTreeSet::from([1])).is_err());
    }

    #[test]
    fn fractional_milliseconds_never_fire_early_and_new_deadlines_check_overflow() {
        let definitions = [Delay {
            objective: 1,
            duration: Duration::from_nanos(1),
        }];
        let mut state = State::default();
        assert!(
            state
                .advance(&definitions, 1, 10, active, 1)
                .unwrap()
                .is_empty()
        );
        assert!(
            state
                .advance(&definitions, 1, 10, active, 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(state.advance(&definitions, 1, 11, active, 1).unwrap(), [1]);
        // A newer snapshot must not calculate a replacement deadline for an
        // existing timer, even at the clock's representable end.
        assert!(
            state
                .advance(&definitions, 2, u64::MAX, active, 1)
                .unwrap()
                .is_empty()
        );
        let mut fresh = State::default();
        let before = fresh.clone();
        assert_eq!(
            fresh.advance(&definitions, 1, u64::MAX, active, 1),
            Err(Error::Bound)
        );
        assert_eq!(fresh, before);
    }
}
