// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{identity, ranked};
use super::*;

#[derive(Clone)]
struct TimingEdge {
    fail: bool,
    finishes: Vec<(u64, u32)>,
}
impl super::super::graphs::Driver for TimingEdge {
    fn invoke(
        &mut self,
        _: nfs_world::scoring::events::native::Request,
        _: &mut dyn super::super::graphs::Reader,
    ) -> Result<Vec<super::super::graphs::Step>, nfs_world::scoring::Error> {
        Err(nfs_world::scoring::Error::UnsupportedEvent)
    }
    fn effect(
        &mut self,
        effect: &Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn super::super::graphs::Reader,
    ) -> Result<Vec<super::super::graphs::Step>, nfs_world::scoring::Error> {
        use super::super::{graphs::Step, tests::clock};
        let Effect::Timing(timed::Effect::Finish {
            participant,
            position,
            ..
        }) = effect
        else {
            return Ok(vec![]);
        };
        self.finishes.push((*participant, *position));
        if *participant == 8 && self.fail {
            return Err(nfs_world::scoring::Error::UnsupportedEvent);
        }
        Ok(if *participant == 7 {
            vec![Step::Lifecycle(vec![
                Command::State(Action::Enter {
                    state: 2,
                    participant: 8,
                }),
                Command::Timing(timed::Command::Waypoint {
                    source: 2,
                    participant: 8,
                    lap: 3,
                    seconds: 2.,
                }),
                Command::Timing(timed::Command::Clocks(clock(1.5, 0.))),
            ])]
        } else {
            vec![]
        })
    }
}

#[test]
fn finish_update_resumes_after_nested_graph_calls_and_rolls_back_clocks_queue_and_positions() {
    use super::super::{
        graphs::{Runtime, Step},
        tests::{clock, inert, plan, timed_definition},
    };
    for fail in [false, true] {
        let mut host = Host::new(plan(timed_definition())).unwrap();
        host.clocks(clock(0., 0.)).unwrap();
        host.apply(
            &[Action::Enter {
                state: 2,
                participant: 7,
            }],
            &mut inert,
        )
        .unwrap();
        host.waypoint(2, 7, 3, 1., &mut inert).unwrap();
        host.clocks(clock(0.5, 0.)).unwrap();
        let mut runtime = Runtime::new(
            host,
            TimingEdge {
                fail,
                finishes: vec![],
            },
        );
        let result = runtime.execute(vec![Step::Lifecycle(vec![Command::Timing(
            timed::Command::Transition(6),
        )])]);
        if fail {
            assert_eq!(
                result.unwrap_err(),
                nfs_world::scoring::Error::UnsupportedEvent
            );
            assert!(runtime.driver().finishes.is_empty());
            assert_eq!(
                runtime.lifecycle().machine().members(2).unwrap(),
                &BTreeSet::from([7])
            );
            assert!(runtime.lifecycle().machine().members(3).unwrap().is_empty());
            let mut recovered = runtime.lifecycle().clone();
            recovered.clocks(clock(0.5, 0.)).unwrap();
            let effects = recovered.update_transition(6, &mut inert).unwrap();
            assert!(effects.iter().any(|effect| matches!(
                effect,
                Effect::Timing(timed::Effect::Finish {
                    participant: 7,
                    position: 1,
                    ..
                })
            )));
        } else {
            result.unwrap();
            assert_eq!(runtime.driver().finishes, [(7, 1), (8, 2)]);
            assert!(runtime.lifecycle().machine().members(2).unwrap().is_empty());
            assert_eq!(
                runtime.lifecycle().machine().members(3).unwrap(),
                &BTreeSet::from([7, 8])
            );
        }
    }
}

fn write(value: i32) -> Command {
    Command::Integer {
        source: IntegerSource::Local(identity(80)),
        participant: 7,
        value,
    }
}

#[test]
fn graph_activation_subscribes_at_its_step_without_seeding_integer_sorters() {
    let mut host = Host::prepare(ranked(), SceneInputs::default()).unwrap();
    let activate = |index| Command::Activate {
        observer: Observer::Sorter(index),
        binding: false,
    };
    let mut no_commands =
        |_: &dyn nfs_world::participant_states::Membership, _: &Effect| Ok(vec![]);
    assert!(
        host.dispatch(vec![write(10)], &mut no_commands)
            .unwrap()
            .is_empty()
    );
    assert!(
        host.dispatch(vec![activate(0)], &mut no_commands)
            .unwrap()
            .is_empty()
    );
    assert!(host.ranking(0).unwrap().is_empty());
    let effects = host.dispatch(vec![write(20)], &mut no_commands).unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(host.ranking(0).unwrap()[0].value, 20);
    assert!(host.ranking(1).unwrap().is_empty());

    let mut execution = host.begin(vec![activate(1), write(30)]).unwrap();
    let mut effects = vec![];
    while let Some(Yield::Effect(effect)) = execution.advance().unwrap() {
        effects.push(effect);
        execution.resume(vec![]).unwrap();
    }
    let (mut host, _) = execution.finish().unwrap();
    assert_eq!(effects.len(), 2);
    assert_eq!(host.ranking(1).unwrap()[0].value, 30);
    // Repeated activation must not register another callback.
    assert_eq!(
        host.dispatch(vec![activate(1), write(40)], &mut no_commands)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        host.dispatch(vec![activate(99)], &mut no_commands),
        Err(Error::Invalid)
    );
    assert_eq!(host.ranking(0).unwrap()[0].value, 40);
}
fn callback(
    _: &dyn nfs_world::participant_states::Membership,
    effect: &Effect,
) -> Result<Vec<Command>, Error> {
    Ok(match effect {
        Effect::Ranking(e) if e.sorter == identity(81) && e.rows[0].value == 100 => {
            vec![write(200)]
        }
        Effect::Hook(Notification::Entered {
            state: 2,
            participant,
            ..
        }) => vec![Command::State(Action::Complete {
            state: 2,
            participant: *participant,
        })],
        _ => vec![],
    })
}

#[test]
fn resumable_host_matches_synchronous_nested_properties_and_state_hooks() {
    let initial = Host::new(ranked()).unwrap();
    let commands = vec![
        write(100),
        Command::State(Action::Enter {
            state: 1,
            participant: 7,
        }),
    ];
    let mut expected = initial.clone();
    let expected_effects = expected.dispatch(commands.clone(), &mut callback).unwrap();
    let mut execution = initial.begin(commands).unwrap();
    let mut effects = Vec::new();
    while let Some(step) = execution.advance().unwrap() {
        let Yield::Effect(effect) = step else {
            panic!("unexpected nested return");
        };
        let commands = callback(execution.machine(), &effect).unwrap();
        effects.push(effect);
        execution.resume(commands).unwrap();
    }
    let (actual, committed_effects) = execution.finish().unwrap();
    assert_eq!(effects, committed_effects);
    assert_eq!(effects, expected_effects);
    assert_eq!(actual.machine(), expected.machine());
    assert_eq!(actual.ranking(0), expected.ranking(0));
    assert_eq!(actual.ranking(1), expected.ranking(1));
    assert_eq!(
        initial.integer_value(&IntegerSource::Local(identity(80)), 7),
        None
    );
}

#[test]
fn incomplete_and_failed_host_executions_cannot_commit() {
    let initial = Host::new(ranked()).unwrap();
    let mut execution = initial.begin(vec![write(100)]).unwrap();
    execution.advance().unwrap().unwrap();
    assert_eq!(
        execution.integer_value(&IntegerSource::Local(identity(80)), 7),
        Some(100)
    );
    assert_eq!(
        initial.integer_value(&IntegerSource::Local(identity(80)), 7),
        None
    );
    assert!(matches!(execution.finish(), Err(Error::Invalid)));
    let mut execution = initial.begin(vec![write(100)]).unwrap();
    execution.advance().unwrap().unwrap();
    assert_eq!(
        execution.resume(vec![Command::State(Action::Invoke {
            observer: 0,
            token: PUBLICATION | 1
        })]),
        Err(Error::Invalid)
    );
    assert_eq!(execution.advance(), Err(Error::Invalid));
    assert!(matches!(execution.finish(), Err(Error::Invalid)));
}

#[test]
fn sequential_graph_calls_finish_inside_one_observer_without_accumulating_stack_depth() {
    let initial = Host::new(ranked()).unwrap();
    let mut execution = initial.begin(vec![write(100)]).unwrap();
    assert!(
        matches!(execution.advance().unwrap(), Some(Yield::Effect(Effect::Ranking(ref e))) if e.sorter == identity(81))
    );
    for value in 200..400 {
        let token = execution.call(vec![write(value)]).unwrap();
        let mut rows = Vec::new();
        loop {
            match execution.advance().unwrap().unwrap() {
                Yield::Effect(Effect::Ranking(e)) => {
                    rows.push((e.sorter.index, e.rows[0].value));
                    execution.resume(vec![]).unwrap();
                }
                Yield::Returned(id) => {
                    assert_eq!(id, token);
                    break;
                }
                effect => panic!("unexpected publication: {effect:?}"),
            }
        }
        assert_eq!(rows, [(81, value), (82, value)]);
        assert_eq!(
            execution.integer_value(&IntegerSource::Local(identity(80)), 7),
            Some(value)
        );
    }
    // Only now has the original graph callback finished. Its sibling receives
    // the outer property's original value argument, as in the native observer.
    execution.resume(vec![]).unwrap();
    assert!(
        matches!(execution.advance().unwrap(), Some(Yield::Effect(Effect::Ranking(ref e))) if e.sorter == identity(82) && e.rows[0].value == 100)
    );
    execution.resume(vec![]).unwrap();
    assert_eq!(execution.advance().unwrap(), None);
    let (host, _) = execution.finish().unwrap();
    assert_eq!(host.ranking(0).unwrap()[0].value, 399);
    assert_eq!(host.ranking(1).unwrap()[0].value, 100);
}
