// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{definition, identity, inert, plan};
use super::*;

fn fixture() -> Arc<Plan> {
    let mut d = definition();
    d.bindings.integers.push(identity(80));
    d.bindings
        .distances
        .push(data::bindings::distance::Definition {
            identity: identity(90),
            state: data::Endpoint {
                serializer: 2,
                port: 0,
            },
            output: IntegerSource::Local(identity(80)),
            route: Link {
                identity: identity(70),
                port: 0,
                class: "ChannelWaypointListProviderEntityData".into(),
                channel: None,
            },
        });
    d.bindings.sorters.push(data::bindings::IntegerSorter {
        identity: identity(91),
        source: IntegerSource::Local(identity(80)),
        order: nfs_world::scoring::rank_sorter::Order::Ascending,
    });
    d.bindings.order = vec![
        Observer::Transition(4),
        Observer::Distance(0),
        Observer::Sorter(0),
    ];
    plan(d)
}

#[test]
fn first_update_initializes_total_then_members_with_controllables_publish_remaining_distance() {
    let route = Route::new(&[100., 200.], 30., 50., 2).unwrap();
    let (mut host, effects) =
        Host::from_scene(fixture(), SceneInputs::default(), &mut inert).unwrap();
    assert!(matches!(
        effects.as_slice(),
        [super::super::Effect::Distance(Effect {
            total_metres: 0.,
            ..
        })]
    ));
    host.apply(
        &[
            Action::Enter {
                state: 2,
                participant: 7,
            },
            Action::Enter {
                state: 2,
                participant: 8,
            },
        ],
        &mut inert,
    )
    .unwrap();
    let values = [
        Measurement {
            participant: 7,
            next: 1,
            lap: 1,
            distance_to_next: Some(20.),
        },
        Measurement {
            participant: 8,
            next: 1,
            lap: 1,
            distance_to_next: None,
        },
    ];
    let effects = host
        .update_distance(0, Some(&route), &values, &mut inert)
        .unwrap();
    assert!(matches!(
        effects.as_slice(),
        [super::super::Effect::Distance(Effect {
            total_metres: 650.,
            ..
        })]
    ));
    assert!(host.ranking(0).unwrap().is_empty());
    host.update_distance(0, None, &values, &mut inert).unwrap();
    assert_eq!(
        host.integer_value(&IntegerSource::Local(identity(80)), 7),
        Some(570_000)
    );
    assert_eq!(
        host.integer_value(&IntegerSource::Local(identity(80)), 8),
        None
    );
    let finished = [Measurement {
        lap: 3,
        ..values[0]
    }];
    host.update_distance(0, None, &finished, &mut inert)
        .unwrap();
    assert_eq!(host.ranking(0).unwrap()[0].value, 0);
    host.apply(
        &[Action::Move {
            source: 2,
            participant: 7,
            target: None,
        }],
        &mut inert,
    )
    .unwrap();
    host.update_distance(0, None, &values, &mut inert).unwrap();
    assert_eq!(host.ranking(0).unwrap()[0].value, 0);
}

#[test]
fn initialization_and_rank_callbacks_fail_atomically_and_repeat_updates_do_not_rewrite() {
    let route = Route::new(&[100.], 0., 0., 1).unwrap();
    let (mut host, _) = Host::from_scene(fixture(), SceneInputs::default(), &mut inert).unwrap();
    assert_eq!(
        host.update_distance(0, Some(&route), &[], &mut |_, _| Err(Error::Invalid)),
        Err(Error::Invalid)
    );
    assert_eq!(
        host.update_distance(0, None, &[], &mut inert),
        Err(Error::Invalid)
    );
    host.update_distance(0, Some(&route), &[], &mut inert)
        .unwrap();
    host.apply(
        &[Action::Enter {
            state: 2,
            participant: 7,
        }],
        &mut inert,
    )
    .unwrap();
    let values = [Measurement {
        participant: 7,
        next: 0,
        lap: 1,
        distance_to_next: Some(10.),
    }];
    assert_eq!(
        host.update_distance(0, None, &values, &mut |_, _| Err(Error::Invalid)),
        Err(Error::Invalid)
    );
    assert!(host.ranking(0).unwrap().is_empty());
    host.update_distance(0, None, &values, &mut inert).unwrap();
    assert!(
        host.update_distance(0, None, &values, &mut inert)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        host.update_distance(0, None, &[values[0], values[0]], &mut inert),
        Err(Error::Bound)
    );
}
