// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::rival_content::population::tests::event_fixture;
use nfs_world::replication::{Initial, Update, goto_waypoint, wander};

fn selections() -> [Selection; 2] {
    [
        Selection {
            persona: 0,
            slot_key: 1000,
            car_key: 2000,
        },
        Selection {
            persona: 1,
            slot_key: 1001,
            car_key: 2001,
        },
    ]
}

#[test]
fn provider_cursor_and_spawn_namespaces_are_session_owned() {
    let (content, _, _) = event_fixture();
    let mut cursor = Cursor::default();
    let mut another = Cursor::default();
    let first = cursor.select(&content).unwrap();
    assert_eq!(first, another.select(&content).unwrap());
    assert_eq!(first.iter().map(|s| s.persona).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(first[0].slot_key, KEY_START);
    let second = cursor.select(&content).unwrap();
    assert_eq!(second[0].car_key, KEY_START + 2);
    let mut invalid = content.clone();
    invalid.roster.spawns[1].personas = vec![0];
    let mut unchanged = Cursor::default();
    assert!(unchanged.select(&invalid).is_err());
    assert_eq!(unchanged.select(&content).unwrap(), first);
}

#[test]
fn grid_cars_use_authored_pose_and_wait_for_the_first_client_report() {
    let (content, mut world, mut registry) = event_fixture();
    let created = content
        .create(&mut world, &mut registry, 1, 100, &selections())
        .unwrap();
    assert_eq!((world.ai_slot_count(), world.ai_count()), (2, 2));
    assert_eq!(created.racers.members.len(), 2);
    for (member, grid) in created.racers.members.iter().zip(&content.roster.spawns) {
        let key = member.selection.car_key;
        assert_eq!(world.ai_ids(1, 100, key), Some(member.ids));
        let motion = world
            .ai_vehicle_physics(1, 100, key)
            .unwrap()
            .decode([0.; 3])
            .unwrap();
        // Native AI spawn lifts this zero-suspension fixture by one up vector.
        let expected_position =
            std::array::from_fn::<_, 3, _>(|i| grid.position[i] + grid.basis[1][i]);
        for (actual, expected) in motion.position.into_iter().zip(expected_position) {
            assert!((actual - expected).abs() < 0.001);
        }
        assert_eq!(motion.velocity, [0.; 3]);
        assert_eq!(
            world.release_ai_control(2, 100, key),
            Err(Error::UnknownObject)
        );
        assert!(world.release_ai_control(1, 100, key).unwrap().is_some());
        assert!(world.release_ai_control(1, 100, key).unwrap().is_none());
    }
    // Manager add/bind, Alive entry and reset-volume tracking for each car.
    assert_eq!(created.calls.len(), 8);
    for record in created.records {
        record.encode().unwrap();
    }
    for message in created.messages {
        message.encode().unwrap();
    }
}

#[test]
fn a_later_car_failure_rolls_back_the_first_car_and_all_registrations() {
    let (content, mut world, mut registry) = event_fixture();
    // The second slot collides only after the first car has been constructed.
    world
        .create_ai_slot(
            1,
            100,
            ai::Request {
                key: 1001,
                name: b"Occupied".to_vec(),
                flags: [false, true],
                words: [1, 1],
                mode: 2,
            },
            content.roster.managers.unwrap(),
        )
        .unwrap()
        .unwrap();
    let (snapshot, registrations) = (world.objects().snapshot(), registry.clone());
    let (mut expected, mut expected_registry) = (world.clone(), registry.clone());
    assert!(
        content
            .create(&mut world, &mut registry, 1, 100, &selections())
            .is_err()
    );
    assert_eq!(world.objects().snapshot(), snapshot);
    assert_eq!(registry, registrations);
    assert_eq!((world.ai_slot_count(), world.ai_count()), (1, 0));
    assert_eq!(world.ai_slot(1, 100, 1000), None);
    let mut retry = selections();
    retry[1].slot_key = 1002;
    let actual = content
        .create(&mut world, &mut registry, 1, 100, &retry)
        .unwrap();
    let expected = content
        .create(&mut expected, &mut expected_registry, 1, 100, &retry)
        .unwrap();
    assert_eq!(actual.racers.members, expected.racers.members);
    assert_eq!(actual.records, expected.records);
    assert_eq!(registry, expected_registry);
}

#[test]
fn invalid_selection_and_duplicate_creation_are_inert_and_disconnect_is_owned() {
    let (content, mut world, mut registry) = event_fixture();
    let before = world.objects().snapshot();
    for selection in [
        vec![selections()[0]],
        vec![selections()[0]; 2],
        vec![
            Selection {
                persona: 2,
                ..selections()[0]
            },
            selections()[1],
        ],
        vec![
            Selection {
                car_key: u64::MAX,
                ..selections()[0]
            },
            selections()[1],
        ],
    ] {
        assert!(
            content
                .create(&mut world, &mut registry, 1, 100, &selection)
                .is_err()
        );
        assert_eq!(world.objects().snapshot(), before);
    }
    let created = content
        .create(&mut world, &mut registry, 1, 100, &selections())
        .unwrap();
    let snapshot = world.objects().snapshot();
    assert!(
        content
            .create(&mut world, &mut registry, 1, 100, &selections())
            .is_err()
    );
    assert_eq!(world.objects().snapshot(), snapshot);
    assert!(
        world
            .remove_ai_for_connection(2)
            .unwrap()
            .deleted
            .is_empty()
    );
    assert_eq!(world.objects().snapshot(), snapshot);
    let removed = world.remove_ai_for_connection(1).unwrap();
    for member in created.racers.members {
        for id in [member.ids.participant, member.ids.actor, member.ids.vehicle] {
            assert!(removed.deleted.contains(&id));
        }
    }
    assert_eq!((world.ai_count(), world.ai_slot_count()), (0, 0));
    assert!(
        world
            .remove_ai_for_connection(1)
            .unwrap()
            .deleted
            .is_empty()
    );
}

fn race_route() -> goto_waypoint::Initial {
    goto_waypoint::Initial {
        selector: 0,
        serial: 1,
        allocation_serial: 0,
        waypoint: 0,
        word: 1,
        points: vec![[0; 3], [100f32.to_bits(), 0, 0]],
        flags: [false; 2],
        radius: 150f32.to_bits(),
        progress_updates: true,
    }
}

fn human(world: &mut Players) -> u16 {
    let participant = world
        .objects()
        .iter()
        .find_map(|(id, _)| world.owns_participant(1, 100, id).then_some(id))
        .unwrap();
    world.create_actor(1, 100, participant).unwrap();
    participant
}

/// A synthetic owned driver car with the same schema as this fixture's AI cars.
pub(crate) fn driver(
    content: &Content,
    world: &mut Players,
    registry: &mut registrations::Registry,
    connection: u8,
) -> (u16, u16) {
    use nfs_world::replication::entity::creation::{Catalog, Content as EntityContent};
    let participant = world
        .objects()
        .iter()
        .find_map(|(id, _)| world.owns_participant(connection, 100, id).then_some(id))
        .unwrap();
    world.create_actor(connection, 100, participant).unwrap();
    let created = content
        .create(world, registry, connection, 100, &selections())
        .unwrap();
    let object = world
        .objects()
        .get(created.racers.members[0].ids.vehicle)
        .unwrap();
    let Initial::Vehicle { prefix, creation } = &object.initial else {
        panic!()
    };
    let Update::Vehicle { fields, .. } = &object.current else {
        panic!()
    };
    let asset = prefix.asset;
    let definition = content
        .vehicles
        .definition(&content.roster.personas[0].preset.vehicle)
        .unwrap();
    let schema = EntityContent::new(
        vec![Catalog::new(asset.bundle, &definition.catalog).unwrap()],
        vec![],
    )
    .unwrap()
    .with_vehicles(vec![(asset, definition.profile.clone())])
    .unwrap();
    let creation = vehicle::EntityCreation {
        prefix: prefix.clone(),
        body: vehicle::Body {
            creation: Some(creation.clone()),
            updates: fields.clone(),
        },
    };
    let car = world
        .create_vehicle(connection, 100, participant, 1, creation, &schema)
        .unwrap()
        .id;
    for member in created.racers.members {
        world
            .remove_ai_slot(connection, 100, member.selection.slot_key)
            .unwrap();
    }
    (participant, car)
}

#[test]
fn shared_race_commands_advance_independently_and_keep_finished_successors() {
    let (content, mut world, mut registry) = event_fixture();
    let human = human(&mut world);
    let created = content
        .create(&mut world, &mut registry, 1, 100, &selections())
        .unwrap();
    let mut intro_goals = Vec::new();
    for member in &created.racers.members {
        let intro = world
            .replace_ai_goals(
                1,
                100,
                member.selection.car_key,
                vec![(
                    Initial::WanderGoal(wander::Initial {
                        allocation_serial: 0,
                        flags: [false; 3],
                    }),
                    Update::WanderGoal,
                )],
                None,
            )
            .unwrap();
        intro_goals.extend(intro.goals);
    }
    let (mut commands, changes) = Commands::start(
        &mut world,
        1,
        100,
        human,
        &created.racers,
        race_route(),
        &[0.5, 0.4, 0.6],
    )
    .unwrap();
    for id in intro_goals {
        assert!(changes.deleted.contains(&id));
        assert!(world.objects().get(id).is_none());
    }
    let group = commands.pacing().unwrap();
    let actor = world.owned_actor(1, 100, human).unwrap();
    let Update::Actor(current) = &world.objects().get(actor).unwrap().current else {
        panic!()
    };
    assert_eq!(current.flag_a, Some(false));
    assert_eq!(current.references_a.as_ref().unwrap()[1], group);
    let member = created.racers.members[0];
    let ids = [
        human,
        member.ids.participant,
        created.racers.members[1].ids.participant,
    ];
    let mut progress = nfs_world::race_progress::Race::new(2, 1, &ids, 0.).unwrap();
    let crossing = progress
        .cross(member.ids.participant, 0, 1.)
        .unwrap()
        .unwrap();
    let changed = commands
        .crossed(&mut world, member.ids.participant, crossing)
        .unwrap();
    assert_eq!(changed.records.len(), 1);
    assert_eq!(
        changed.records[0].update,
        Update::GotoWaypointGoal(goto_waypoint::Update::Progress(Some([1, 1])))
    );
    let finished = progress
        .cross(member.ids.participant, 1, 2.)
        .unwrap()
        .unwrap();
    let removed = commands
        .crossed(&mut world, member.ids.participant, finished)
        .unwrap();
    assert_eq!(removed.deleted.len(), 1);
    assert!(world.objects().get(group).is_some());
    assert_eq!(commands.members(), &[ids[0], ids[2]]);
    assert_eq!(
        world.objects().get(group).unwrap().current,
        Update::AiPacingGroup(nfs_world::replication::pacing::Update {
            entries: Some(vec![
                nfs_world::replication::pacing::Entry {
                    participant: ids[0],
                    value: 0.5f32.to_bits(),
                },
                nfs_world::replication::pacing::Entry {
                    participant: ids[2],
                    value: 0.6f32.to_bits(),
                },
            ]),
        })
    );
    assert!(
        commands
            .crossed(&mut world, member.ids.participant, finished)
            .unwrap()
            .records
            .is_empty()
    );
    // The successor belongs to the director/outro and must survive race teardown.
    let successor = world
        .replace_ai_goals(
            1,
            100,
            member.selection.car_key,
            vec![(
                Initial::WanderGoal(wander::Initial {
                    allocation_serial: 0,
                    flags: [false, false, true],
                }),
                Update::WanderGoal,
            )],
            None,
        )
        .unwrap()
        .goals[0];
    assert!(commands.pace(&mut world, &[0.5, 0.7]).unwrap().is_some());
    let removed = commands.clear(&mut world).unwrap();
    assert!(removed.deleted.contains(&group));
    assert!(world.objects().get(successor).is_some());
    for member in &created.racers.members {
        assert_eq!(
            world.ai_ids(1, 100, member.selection.car_key),
            Some(member.ids)
        );
    }
    assert!(commands.clear(&mut world).unwrap().deleted.is_empty());
    assert!(commands.pace(&mut world, &[0.5; 3]).is_err());
    for record in changes.records.into_iter().chain(removed.records) {
        record.encode().unwrap();
    }
}

#[test]
fn race_command_failure_rolls_back_pacing_allocation_and_intro_commands() {
    let (content, mut world, mut registry) = event_fixture();
    let human = human(&mut world);
    let created = content
        .create(&mut world, &mut registry, 1, 100, &selections())
        .unwrap();
    world.actor_arriving(1, 100, human).unwrap();
    let before = world.objects().snapshot();
    assert!(
        Commands::start(
            &mut world,
            1,
            100,
            human,
            &created.racers,
            race_route(),
            &[0.5; 3]
        )
        .is_err()
    );
    assert_eq!(world.objects().snapshot(), before);
    assert!(
        Commands::start(
            &mut world,
            2,
            100,
            human,
            &created.racers,
            race_route(),
            &[0.5; 3]
        )
        .is_err()
    );
    assert!(
        Commands::start(
            &mut world,
            1,
            100,
            human,
            &created.racers,
            race_route(),
            &[f32::NAN; 3]
        )
        .is_err()
    );
    assert_eq!(world.objects().snapshot(), before);
}
