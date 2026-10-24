// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
mod graphs;

fn scene() -> data::Lifecycle {
    let mut data = super::super::tests::definition();
    data.transitions.clear();
    data.registered_transitions.clear();
    for state in &mut data.states {
        state.kind = data::StateKind::Updateable;
    }
    data
}
fn mode() -> Arc<Plan> {
    let mut data = scene();
    let endpoint = data::Endpoint {
        serializer: 20,
        port: 17,
    };
    let mut imported = data.states[0].clone();
    imported.kind = data::StateKind::External;
    data.imports.insert(
        endpoint,
        vec![data::ImportedState {
            identity: super::super::tests::identity(31),
            class: "ParticipantStateEntityData".into(),
            scene_content_key: 77,
            state: imported,
        }],
    );
    data.bindings
        .properties
        .push(data::bindings::StateProperty {
            identity: super::super::tests::identity(30),
            definition: properties::Definition::List(vec![endpoint]),
        });
    data.bindings.order.push(Observer::Property(0));
    Arc::new(Plan::installed(&data).unwrap())
}
fn command(instance: Instance, value: Command) -> Scoped<Command> {
    Scoped { instance, value }
}
fn state(instance: Instance, action: Action) -> Scoped<Command> {
    command(instance, Command::State(action))
}
fn activate(instance: Instance) -> Scoped<Command> {
    command(
        instance,
        Command::Activate {
            observer: Observer::Property(0),
            binding: false,
        },
    )
}
fn dispatch(world: &mut WorldHost, commands: Vec<Scoped<Command>>) -> Vec<Publication> {
    world.dispatch(commands, &mut |_, _| Ok(vec![])).unwrap()
}
fn setup() -> (WorldHost, Instance, Instance, Instance) {
    let mut world = WorldHost::default();
    let scene = world
        .install(Arc::new(Plan::scene(&scene().scope()).unwrap()), Some(77))
        .unwrap();
    let first = world.install(mode(), None).unwrap();
    let second = world.install(mode(), None).unwrap();
    (world, scene, first, second)
}

#[test]
fn two_modes_bind_live_scene_membership_and_emit_only_the_scene_serializer() {
    let (mut world, scene, first, second) = setup();
    dispatch(
        &mut world,
        vec![state(
            scene,
            Action::Enter {
                state: 1,
                participant: 91,
            },
        )],
    );
    let seeded = dispatch(&mut world, vec![activate(first), activate(second)]);
    assert_eq!(
        seeded
            .iter()
            .filter(|e| matches!(e.value, Effect::Boolean { .. }))
            .count(),
        2
    );
    assert!(!seeded.iter().any(|e| matches!(e.value, Effect::State(_))));
    for instance in [first, second] {
        assert!(
            world
                .view(instance)
                .unwrap()
                .members(4)
                .unwrap()
                .contains(&91)
        );
        assert_eq!(
            world.boolean(instance, &BooleanSource::Local(0), 91),
            Ok(Some(true))
        );
        assert!(world.view(instance).unwrap().members(1).unwrap().is_empty());
    }
    let effects = dispatch(
        &mut world,
        vec![state(
            first,
            Action::Move {
                source: 4,
                participant: 91,
                target: Some(2),
            },
        )],
    );
    assert!(world.view(scene).unwrap().members(1).unwrap().is_empty());
    assert!(world.view(first).unwrap().members(2).unwrap().contains(&91));
    assert!(world.view(second).unwrap().members(2).unwrap().is_empty());
    for instance in [first, second] {
        assert_eq!(
            world.boolean(instance, &BooleanSource::Local(0), 91),
            Ok(Some(false))
        );
    }
    assert!(
        effects
            .iter()
            .filter(|e| matches!(e.value, Effect::State(Notification::Left { .. })))
            .all(|e| e.instance == scene
                && matches!(e.value, Effect::State(Notification::Left { state: 1, .. })))
    );
}

#[test]
fn listeners_follow_activation_order_and_live_append_during_callback() {
    let (mut world, scene, first, second) = setup();
    dispatch(&mut world, vec![activate(second)]);
    let mut seen = Vec::new();
    world
        .dispatch(
            vec![state(
                scene,
                Action::Enter {
                    state: 1,
                    participant: 7,
                },
            )],
            &mut |execution, e| {
                if matches!(
                    e.value,
                    Effect::Boolean {
                        change: properties::Change { value: true, .. },
                        ..
                    }
                ) {
                    seen.push(e.instance);
                    assert!(execution.view(first)?.members(4)?.contains(&7));
                    if e.instance == second {
                        return Ok(vec![activate(first)]);
                    }
                }
                Ok(vec![])
            },
        )
        .unwrap();
    assert_eq!(seen, vec![second, first]);
    // Activation seeds first immediately; its later live-vector callback is
    // still visited but the Boolean latch emits no duplicate value.
    dispatch(
        &mut world,
        vec![state(
            scene,
            Action::Move {
                source: 1,
                participant: 7,
                target: Some(2),
            },
        )],
    );
    let effects = dispatch(
        &mut world,
        vec![state(
            scene,
            Action::Enter {
                state: 1,
                participant: 8,
            },
        )],
    );
    let order: Vec<_> = effects
        .iter()
        .filter_map(|e| matches!(e.value, Effect::Boolean { .. }).then_some(e.instance))
        .collect();
    assert_eq!(order, vec![second, first]);
}

#[test]
fn admission_failure_rolls_back_shared_membership_observers_and_nested_moves() {
    let (mut world, scene, first, second) = setup();
    dispatch(&mut world, vec![activate(first), activate(second)]);
    let result = world.dispatch(
        vec![state(
            first,
            Action::Enter {
                state: 4,
                participant: 8,
            },
        )],
        &mut |_, e| {
            if e.instance == first && matches!(e.value, Effect::Boolean { .. }) {
                return Ok(vec![state(
                    second,
                    Action::Move {
                        source: 4,
                        participant: 8,
                        target: Some(2),
                    },
                )]);
            }
            if e.instance == second
                && matches!(
                    e.value,
                    Effect::State(Notification::Entered { state: 2, .. })
                )
            {
                return Err(Error::Bound);
            }
            Ok(vec![])
        },
    );
    assert_eq!(result, Err(Error::Bound));
    assert!(world.view(scene).unwrap().members(1).unwrap().is_empty());
    assert!(world.view(second).unwrap().members(2).unwrap().is_empty());
    for instance in [first, second] {
        assert_eq!(
            world.boolean(instance, &BooleanSource::Local(0), 8),
            Ok(None)
        );
    }
    dispatch(
        &mut world,
        vec![state(
            scene,
            Action::Enter {
                state: 1,
                participant: 8,
            },
        )],
    );
    assert_eq!(
        world.boolean(first, &BooleanSource::Local(0), 8),
        Ok(Some(true))
    );
}

#[test]
fn retirement_preserves_import_owners_and_rejects_stale_instance_commands() {
    let (mut world, scene, first, second) = setup();
    assert_eq!(world.retire(scene), Err(Error::Invalid));
    dispatch(
        &mut world,
        vec![
            activate(first),
            state(
                first,
                Action::Enter {
                    state: 2,
                    participant: 5,
                },
            ),
        ],
    );
    assert_eq!(world.retire(first), Err(Error::Invalid));
    dispatch(
        &mut world,
        vec![state(first, Action::RemoveAll { state: 2 })],
    );
    world.retire(first).unwrap();
    let replacement = world.install(mode(), None).unwrap();
    assert_ne!(replacement, first);
    assert!(matches!(
        world.begin(vec![state(
            first,
            Action::Enter {
                state: 4,
                participant: 99
            }
        )]),
        Err(Error::UnknownState)
    ));
    dispatch(
        &mut world,
        vec![
            activate(second),
            activate(replacement),
            state(
                scene,
                Action::Enter {
                    state: 1,
                    participant: 99,
                },
            ),
        ],
    );
    assert_eq!(
        world.boolean(replacement, &BooleanSource::Local(0), 99),
        Ok(Some(true))
    );
    assert!(world.view(scene).unwrap().members(1).unwrap().contains(&99));
}

#[test]
fn missing_scene_and_duplicate_scene_install_are_atomic() {
    let mut world = WorldHost::default();
    assert_eq!(world.install(mode(), None), Err(Error::UnknownState));
    let plan = Arc::new(Plan::scene(&scene().scope()).unwrap());
    let scene = world.install(plan.clone(), Some(77)).unwrap();
    assert_eq!(scene.value(), 1);
    assert_eq!(world.install(plan, Some(77)), Err(Error::Invalid));
    let instance = world.install(mode(), None).unwrap();
    assert_eq!(instance.value(), 2);
}

#[test]
fn many_scene_subscribers_iterate_without_consuming_native_reentry_depth() {
    let (mut world, scene, first, second) = setup();
    let mut instances = vec![first, second];
    for _ in 2..100 {
        instances.push(world.install(mode(), None).unwrap());
    }
    dispatch(
        &mut world,
        instances.iter().copied().map(activate).collect(),
    );
    let effects = dispatch(
        &mut world,
        vec![state(
            scene,
            Action::Enter {
                state: 1,
                participant: 7,
            },
        )],
    );
    let notified: Vec<_> = effects
        .iter()
        .filter_map(|e| matches!(e.value, Effect::Boolean { .. }).then_some(e.instance))
        .collect();
    assert_eq!(notified, instances);
}

#[test]
fn scene_and_property_only_scopes_have_no_race_root() {
    let mut world = WorldHost::default();
    let plan = Arc::new(Plan::scene(&data::Scope::default()).unwrap());
    let empty = world.install(plan, Some(9)).unwrap();
    assert!(world.plan(empty).unwrap().root().is_none());
    assert!(dispatch(&mut world, vec![]).is_empty());
    world.retire(empty).unwrap();
    let mode = Arc::new(Plan::new(&scene(), |_| Err(Error::Invalid)).unwrap());
    assert!(mode.root().is_some());
    assert_eq!(world.install(mode, Some(9)), Err(Error::Invalid));
}
