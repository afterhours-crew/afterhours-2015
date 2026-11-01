// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::{
    rival_content::population::{Focus, Input},
    vehicle_content::registrations::Registry,
};
use nfs_content::scoring::{
    ai_deployment::{BooleanSet, membership::Binding},
    events::Identity,
    participant_properties::Property,
};
use nfs_world::scoring::{
    nodes::FLOAT_VALUE,
    program::{Behavior, Step},
    properties::{Definition, Port},
};

fn advance_population(
    listener: &mut PlayerListener,
    registry: &mut Registry,
    now: u64,
    position: Option<[f32; 3]>,
) {
    let focus: Vec<_> = position
        .into_iter()
        .map(|position| Focus { id: 1, position })
        .collect();
    listener
        .rival_population
        .as_mut()
        .unwrap()
        .advance(
            &mut listener.players,
            registry,
            HOST_SELECTOR as u8,
            100,
            Input {
                now,
                focus: &focus,
                random: 12345,
                preserved: &[],
            },
        )
        .unwrap();
}

fn fixture() -> (PlayerListener, Registry) {
    let (population, players, mut registry) =
        crate::rival_content::population::tests::far_fixture_with(HOST_SELECTOR as u8);
    let mut listener = PlayerListener::new(100);
    listener.players = players;
    listener.rival_population = Some(population);
    advance_population(&mut listener, &mut registry, 0, Some([0., 1., 0.]));
    let ai = listener
        .players
        .ai_participants(HOST_SELECTOR as u8, 100)
        .unwrap()[0];
    assert!(ai.car.is_none());
    let managers = ai.managers.unwrap();
    let mut installed = crate::scoring::tests::content().installed().clone();
    for part in [
        &mut installed.acquisition,
        &mut installed.scoring,
        &mut installed.ai.participant,
    ] {
        part.layout.scene_content_key = managers.scene;
    }
    let id = Identity {
        asset: "constructed/scene".into(),
        index: 3,
        instances: vec![],
    };
    installed.ai.eligibility = serde_json::from_value(serde_json::json!({
        "ops": [{"Fact": {"States": [{"scene_key": managers.alive.scene, "serializer": managers.alive.serializer}]}},
                {"Fact": {"Ai": id}}, {"Combine": ["Intersection", 0, 1]}],
        "ranges": [], "empty_sets": []
    })).unwrap();
    installed.ai.membership = vec![Binding {
        fact: Fact::Ai(id),
        owner: Owner::Ai {
            manager: Endpoint {
                scene_key: managers.scene,
                serializer: managers.participants,
            },
        },
    }];
    let program = Arc::make_mut(&mut installed.ai.participant.program);
    for (node, kind, value) in [
        (1, Kind::Float, Value::Float(0.)),
        (2, Kind::Boolean, Value::Boolean(false)),
    ] {
        program.cells.push(Definition {
            kind,
            initial: Some(value),
            ports: vec![Port {
                node,
                field: nfs_world::scoring::synced::OUT,
            }],
            listeners: vec![],
        });
    }
    for port in [101, 102] {
        let index = program.nodes.len() as u16;
        let property = Property {
            channel: "constructed/driver".into(),
            port,
        };
        let mut node = program.nodes[0].clone();
        node.identity.index = u32::from(index);
        node.class = "BoolParticipantPropertyAccessorEntityData".into();
        node.behavior = Behavior::ParticipantProperty {
            property: property.clone(),
            kind: Kind::Boolean,
            write: true,
        };
        program.nodes.push(node);
        program.cells.push(Definition {
            kind: Kind::Boolean,
            initial: Some(Value::Boolean(true)),
            ports: vec![Port {
                node: index,
                field: FLOAT_VALUE,
            }],
            listeners: vec![],
        });
        program
            .startup
            .extend([Step::Bind(index), Step::Activate(index)]);
        installed.ai.boolean_sets.push(BooleanSet {
            property,
            set: Property {
                channel: "constructed/driver".into(),
                port: port + 100,
            },
            definition: Identity {
                asset: "constructed/scene".into(),
                index: index.into(),
                instances: vec![],
            },
        });
    }
    for field in [0xad59_4576, 0x445f_266b, 0xd70a_7f0c, 0xd887_31d6] {
        program.cells.push(Definition {
            kind: Kind::Float,
            initial: Some(Value::Float(0.)),
            ports: vec![Port { node: 3, field }],
            listeners: vec![],
        });
    }
    listener.scoring = Some(Arc::new(Content::new(Arc::new(installed)).unwrap()));
    (listener, registry)
}

#[test]
fn ai_scoring_keeps_carless_sessions_projects_authored_flags_and_retires() {
    let (mut listener, mut registry) = fixture();
    let before = listener.players.objects().snapshot();
    listener.advance_scoring(0).unwrap();
    let participant = *listener.scoring_state.ai.keys().next().unwrap();
    assert_eq!(listener.players.objects().snapshot(), before);
    assert!(listener.pending.is_empty());
    assert!(listener.pending_rpcs.is_empty());
    assert!(listener.scoring_state.ai[&participant].car.is_none());
    let content = listener.scoring.clone().unwrap();
    let property = &content.installed().ai.boolean_sets[0].property;
    assert_eq!(
        listener.scoring_state.ai[&participant]
            .session
            .participant_value(u64::from(participant), property, Kind::Boolean),
        Some(Value::Boolean(true))
    );
    assert!(
        members(&content, &listener.players, &listener.scoring_state, 100)
            .unwrap()
            .is_empty()
    );
    advance_population(&mut listener, &mut registry, 10_000, Some([0., 1., 0.]));
    listener.advance_scoring(10_000).unwrap();
    assert_eq!(listener.scoring_state.ai.len(), 1);
    let (key, car) = listener.scoring_state.ai[&participant].car.unwrap();
    let rows = members(&content, &listener.players, &listener.scoring_state, 100).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| row.vehicle.identity == u32::from(car))
    );
    let own = draft::Vehicle {
        identity: 6000,
        position: rows[0].vehicle.position,
        velocity: [0.; 3],
    };
    assert_eq!(
        nfs_world::scoring::proximity::count(own, &rows, 201, 20.).unwrap(),
        1
    );
    assert_eq!(
        nfs_world::scoring::proximity::count(rows[0].vehicle, &rows, 201, 20.).unwrap(),
        0
    );
    assert!(members(&content, &listener.players, &listener.scoring_state, 200).is_err());
    // The missing-host control produces no Crew members despite an owned car.
    let mut missing = listener.scoring_state.clone();
    missing.ai.clear();
    assert_eq!(
        nfs_world::scoring::proximity::count(
            own,
            &members(&content, &listener.players, &missing, 100).unwrap(),
            201,
            20.
        )
        .unwrap(),
        0
    );
    // A delegated car goes away while its participant survives. Update the
    // owned snapshot directly to isolate that handoff from population policy.
    listener
        .players
        .remove_ai_vehicle(HOST_SELECTOR as u8, 100, key)
        .unwrap();
    let desired = listener
        .players
        .ai_participants(HOST_SELECTOR as u8, 100)
        .unwrap()
        .into_iter()
        .map(|p| (p.participant, p))
        .collect();
    let players = listener.players.clone();
    let mut state = listener.scoring_state.clone();
    listener
        .advance_ai_scoring(&content, &players, &mut state, &desired, 10_100)
        .unwrap();
    assert_eq!(state.ai.len(), 1);
    assert!(state.ai[&participant].car.is_none());
    assert!(members(&content, &players, &state, 100).unwrap().is_empty());
    listener
        .advance_ai_scoring(&content, &players, &mut state, &BTreeMap::new(), 10_200)
        .unwrap();
    assert!(state.ai.is_empty());
    assert_eq!(
        players.objects().snapshot(),
        listener.players.objects().snapshot()
    );
    assert!(state.dirty.is_empty());
    assert!(state.dirty_deleted.is_empty());
    assert!(state.pending_awards.is_empty());
}

#[test]
fn ai_scoring_failed_start_rolls_back_and_unknown_population_rejects() {
    let (mut listener, _) = fixture();
    let before = listener.players.objects().snapshot();
    let good = listener.scoring.clone().unwrap();
    let mut broken = good.installed().clone();
    // Fail after both startup flags have been written. A duplicate accessor
    // activation is invalid at runtime and must not leave a partial session.
    Arc::make_mut(&mut broken.ai.participant.program)
        .startup
        .push(Step::Activate(5));
    listener.scoring = Some(Arc::new(Content::new(Arc::new(broken)).unwrap()));
    assert_eq!(listener.advance_scoring(1), Err(replication::Error::Shape));
    assert_eq!(listener.players.objects().snapshot(), before);
    assert!(listener.scoring_state.ai.is_empty());
    assert!(listener.scoring_state.dirty.is_empty());
    assert!(listener.pending.is_empty());
    assert!(listener.pending_rpcs.is_empty());
    listener.scoring = Some(good);
    listener.advance_scoring(1).unwrap();
    assert_eq!(listener.scoring_state.ai.len(), 1);
    let content = listener.scoring.clone().unwrap();
    let previous = listener.players.objects().snapshot();
    listener.rival_population = None;
    assert_eq!(
        listener.desired_ai_scoring(&content).unwrap_err(),
        replication::Error::Unsupported
    );
    assert_eq!(listener.players.objects().snapshot(), previous);
    assert!(listener.scoring_state.pending_awards.is_empty());
}

#[test]
fn local_ai_cannot_send_client_commands_or_credit_a_human_player() {
    use nfs_world::scoring::collision::Command;
    let (listener, _) = fixture();
    let content = listener.scoring.as_deref().unwrap();
    for output in [
        Output {
            commands: vec![(4, Command::Begin)],
            ..Output::default()
        },
        Output {
            awards: vec![Award {
                keeper: 10,
                participant: 100,
                amount: 500,
            }],
            ..Output::default()
        },
        Output {
            participant_properties: vec![nfs_world::scoring::participant_properties::Write {
                participant: 100,
                property: content.installed().ai.boolean_sets[0].property.clone(),
                value: Value::Boolean(true),
            }],
            ..Output::default()
        },
    ] {
        assert_eq!(
            local_output(content, 42, output, false),
            Err(replication::Error::Unsupported)
        );
    }
}

#[test]
fn ai_scoring_has_no_client_query_target_and_retires_after_population() {
    use nfs_world::bits::BitWriter;
    let (mut listener, _) = fixture();
    listener.advance_scoring(0).unwrap();
    let (&participant, live) = listener.scoring_state.ai.first_key_value().unwrap();
    let before_session = format!("{:?}", live.session);
    let mut payload = BitWriter::new();
    payload.put(1, 9).put(0, 32);
    for value in [1f32, 2., 3., 4.] {
        payload.put(u64::from(value.to_bits()), 32);
    }
    payload.put(0x7f, 7);
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(1, 8)
        .put(u64::from(participant), 13)
        .put(payload.span().len().div_ceil(8) as u64, 9)
        .put_span(payload.span());
    assert!(
        listener
            .scoring_report(&listener.players, body.span())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        format!("{:?}", listener.scoring_state.ai[&participant].session),
        before_session
    );
    assert!(listener.scoring_state.pending_awards.is_empty());
    let removed = listener
        .players
        .remove_ai_for_connection(HOST_SELECTOR as u8)
        .unwrap();
    assert!(removed.deleted.contains(&participant));
    listener.pending.clear();
    listener.advance_scoring(2).unwrap();
    assert!(listener.scoring_state.ai.is_empty());
    assert!(listener.scoring_state.dirty.is_empty());
    assert!(
        listener.pending.is_empty(),
        "local scoring retirement must not publish an extra deletion"
    );
}

#[test]
fn explicit_ai_children_retire_atomically_and_preserve_unregistered_references() {
    let (mut listener, _) = fixture();
    listener.advance_scoring(0).unwrap();
    let (&participant, live) = listener.scoring_state.ai.first_key_value().unwrap();
    let content = listener.scoring.clone().unwrap();
    let extra = content
        .creation(Which::Ai, &live.session, participant, &listener.players)
        .unwrap();
    // A caller may explicitly own a replicated child for another purpose.
    // The local scoring session itself has not allocated one.
    let ghost = listener
        .players
        .spawn_entities(vec![extra.clone()], content.creation_content())
        .unwrap()[0]
        .id;
    listener
        .players
        .bind_ai_entity(HOST_SELECTOR as u8, 100, participant, ghost)
        .unwrap();
    let extra_id = listener
        .players
        .spawn_entities(vec![extra], content.creation_content())
        .unwrap()[0]
        .id;
    for (connection, persona) in [(1, 100), (HOST_SELECTOR as u8, 200)] {
        assert_eq!(
            listener
                .players
                .bind_ai_entity(connection, persona, participant, extra_id),
            Err(replication::Error::UnknownObject)
        );
    }
    assert_eq!(
        listener
            .players
            .bind_ai_entity(HOST_SELECTOR as u8, 100, participant, participant),
        Err(replication::Error::TypeMismatch)
    );
    // Registering the same child is inert and cannot consume capacity.
    listener
        .players
        .bind_ai_entity(HOST_SELECTOR as u8, 100, participant, ghost)
        .unwrap();
    let before = listener.players.objects().snapshot();
    assert_eq!(
        listener
            .players
            .remove_ai_for_connection(HOST_SELECTOR as u8)
            .unwrap_err(),
        replication::Error::UnknownObject
    );
    assert_eq!(listener.players.objects().snapshot(), before);
    assert!(
        listener
            .players
            .owns_ai_participant(HOST_SELECTOR as u8, 100, participant)
    );
    // Only the explicit child cascades. The extra reference must be resolved by
    // its own caller; a failed later removal restores the child's registration.
    listener.players.remove_entities(&[extra_id]).unwrap();
    let removed = listener
        .players
        .remove_ai_for_connection(HOST_SELECTOR as u8)
        .unwrap();
    assert_eq!(removed.deleted.iter().filter(|id| **id == ghost).count(), 1);
    assert!(!removed.deleted.contains(&extra_id));
    assert!(listener.players.objects().get(ghost).is_none());
    assert!(
        !listener
            .players
            .owns_ai_participant(HOST_SELECTOR as u8, 100, participant)
    );
    assert!(
        listener
            .players
            .remove_ai_for_connection(HOST_SELECTOR as u8)
            .unwrap()
            .deleted
            .is_empty()
    );
}

#[test]
fn ai_scoring_waits_for_join_before_reading_owned_population() {
    let (mut listener, _) = fixture();
    let joined = std::mem::take(&mut listener.players);
    for now in [0, 1, 2] {
        listener.advance_scoring(now).unwrap();
        assert!(listener.scoring_state.ai.is_empty());
        assert!(listener.pending.is_empty());
        assert!(listener.pending_rpcs.is_empty());
    }
    listener.players = joined;
    listener.advance_scoring(3).unwrap();
    assert_eq!(listener.scoring_state.ai.len(), 1);
    assert_eq!(
        listener
            .pending
            .iter()
            .flat_map(|s| &s.records)
            .filter(|r| r.initial.is_some())
            .count(),
        0
    );
    // Once a population exists, missing/foreign account ownership is still
    // an error; the startup boundary must not conceal invalid live state.
    let before = listener.players.objects().snapshot();
    listener.persona = 200;
    assert_eq!(
        listener.advance_scoring(4),
        Err(replication::Error::UnknownObject)
    );
    assert_eq!(listener.players.objects().snapshot(), before);
    assert_eq!(listener.scoring_state.ai.len(), 1);
}

#[test]
fn ai_scoring_waits_for_publication_capacity_and_resumes_once() {
    let (mut listener, _) = fixture();
    let before = listener.players.objects().snapshot();
    for _ in 0..MAX_PENDING {
        listener.pending.push_back(packed(vec![], vec![]));
    }
    listener.advance_scoring(0).unwrap();
    assert!(listener.scoring_state.ai.is_empty());
    assert_eq!(listener.players.objects().snapshot(), before);
    assert!(listener.pending_rpcs.is_empty());
    listener.pending.clear();
    listener.advance_scoring(1).unwrap();
    assert_eq!(listener.scoring_state.ai.len(), 1);
    let (&participant, live) = listener.scoring_state.ai.first_key_value().unwrap();
    let before_session = format!("{:?}", live.session);
    listener.advance_scoring(1).unwrap();
    assert_eq!(
        format!("{:?}", listener.scoring_state.ai[&participant].session),
        before_session
    );
    assert!(listener.pending.is_empty());
    assert!(listener.pending_rpcs.is_empty());
    assert_eq!(listener.players.objects().snapshot(), before);
}
