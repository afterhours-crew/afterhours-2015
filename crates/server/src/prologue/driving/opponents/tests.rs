// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::events::{
    Field,
    lifecycle::{self as data, bindings::Observer},
};
use nfs_world::replication::entity::Kind;

pub(crate) fn fixture() -> (
    Players,
    Owner,
    Mode,
    crate::vehicle_content::registrations::Registry,
) {
    let (ai, players, registry) = crate::event_ai::tests::story_fixture();
    let (players, owner, base) = super::super::tests::clock_fixture_in(true, players);
    let mut data = base.data;
    for (i, kind) in [(7, Kind::RpcReferences), (8, Kind::Rpc)] {
        data.game_mode.layout.fields.push(Field {
            kind,
            class: format!("test{i}"),
            asset: "test/drive".into(),
            object: i,
            binding: None,
            mirrors: None,
        });
    }
    data.game_mode.lifecycle.states.push(data::State {
        serializer: 7,
        kind: StateKind::WaitForAi,
        configuration: StateConfiguration::WaitForAi {
            manager: Property {
                channel: "test/ai".into(),
                port: 1,
            },
        },
        handles_marked_for_delete: false,
        send_rpc_to_client: true,
    });
    data.game_mode.lifecycle.registered_states.push(7);
    data.game_mode.lifecycle.transitions.push(data::Transition {
        serializer: 8,
        kind: data::TransitionKind::Completed,
        sources: vec![data::Endpoint {
            serializer: 7,
            port: 0,
        }],
        target: Some(data::Endpoint {
            serializer: 3,
            port: 0,
        }),
        configuration: data::TransitionConfiguration::Default,
    });
    data.game_mode.lifecycle.registered_transitions.push(8);
    data.game_mode
        .lifecycle
        .bindings
        .order
        .push(Observer::Transition(8));
    let mut mode = Mode::new(data, base.parent, base.sequences).unwrap();
    mode.opponents = Some(Arc::new(ai));
    (players, owner, mode, registry)
}

pub(crate) fn finished_fixture() -> (Players, Owner, Mode, Active) {
    let (mut players, owner, mode, mut registry) = fixture();
    let ai = mode.opponents().unwrap();
    let selected = crate::event_ai::Cursor::default().select(ai).unwrap();
    let mut created = ai
        .create(
            &mut players,
            &mut registry,
            owner.connection,
            owner.persona,
            &selected,
        )
        .unwrap();
    let (mut active, _) = Active::start(&mode, &mut players, owner, 12.).unwrap();
    active
        .admit(&mode, &mut players, &mut created.racers)
        .unwrap();
    let intro = active.sequences[&3].ghost(owner.participant).unwrap();
    active
        .sequences
        .get_mut(&3)
        .unwrap()
        .stop(&mode.sequences[&3], &players, owner)
        .unwrap();
    active
        .completed(
            &mode,
            &mut players,
            Completed {
                sequence: intro,
                participant: owner.participant,
            },
            123,
        )
        .unwrap();
    let sequence = active.sequences[&1].ghost(owner.participant).unwrap();
    active.advance(&mode, &mut players, 12.1).unwrap();
    active
        .completed(
            &mode,
            &mut players,
            Completed {
                sequence,
                participant: owner.participant,
            },
            123,
        )
        .unwrap();
    (players, owner, mode, active)
}

#[test]
fn timed_opponent_outros_complete_without_acknowledging_the_human_sequence() {
    let (mut players, owner, base, mut registry) = fixture();
    let mut data = base.data;
    data.game_mode
        .lifecycle
        .states
        .iter_mut()
        .find(|s| s.serializer == 2)
        .unwrap()
        .kind = StateKind::Sequence;
    data.game_mode
        .lifecycle
        .states
        .iter_mut()
        .find(|s| s.serializer == 2)
        .unwrap()
        .configuration = StateConfiguration::Sequence {
        sequence: ("test/outro".into(), 0),
    };
    let mut sequence = data.sequences[0].clone();
    sequence.spawner = Some(2);
    data.sequences.push(sequence);
    for (i, kind) in [(9, Kind::RpcReferences), (10, Kind::Rpc)] {
        data.game_mode.layout.fields.push(Field {
            kind,
            class: format!("test{i}"),
            asset: "test/drive".into(),
            object: i,
            binding: None,
            mirrors: None,
        });
    }
    data.game_mode.lifecycle.states.push(data::State {
        serializer: 9,
        kind: StateKind::GameMode,
        configuration: StateConfiguration::Default,
        handles_marked_for_delete: false,
        send_rpc_to_client: true,
    });
    data.game_mode.lifecycle.final_state = 9;
    data.game_mode.lifecycle.registered_states.push(9);
    data.game_mode.lifecycle.transitions.push(data::Transition {
        serializer: 10,
        kind: data::TransitionKind::Completed,
        sources: vec![data::Endpoint {
            serializer: 2,
            port: 0,
        }],
        target: Some(data::Endpoint {
            serializer: 9,
            port: 0,
        }),
        configuration: data::TransitionConfiguration::Default,
    });
    data.game_mode.lifecycle.registered_transitions.push(10);
    data.game_mode
        .lifecycle
        .bindings
        .order
        .push(Observer::Transition(10));
    let mut sequences = base.sequences;
    sequences.insert(2, sequences[&1].clone());
    let mut mode = Mode::new(data, base.parent, sequences).unwrap();
    mode.opponents = base.opponents;
    mode.timing.insert(2, 2.);
    let ai = mode.opponents().unwrap();
    let selections = crate::event_ai::Cursor::default().select(ai).unwrap();
    let mut created = ai
        .create(
            &mut players,
            &mut registry,
            owner.connection,
            owner.persona,
            &selections,
        )
        .unwrap();
    let ids = created
        .racers
        .members
        .iter()
        .map(|m| u64::from(m.ids.participant))
        .collect::<BTreeSet<_>>();
    let (mut active, _) = Active::start(&mode, &mut players, owner, 12.).unwrap();
    active
        .admit(&mode, &mut players, &mut created.racers)
        .unwrap();
    let intro = active.sequences[&3].ghost(owner.participant).unwrap();
    active
        .sequences
        .get_mut(&3)
        .unwrap()
        .stop(&mode.sequences[&3], &players, owner)
        .unwrap();
    active
        .completed(
            &mode,
            &mut players,
            Completed {
                sequence: intro,
                participant: owner.participant,
            },
            123,
        )
        .unwrap();
    let start = active.advance(&mode, &mut players, 12.1).unwrap();
    assert_eq!(start.records.iter().filter(|r| matches!(&r.initial, Some(replication::Initial::Entity { fields, .. }) if matches!(fields.first(), Some(entity::Initial::SequenceRoot { .. })))).count(), 1);
    active.advance(&mode, &mut players, 14.099).unwrap();
    assert!(active.host.machine().members(9).unwrap().is_empty());
    let stopped = active.advance(&mode, &mut players, 14.101).unwrap();
    assert_eq!(active.host.machine().members(9).unwrap(), &ids);
    assert_eq!(
        active.host.machine().members(2).unwrap(),
        &BTreeSet::from([u64::from(owner.participant)])
    );
    assert!(!active.ending);
    assert_eq!(
        stopped
            .calls
            .iter()
            .filter(|c| matches!(c, HostRpc::SequenceStop(_)))
            .count(),
        1
    );
    assert!(
        active
            .advance(&mode, &mut players, 15.)
            .unwrap()
            .calls
            .is_empty()
    );
    let outro = active.sequences[&2].ghost(owner.participant).unwrap();
    active
        .completed(
            &mode,
            &mut players,
            Completed {
                sequence: outro,
                participant: owner.participant,
            },
            123,
        )
        .unwrap();
    assert!(active.ending);
    assert_eq!(active.host.machine().members(9).unwrap().len(), 3);
}

#[test]
fn admission_is_owned_and_shared_native_completion_advances_every_racer_once() {
    let (mut players, owner, mode, mut registry) = fixture();
    let selection = crate::event_ai::Cursor::default()
        .select(mode.opponents().unwrap())
        .unwrap();
    let mut created = mode
        .opponents()
        .unwrap()
        .create(
            &mut players,
            &mut registry,
            owner.connection,
            owner.persona,
            &selection,
        )
        .unwrap();
    let ids = created
        .racers
        .members
        .iter()
        .map(|m| m.ids.participant)
        .collect::<Vec<_>>();
    let (mut active, _) = Active::start(&mode, &mut players, owner, 12.).unwrap();
    let initial = players.objects().snapshot();
    let mut foreign = created.racers.clone();
    foreign.members[1].ids.actor += 1;
    assert!(active.admit(&mode, &mut players, &mut foreign).is_err());
    assert_eq!(players.objects().snapshot(), initial);
    assert!(active.opponents.is_none());
    assert_eq!(foreign.members.len(), 2);
    let admitted = active
        .admit(&mode, &mut players, &mut created.racers)
        .unwrap();
    assert!(created.racers.members.is_empty());
    assert!(
        admitted.records.is_empty(),
        "no extra human sequence per AI"
    );
    assert_eq!(
        admitted.calls.len(),
        8,
        "root joins, wait entries/leaves and intro entries"
    );
    let members = std::iter::once(u64::from(owner.participant))
        .chain(ids.iter().copied().map(u64::from))
        .collect::<BTreeSet<_>>();
    assert_eq!(active.host.machine().members(3).unwrap(), &members);
    let intro = active.sequences[&3].ghost(owner.participant).unwrap();
    active
        .sequences
        .get_mut(&3)
        .unwrap()
        .stop(&mode.sequences[&3], &players, owner)
        .unwrap();
    let call = Completed {
        sequence: intro,
        participant: owner.participant,
    };
    assert!(
        active
            .completed(
                &mode,
                &mut players,
                Completed {
                    participant: ids[0],
                    ..call
                },
                123
            )
            .is_err()
    );
    active.completed(&mode, &mut players, call, 123).unwrap();
    assert_eq!(active.host.machine().members(1).unwrap(), &members);
    let sequence = active.sequences[&1].ghost(owner.participant).unwrap();
    active.advance(&mode, &mut players, 12.1).unwrap();
    assert_eq!(active.host.machine().members(2).unwrap(), &members);
    let ack = Completed {
        sequence,
        participant: owner.participant,
    };
    assert_eq!(
        active
            .completed(&mode, &mut players, ack, 123)
            .unwrap()
            .deleted,
        vec![sequence]
    );
    assert!(
        active
            .completed(&mode, &mut players, ack, 123)
            .unwrap()
            .deleted
            .is_empty()
    );
    assert_eq!(active.opponents().unwrap().members.len(), 2);
    assert_eq!(active.delegated(&players).len(), 2);
    let before = players.objects().snapshot();
    let mut invalid = active.clone();
    invalid.opponents.as_mut().unwrap().members[1].ids.actor += 1;
    assert!(
        invalid
            .leave(&mode, &mut players, owner.participant)
            .is_err()
    );
    assert_eq!(players.objects().snapshot(), before);
    assert!(!invalid.has_left());
    assert!(active.leave(&mode, &mut players, ids[0]).is_err());
    let retired = active
        .leave(&mode, &mut players, owner.participant)
        .unwrap();
    assert!(active.has_left());
    assert!(active.delegated(&players).is_empty());
    assert!(active.locations(&players).unwrap().is_empty());
    assert!(retired.deleted.contains(&active.ghost));
    assert!(!retired.departures.is_empty());
    for id in ids {
        assert!(retired.deleted.contains(&id));
        assert!(players.objects().get(id).is_none());
    }
    assert!(players.owns_participant(owner.connection, owner.persona, owner.participant));
    let repeated = active
        .leave(&mode, &mut players, owner.participant)
        .unwrap();
    assert!(repeated.deleted.is_empty() && repeated.departures.is_empty());
}
