// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn definition() -> Immediate {
    with_manager(Component {
        scene_key: 7,
        index: 3,
    })
    .unwrap()
}
fn with_manager(manager: Component) -> Result<Immediate, Error> {
    let asset = Asset {
        bundle: 16,
        type_id: 3424,
        local_index: 10,
    };
    let content = Content::new(
        vec![Catalog::new(16, &[(3404, 1), (3424, 11)]).unwrap()],
        vec![(
            asset,
            entity::Profile::new(&[entity::Kind::SequenceRoot]).unwrap(),
        )],
    )
    .unwrap();
    Immediate::new(asset, manager, 2, 1, content)
}

#[test]
fn selected_arrival_is_owned_root_only_and_requires_completion_before_cleanup() {
    let (mut players, arrivals) = setup();
    let mut arrivals = arrivals
        .with_immediate(definition())
        .unwrap()
        .with_spline(spline::Definition {
            asset: Asset {
                bundle: 23,
                type_id: 5100,
                local_index: 2,
            },
            catalog: Catalog::new(23, &[(5100, 3)]).unwrap(),
            flag: false,
        })
        .unwrap();
    let owner = join(&mut players);
    // No Actor: a selected departure must not create/assign the garage spline.
    let before = players.objects().snapshot().len();
    let records = arrivals.start_departure(&mut players, owner, true).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(players.objects().snapshot().len(), before + 1);
    let sequence = records[0].id;
    let Some(Initial::Entity { prefix, fields }) = &records[0].initial else {
        panic!()
    };
    assert_eq!(prefix.asset.local_index, 10);
    assert_eq!(fields.len(), 1);
    assert!(matches!(&fields[0], entity::Initial::SequenceRoot {
        manager_selector: 2, sequence: 0, stopping: true, participants, ..
    } if participants == &[owner.participant]));
    assert!(
        arrivals
            .start_departure(&mut players, owner, true)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        arrivals.start_rollout(&mut players, owner),
        Err(Error::DuplicateObject)
    );
    assert_eq!(
        arrivals.finish_rollout(&mut players, owner),
        Err(Error::Shape)
    );
    assert!(!arrivals.acknowledges(sequence, 2, 0));
    assert!(!arrivals.recognizes_arrival(
        &players,
        owner.connection,
        owner.persona,
        &reached(ARRIVED, sequence, 1)
    ));
    assert!(arrivals.has_completion(sequence, 0));
    let call = Completed {
        sequence,
        participant: owner.participant,
    };
    assert_eq!(
        arrivals.complete(
            &players,
            Owner {
                persona: owner.persona + 1,
                ..owner
            },
            call
        ),
        Err(Error::UnknownObject)
    );
    assert_eq!(arrivals.complete(&players, owner, call), Ok(Some(sequence)));
    assert_eq!(arrivals.complete(&players, owner, call), Ok(None));
    assert_eq!(
        arrivals.finish_rollout(&mut players, owner),
        Ok((vec![], vec![sequence]))
    );
    assert_eq!(
        arrivals.finish_rollout(&mut players, owner),
        Ok((vec![], vec![]))
    );
    arrivals.reset(&players, owner).unwrap();
    let next = arrivals.start_departure(&mut players, owner, true).unwrap()[0].id;
    assert_ne!(next, sequence);
    assert!(arrivals.complete(&players, owner, call).is_err());
    assert!(players.objects().get(next).is_some());
}

#[test]
fn missing_configuration_and_manager_leave_no_partial_selected_arrival() {
    let (mut players, mut arrivals) = setup();
    let owner = join(&mut players);
    let before = players.objects().snapshot();
    assert_eq!(
        arrivals.start_departure(&mut players, owner, true),
        Err(Error::Unsupported)
    );
    assert_eq!(players.objects().snapshot(), before);
    assert_eq!(arrivals.ghost(owner.participant), None);
    let missing = with_manager(Component {
        scene_key: 99,
        index: 3,
    })
    .unwrap();
    let mut arrivals = arrivals.with_immediate(missing).unwrap();
    assert_eq!(
        arrivals.start_departure(&mut players, owner, true),
        Err(Error::UnknownObject)
    );
    assert_eq!(players.objects().snapshot(), before);
    assert_eq!(arrivals.ghost(owner.participant), None);
}

#[test]
fn selected_timeout_never_releases_or_falls_back_and_preserves_other_arrivals() {
    let (mut players, arrivals) = setup();
    let mut arrivals = arrivals.with_immediate(definition()).unwrap();
    let normal = join(&mut players);
    let normal_record = arrivals
        .start_rollout(&mut players, normal)
        .unwrap()
        .remove(0);
    let player = players
        .create(
            5,
            500,
            Request {
                name: b"Other".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let selected = Owner {
        connection: 5,
        persona: 500,
        participant: players.join(5, 500, player).unwrap().unwrap().id,
    };
    let record = arrivals
        .start_departure(&mut players, selected, true)
        .unwrap()
        .remove(0);
    let Some(Initial::Entity { fields, .. }) = &record.initial else {
        panic!()
    };
    assert!(matches!(
        &fields[0],
        entity::Initial::SequenceRoot { sequence: 1, .. }
    ));
    assert!(arrivals.poll(FALLBACK_MS - 1).unwrap().is_empty());
    let before = players.objects().snapshot();
    assert_eq!(arrivals.poll(FALLBACK_MS), Err(Error::Unsupported));
    assert_eq!(arrivals.world_ms, FALLBACK_MS - 1);
    assert_eq!(players.objects().snapshot(), before);
    assert!(matches!(
        arrivals.entries[&normal.participant].phase,
        Phase::Running { .. }
    ));
    assert!(matches!(
        arrivals.entries[&selected.participant].phase,
        Phase::Immediate { .. }
    ));
    let call = Completed {
        sequence: record.id,
        participant: selected.participant,
    };
    assert!(arrivals.complete(&players, normal, call).is_err());
    assert_eq!(
        arrivals.complete(&players, selected, call),
        Ok(Some(record.id))
    );
    // After real completion, the independent ordinary deadline is still serviced.
    assert_eq!(
        arrivals.poll(FALLBACK_MS).unwrap()[0].sequence,
        normal_record.id
    );
    arrivals.finish_rollout(&mut players, selected).unwrap();
    arrivals.reset(&players, selected).unwrap();
    let next = arrivals.start_rollout(&mut players, selected).unwrap();
    let Some(Initial::Entity { prefix, .. }) = &next[0].initial else {
        panic!()
    };
    assert_eq!(prefix.asset.local_index, 9);
    assert!(arrivals.acknowledges(next[0].id, 2, 0));
}

#[test]
fn immediate_definition_rejects_non_root_profile_and_invalid_manager() {
    let (_, arrivals) = setup();
    assert!(
        Immediate::new(
            arrivals.definition.asset,
            arrivals.definition.manager,
            2,
            1,
            arrivals.content
        )
        .is_err()
    );
    assert!(
        with_manager(Component {
            scene_key: 0,
            index: 0
        })
        .is_err()
    );
    assert!(
        with_manager(Component {
            scene_key: 7,
            index: 512
        })
        .is_err()
    );
}
