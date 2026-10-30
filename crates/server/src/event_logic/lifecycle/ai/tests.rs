// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{definition, identity, inert, plan};
use super::*;
use data::bindings::ai as authored;

fn fixture(one_by_one: bool) -> Arc<Plan> {
    let mut d = definition();
    d.transitions.clear();
    d.bindings.ai_commands = BTreeMap::from([
        (
            identity(90),
            authored::Command::Wander {
                urgent: true,
                prefer_straight: false,
            },
        ),
        (
            identity(91),
            authored::Command::Wander {
                urgent: false,
                prefer_straight: true,
            },
        ),
    ]);
    let manager = Property {
        channel: "actors".into(),
        port: 1,
    };
    d.bindings.ai_controls.push(authored::Control {
        identity: identity(80),
        context_hash: 123,
        one_by_one,
        apply_existing: false,
        states: vec![
            data::Endpoint {
                serializer: 1,
                port: 0,
            },
            data::Endpoint {
                serializer: 2,
                port: 0,
            },
        ],
        commands: vec![identity(90), identity(91)],
        command_manager: manager.clone(),
        actor_manager: manager,
        game_mode: None,
    });
    d.bindings.order = vec![Observer::Ai(0)];
    plan(d)
}
fn ai(effects: &[super::super::Effect]) -> Vec<Effect> {
    effects
        .iter()
        .filter_map(|e| match e {
            super::super::Effect::Ai(e) => Some(e.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn controller_keeps_context_across_watched_states_and_releases_before_target_entry() {
    let mut h = Host::new(fixture(false)).unwrap();
    let effects = h
        .apply(
            &[Action::Enter {
                state: 1,
                participant: 7,
            }],
            &mut inert,
        )
        .unwrap();
    assert!(
        matches!(ai(&effects).as_slice(), [Effect::Event { event: ENTERED, .. }, Effect::Start { token: 1, commands, .. }] if commands == &[identity(90), identity(91)])
    );
    let effects = h
        .apply(
            &[Action::Move {
                source: 1,
                participant: 7,
                target: Some(2),
            }],
            &mut inert,
        )
        .unwrap();
    assert!(ai(&effects).is_empty());
    assert_eq!(h.ai_context(0, 7), Some(1));
    let effects = h
        .apply(
            &[Action::Move {
                source: 2,
                participant: 7,
                target: Some(3),
            }],
            &mut inert,
        )
        .unwrap();
    let release = effects
        .iter()
        .position(|e| {
            matches!(
                e,
                super::super::Effect::Ai(Effect::Release { token: 1, .. })
            )
        })
        .unwrap();
    let entry = effects
        .iter()
        .position(|e| {
            matches!(
                e,
                super::super::Effect::State(Notification::Entered { state: 3, .. })
            )
        })
        .unwrap();
    assert!(release < entry);
    assert_eq!(h.ai_context(0, 7), None);
}

#[test]
fn participant_entry_callbacks_finish_before_command_creation_and_next_is_scoped() {
    use commands::Command;
    let mut h = Host::new(fixture(true)).unwrap();
    let effects = h
        .dispatch(
            vec![Command::State(Action::Enter {
                state: 2,
                participant: 7,
            })],
            &mut |_, e| {
                Ok(
                    if matches!(e, super::super::Effect::Ai(Effect::Event { .. })) {
                        vec![Command::Ai {
                            identity: identity(80),
                            event: NEXT,
                        }]
                    } else {
                        vec![]
                    },
                )
            },
        )
        .unwrap();
    assert!(
        matches!(ai(&effects).as_slice(), [Effect::Event { .. }, Effect::Start { token: 1, commands, .. }] if commands == &[identity(91)])
    );
    let effects = h
        .dispatch(
            vec![Command::Ai {
                identity: identity(80),
                event: NEXT,
            }],
            &mut |_, _| Ok(vec![]),
        )
        .unwrap();
    assert!(
        matches!(ai(&effects).as_slice(), [Effect::Release { token: 1, .. }, Effect::Start { token: 2, commands, .. }] if commands == &[identity(90)])
    );
    let other = Host::new(fixture(true)).unwrap();
    assert_eq!(other.ai_context(0, 7), None);
}

#[test]
fn failed_creation_and_retirement_are_atomic_and_retirement_is_idempotent() {
    let mut h = Host::new(fixture(false)).unwrap();
    let action = Action::Enter {
        state: 2,
        participant: 7,
    };
    assert_eq!(
        h.apply(std::slice::from_ref(&action), &mut |_, e| {
            if matches!(e, super::super::Effect::Ai(Effect::Start { .. })) {
                Err(Error::Invalid)
            } else {
                Ok(vec![])
            }
        }),
        Err(Error::Invalid)
    );
    assert_eq!(h.ai_context(0, 7), None);
    h.apply(&[action], &mut inert).unwrap();
    assert_eq!(
        h.retire_ai(&mut |_, _| Err(Error::Invalid)),
        Err(Error::Invalid)
    );
    assert_eq!(h.ai_context(0, 7), Some(1));
    assert_eq!(ai(&h.retire_ai(&mut inert).unwrap()).len(), 1);
    assert!(h.retire_ai(&mut inert).unwrap().is_empty());
    assert_eq!(h.ai_context(0, 7), None);
}

#[test]
fn source_aliases_seed_only_when_requested_and_without_entry_events() {
    let mut d = fixture(false).data().clone();
    d.bindings.ai_controls[0].apply_existing = true;
    // The normal scene snapshot only seeds imported states. This alias models
    // that already-existing scene membership and retains external hooks.
    let mut p = Plan::scene(&d).unwrap();
    p.external.insert(
        2,
        data::ImportedState {
            identity: identity(200),
            scene_content_key: 1,
            class: "WaypointsGameModeStateEntityData".into(),
            state: p.data.states[1].clone(),
        },
    );
    let (h, effects) = Host::from_scene(
        Arc::new(p),
        SceneInputs {
            members: vec![(2, 7, false)],
            ..Default::default()
        },
        &mut inert,
    )
    .unwrap();
    assert!(matches!(
        ai(&effects).as_slice(),
        [Effect::Start { token: 1, .. }]
    ));
    assert_eq!(h.ai_context(0, 7), Some(1));
}
