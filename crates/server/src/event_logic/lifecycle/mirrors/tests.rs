// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{definition, identity, inert, plan};
use super::*;
use data::bindings::{IntegerSorter, mirrors as authored};
use nfs_world::scoring::rank_sorter::Order;

fn fixture() -> Arc<Plan> {
    let mut d = definition();
    d.transitions.clear();
    d.bindings.integers = vec![identity(80), identity(81), identity(82)];
    d.bindings.mirror_managers = vec![identity(70)];
    d.bindings.mirrors = vec![authored::Mirror {
        identity: identity(81),
        identifier: 12,
        manager: Some(Manager::Local(identity(70))),
    }];
    d.bindings.sorters = vec![IntegerSorter {
        identity: identity(90),
        source: IntegerSource::Local(identity(81)),
        order: Order::Descending,
    }];
    d.bindings.order = vec![Observer::Sorter(0), Observer::Mirror(0)];
    plan(d)
}
fn provider(instance: u64, source: usize) -> Provider {
    Provider {
        id: ProviderId {
            instance,
            identity: identity(60),
        },
        manager: Manager::Local(identity(70)),
        identifier: 12,
        source: IntegerSource::Local(identity(source)),
    }
}
fn source(index: usize) -> IntegerSource {
    IntegerSource::Local(identity(index))
}

#[test]
fn mirrors_copy_then_publish_bound_and_keep_source_removal_distinct_from_participant_destruction() {
    let (mut h, effects) = Host::from_scene(fixture(), SceneInputs::default(), &mut inert).unwrap();
    assert!(matches!(
        effects.as_slice(),
        [super::super::Effect::Mirror(Effect { bound: false, .. })]
    ));
    h.integer(source(80), 7, 100, &mut inert).unwrap();
    let effects = h
        .register_mirror_provider(provider(1, 80), &mut inert)
        .unwrap();
    assert!(matches!(
        effects.as_slice(),
        [
            super::super::Effect::Ranking(_),
            super::super::Effect::Mirror(Effect { bound: true, .. })
        ]
    ));
    assert_eq!(h.integer_value(&source(81), 7), Some(100));
    h.integer(source(80), 7, 200, &mut inert).unwrap();
    assert_eq!(h.integer_value(&source(81), 7), Some(200));
    h.unregister_mirror_provider(&provider(1, 80).id, &mut inert)
        .unwrap();
    assert_eq!(h.integer_value(&source(81), 7), None);
    assert_eq!(h.ranking(0).unwrap()[0].value, 200);
    h.integer(source(80), 7, 300, &mut inert).unwrap();
    assert_eq!(h.integer_value(&source(81), 7), None);
    h.remove_integer_participant(7, &mut inert).unwrap();
    assert!(h.ranking(0).unwrap().is_empty());
}

#[test]
fn unregister_does_not_fall_back_but_a_later_matching_registration_binds() {
    let (mut h, _) = Host::from_scene(fixture(), SceneInputs::default(), &mut inert).unwrap();
    h.integer(source(80), 7, 100, &mut inert).unwrap();
    h.integer(source(82), 7, 900, &mut inert).unwrap();
    h.register_mirror_provider(provider(1, 80), &mut inert)
        .unwrap();
    h.register_mirror_provider(provider(2, 82), &mut inert)
        .unwrap();
    assert_eq!(h.integer_value(&source(81), 7), Some(100));
    h.unregister_mirror_provider(&provider(1, 80).id, &mut inert)
        .unwrap();
    assert_eq!(h.mirror_source(0), None);
    h.register_mirror_provider(provider(3, 82), &mut inert)
        .unwrap();
    assert_eq!(h.integer_value(&source(81), 7), Some(900));
    assert_eq!(
        h.register_mirror_provider(provider(3, 82), &mut inert),
        Err(Error::Invalid)
    );
    h.destroy_integer_source(&source(82), &mut inert).unwrap();
    assert_eq!(h.mirror_source(0), None);
    assert_eq!(
        h.integer(source(82), 7, 950, &mut inert),
        Err(Error::Invalid)
    );
    assert_eq!(
        h.register_mirror_provider(provider(4, 82), &mut inert),
        Err(Error::Invalid)
    );
}

#[test]
fn failed_binding_rolls_back_and_managers_and_hosts_are_isolated() {
    let (mut h, _) = Host::from_scene(fixture(), SceneInputs::default(), &mut inert).unwrap();
    h.integer(source(80), 7, 100, &mut inert).unwrap();
    assert_eq!(
        h.register_mirror_provider(provider(1, 80), &mut |_, _| Err(Error::Invalid)),
        Err(Error::Invalid)
    );
    assert!(h.mirror_source(0).is_none());
    assert!(h.ranking(0).unwrap().is_empty());
    h.register_mirror_provider(provider(1, 80), &mut inert)
        .unwrap();
    let (other, _) = Host::from_scene(fixture(), SceneInputs::default(), &mut inert).unwrap();
    assert!(other.mirror_source(0).is_none());
    h.destroy_mirror_manager(&Manager::Local(identity(70)), &mut inert)
        .unwrap();
    assert!(h.mirror_source(0).is_none());
    assert_eq!(
        h.register_mirror_provider(provider(2, 80), &mut inert),
        Err(Error::Invalid)
    );
}

#[test]
fn activation_queries_existing_providers_and_sorters_subscribe_without_seeding() {
    let mut d = fixture().data().clone();
    d.bindings.mirror_providers.push(authored::Provider {
        identity: identity(60),
        identifier: 12,
        manager: Some(Manager::Local(identity(70))),
        source: Some(source(80)),
    });
    d.bindings.order = vec![
        Observer::MirrorProvider(0),
        Observer::Mirror(0),
        Observer::Sorter(0),
    ];
    let (mut h, effects) = Host::from_scene(
        Arc::new(Plan::scene(&d).unwrap()),
        SceneInputs {
            integers: BTreeMap::from([((source(80), 7), 100)]),
            ..Default::default()
        },
        &mut inert,
    )
    .unwrap();
    assert_eq!(h.integer_value(&source(81), 7), Some(100));
    assert!(h.ranking(0).unwrap().is_empty());
    assert!(matches!(
        effects.as_slice(),
        [super::super::Effect::Mirror(Effect { bound: true, .. })]
    ));
    h.integer(source(80), 7, 101, &mut inert).unwrap();
    assert_eq!(h.ranking(0).unwrap()[0].value, 101);
}

#[test]
fn changes_during_seed_copy_finish_before_mirror_subscription() {
    use super::super::commands::Command;
    let mut d = fixture().data().clone();
    d.bindings.mirror_providers.push(authored::Provider {
        identity: identity(60),
        identifier: 12,
        manager: Some(Manager::Local(identity(70))),
        source: Some(source(80)),
    });
    d.bindings.order = vec![
        Observer::Sorter(0),
        Observer::MirrorProvider(0),
        Observer::Mirror(0),
    ];
    let (mut h, _) = Host::from_scene_with_commands(
        Arc::new(Plan::scene(&d).unwrap()),
        SceneInputs {
            integers: BTreeMap::from([((source(80), 7), 100)]),
            ..Default::default()
        },
        &mut |_, effect| {
            if matches!(effect, super::super::Effect::Ranking(_)) {
                Ok(vec![Command::Integer {
                    source: source(80),
                    participant: 7,
                    value: 200,
                }])
            } else {
                Ok(vec![])
            }
        },
    )
    .unwrap();
    assert_eq!(h.integer_value(&source(80), 7), Some(200));
    assert_eq!(h.integer_value(&source(81), 7), Some(100));
    h.integer(source(80), 7, 201, &mut inert).unwrap();
    assert_eq!(h.integer_value(&source(81), 7), Some(201));
}
