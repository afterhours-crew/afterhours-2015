// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
pub(crate) fn configure_shutdown(mode: &mut Mode, active: &mut Active, objective: u64) {
    mode.data.shutdown = Some(intro::shutdown::Binding {
        objective,
        source: Identity {
            asset: "test".into(),
            index: 0,
            instances: vec![],
        },
        event: 41,
    });
    active.callbacks = Some(super::reporters::tests::shutdown_fixture(
        active.owner.participant,
        mode.data.game_mode.lifecycle.initial as u16,
        mode.data.game_mode.lifecycle.final_state as u16,
    ));
}
use nfs_content::events::{
    Field, GameMode, Goto, Layout, Sequence,
    lifecycle::{self as data, StateConfiguration, bindings::Observer},
};
use nfs_world::{replication::entity::Kind, sequences::Component};

pub(crate) fn content(parent: Asset, manager: Component) -> (Mode, Mode) {
    let catalog = vec![(8, 1), (9, 2), (10, 2)];
    let field = |i, kind| Field {
        kind,
        class: format!("field{i}"),
        asset: "test/drive".into(),
        object: i,
        binding: None,
        mirrors: None,
    };
    let layout = Layout {
        blueprint: "test/drive".into(),
        asset: Asset {
            bundle: parent.bundle,
            type_id: 10,
            local_index: 0,
        },
        catalog: catalog.clone(),
        bus_count: 1,
        sub_id: 1,
        occurrence: 0,
        fields: [
            Kind::GameModeRoot,
            Kind::RpcReferences,
            Kind::WaypointsState,
            Kind::RpcReferences,
            Kind::Rpc,
            Kind::RpcReference,
            Kind::Rpc,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, k)| field(i, k))
        .collect(),
    };
    let mut layout = layout;
    layout.fields[5].binding = Some(Binding::Participant { serializer: 1 });
    let state = |serializer, kind, configuration| data::State {
        serializer,
        kind,
        configuration,
        handles_marked_for_delete: false,
        send_rpc_to_client: true,
    };
    let mut lifecycle = data::Lifecycle {
        imports: BTreeMap::new(),
        bindings: data::bindings::Bindings {
            order: vec![Observer::Transition(4)],
            ..Default::default()
        },
        initial: 1,
        final_state: 3,
        root_state: None,
        states: vec![
            state(
                1,
                StateKind::Sequence,
                StateConfiguration::Sequence {
                    sequence: ("test/id".into(), 0),
                },
            ),
            state(
                2,
                StateKind::Waypoints,
                StateConfiguration::Waypoints {
                    allowed_wrong_direction_distance: 50.,
                },
            ),
            state(3, StateKind::GameMode, StateConfiguration::Default),
        ],
        transitions: vec![data::Transition {
            serializer: 4,
            kind: data::TransitionKind::Completed,
            sources: vec![data::Endpoint {
                serializer: 1,
                port: 0,
            }],
            target: Some(data::Endpoint {
                serializer: 2,
                port: 0,
            }),
            configuration: data::TransitionConfiguration::Default,
        }],
        registered_states: vec![1, 2, 3],
        registered_transitions: vec![4],
    };
    let identity = |index| Identity {
        asset: "test/drive".into(),
        index,
        instances: vec![],
    };
    lifecycle.transitions.push(data::Transition {
        serializer: 6,
        kind: data::TransitionKind::FinishLine,
        sources: vec![data::Endpoint {
            serializer: 2,
            port: 0,
        }],
        target: Some(data::Endpoint {
            serializer: 3,
            port: 0,
        }),
        configuration: data::TransitionConfiguration::FinishLine { laps: 1 },
    });
    lifecycle.registered_transitions.push(6);
    lifecycle.bindings.order.push(Observer::Transition(6));
    let state = data::Endpoint {
        serializer: 2,
        port: 0,
    };
    let route = data::bindings::Link {
        identity: identity(10),
        port: 0,
        class: "ChannelWaypointListProviderEntityData".into(),
        channel: None,
    };
    let manager_property = Property {
        channel: "test/managers".into(),
        port: 1,
    };
    lifecycle.bindings.order.push(Observer::Ai(0));
    lifecycle
        .bindings
        .ai_controls
        .push(data::bindings::ai::Control {
            identity: identity(11),
            context_hash: 17,
            one_by_one: false,
            apply_existing: false,
            states: vec![state],
            commands: vec![identity(12)],
            command_manager: manager_property.clone(),
            actor_manager: manager_property,
            game_mode: None,
        });
    lifecycle.bindings.ai_commands.insert(
        identity(12),
        data::bindings::ai::Command::Waypoints {
            state,
            route,
            drift: false,
        },
    );
    let mut sequence = Sequence {
        layout: Layout {
            blueprint: "test/drive-intro".into(),
            asset: Asset {
                bundle: parent.bundle,
                type_id: 9,
                local_index: 1,
            },
            catalog: catalog.clone(),
            bus_count: 1,
            sub_id: 2,
            occurrence: 0,
            fields: [
                Kind::SequenceRoot,
                Kind::RpcReference,
                Kind::RpcReference,
                Kind::RpcReferences,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, k)| field(i, k))
            .collect(),
        },
        manager: (manager.scene_key, manager.index),
        spawner: Some(1),
        goto: Some(Goto {
            point: [30., 4., 50.],
            radius: 20.,
            speed_limit: 8.,
        }),
        wander: None,
    };
    sequence.layout.fields[1].binding = Some(Binding::Participant { serializer: 1 });
    sequence.layout.fields[2].binding = Some(Binding::GameMode { serializer: 5 });
    let authored = Authored::from_mode(
        &sequence,
        parent,
        layout.asset,
        vec![nfs_world::scoring::event_sync::Binding {
            entity: 1,
            event: 43,
            node: 0,
        }],
    )
    .unwrap();
    let data = intro::Mode {
        shutdown: None,
        owned_filters: vec![],
        completion: None,
        game_mode: GameMode {
            layout,
            lifecycle,
            root_bindings: [(manager.scene_key, manager.index); 3],
            checkpoints: vec![(manager.scene_key, manager.index)],
            points: vec![[70., 4., 90.]],
            checkpoint_points: vec![],
            laps: 1,
            drift: Some(false),
            pacing: None,
            standings_interval: None,
            recovery: None,
        },
        launcher: 1,
        sequences: vec![sequence],
        logics: vec![],
    };
    let mut cancel = data.clone();
    cancel.game_mode.layout.blueprint = "test/cancel".into();
    cancel.game_mode.layout.asset.local_index = 1;
    cancel.sequences.clear();
    cancel.game_mode.lifecycle.states[0].kind = StateKind::GameMode;
    cancel.game_mode.lifecycle.states[0].configuration = StateConfiguration::Default;
    cancel.game_mode.lifecycle.transitions.clear();
    cancel.game_mode.lifecycle.registered_transitions.clear();
    cancel.game_mode.lifecycle.bindings.order.clear();
    cancel.game_mode.lifecycle.bindings.ai_controls.clear();
    cancel.game_mode.lifecycle.bindings.ai_commands.clear();
    cancel.game_mode.checkpoints.clear();
    (
        Mode::new(data, parent, BTreeMap::from([(1, authored)])).unwrap(),
        Mode::new(cancel, parent, BTreeMap::new()).unwrap(),
    )
}

fn clock_fixture(countdown: bool) -> (Players, Owner, Mode) {
    clock_fixture_in(countdown, Players::default())
}
pub(super) fn clock_fixture_in(countdown: bool, mut players: Players) -> (Players, Owner, Mode) {
    use nfs_protocol::world::rpc::Serial;
    use nfs_world::replication::{
        entity::{
            Body, Initial, Profile, Rpc, Update,
            creation::{Creation, Prefix},
        },
        players::Request,
        sublevel,
    };
    let scenes = sublevel::Content::new(vec![(
        7,
        sublevel::Profile::new(false, &[sublevel::Kind::Noop, sublevel::Kind::Rpc]).unwrap(),
    )])
    .unwrap();
    let scene = sublevel::passive::creation(777, 7, Serial::new(1).unwrap(), &scenes).unwrap();
    let scene_id = players.create_scenes(&[777], vec![scene], &scenes).unwrap()[0].id;
    let player = players
        .create(
            4,
            10,
            Request {
                name: b"Clock".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let owner = Owner {
        connection: 4,
        persona: 10,
        participant: players.join(4, 10, player).unwrap().unwrap().id,
    };
    let parent = Asset {
        bundle: 4,
        type_id: 8,
        local_index: 0,
    };
    let codec = CreationContent::new(
        vec![Catalog::new(4, &[(8, 1), (9, 2), (10, 2)]).unwrap()],
        vec![(parent, Profile::new(&[Kind::Root, Kind::Rpc]).unwrap())],
    )
    .unwrap();
    players
        .spawn_entities(
            vec![Creation {
                prefix: Prefix {
                    parent: None,
                    blueprint: scene_id,
                    sub_id: 1,
                    owner: None,
                    asset: parent,
                },
                body: Body {
                    initial: Some(vec![
                        Initial::Root {
                            value: 1,
                            rpc: Rpc {
                                selector: 0,
                                serial: Serial::new(1).unwrap(),
                            },
                            reference: owner.participant,
                        },
                        Initial::Rpc(Rpc {
                            selector: 1,
                            serial: Serial::new(1).unwrap(),
                        }),
                    ]),
                    updates: vec![Some(Update::Noop); 2],
                },
            }],
            &codec,
        )
        .unwrap();
    let (base, _) = content(
        parent,
        Component {
            scene_key: 7,
            index: 1,
        },
    );
    let mut data = base.data;
    data.sequences[0].goto = None;
    data.game_mode.layout.fields[2].kind = Kind::RpcReferences;
    data.game_mode.lifecycle.states[1].kind = StateKind::GameMode;
    data.game_mode.lifecycle.states[1].configuration = StateConfiguration::Default;
    data.game_mode.lifecycle.final_state = 2;
    data.game_mode.lifecycle.transitions.truncate(1);
    data.game_mode.lifecycle.registered_transitions = vec![4];
    data.game_mode.lifecycle.bindings = data::bindings::Bindings {
        order: vec![Observer::Transition(4)],
        ..Default::default()
    };
    if countdown {
        let clock = Property {
            channel: "test/clock".into(),
            port: 1,
        };
        data.game_mode.layout.fields[1].kind = Kind::CountdownState;
        data.game_mode.lifecycle.states[0].kind = StateKind::Countdown;
        data.game_mode.lifecycle.states[0].configuration = StateConfiguration::Countdown {
            sequence: ("test/id".into(), 0),
            seconds: 0.,
        };
        data.game_mode.lifecycle.states[2].kind = StateKind::Sequence;
        data.game_mode.lifecycle.states[2].configuration = StateConfiguration::Sequence {
            sequence: ("test/intro-id".into(), 0),
        };
        let mut intro = data.sequences[0].clone();
        intro.spawner = Some(3);
        data.sequences.push(intro);
        data.game_mode.lifecycle.initial = 3;
        data.game_mode.lifecycle.bindings.clocks.insert(1, clock);
        data.game_mode.lifecycle.transitions.push(data::Transition {
            serializer: 6,
            kind: data::TransitionKind::Completed,
            sources: vec![data::Endpoint {
                serializer: 3,
                port: 0,
            }],
            target: Some(data::Endpoint {
                serializer: 1,
                port: 0,
            }),
            configuration: data::TransitionConfiguration::Default,
        });
        data.game_mode.lifecycle.registered_transitions.push(6);
        data.game_mode
            .lifecycle
            .bindings
            .order
            .push(Observer::Transition(6));
    }
    let mut sequences = base.sequences;
    if countdown {
        sequences.insert(3, sequences[&1].clone());
    }
    let mut mode = Mode::new(data, parent, sequences).unwrap();
    if !countdown {
        mode.timing.insert(1, 2.);
    }
    (players, owner, mode)
}

#[test]
fn final_dependency_publications_wait_for_the_owned_leave_acknowledgement() {
    let (mut players, owner, base) = clock_fixture(false);
    let mut data = base.data;
    data.game_mode.lifecycle.states[1].kind = StateKind::Simple;
    data.game_mode.lifecycle.root_state = Some(3);
    data.game_mode.lifecycle.transitions.push(data::Transition {
        serializer: 6,
        kind: data::TransitionKind::Completed,
        sources: vec![data::Endpoint {
            serializer: 2,
            port: 0,
        }],
        target: Some(data::Endpoint {
            serializer: 3,
            port: 0,
        }),
        configuration: data::TransitionConfiguration::Default,
    });
    data.game_mode
        .lifecycle
        .bindings
        .order
        .push(Observer::Transition(6));
    let mut mode = Mode::new(data, base.parent, base.sequences).unwrap();
    mode.timing = base.timing;
    let (mut active, _) = Active::start(&mode, &mut players, owner, 12.).unwrap();
    assert!(
        active
            .leave(&mode, &mut players, owner.participant)
            .is_err()
    );
    active.advance(&mode, &mut players, 15.).unwrap();
    let sequence = active.sequences[&1].ghost(owner.participant).unwrap();
    let out = active
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
    let calls = |values: &[HostRpc]| {
        values
            .iter()
            .filter_map(|v| match v {
                HostRpc::GameMode(c) => Some((c.selector, c.method, c.reference)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        calls(&out.calls),
        vec![
            (1, 1, Some(owner.participant)),
            (2, 0, Some(owner.participant)),
            (0, 1, None)
        ]
    );
    assert!(active.ending);
    // The shared hook completes normally, even while publication awaits ACK.
    assert!(active.host.machine().members(2).unwrap().is_empty());
    assert!(
        active
            .host
            .machine()
            .members(3)
            .unwrap()
            .contains(&u64::from(owner.participant))
    );
    assert!(
        active
            .advance(&mode, &mut players, 16.)
            .unwrap()
            .calls
            .is_empty()
    );
    let before = players.objects().snapshot();
    assert!(
        active
            .leave(&mode, &mut players, owner.participant + 1)
            .is_err()
    );
    assert_eq!(players.objects().snapshot(), before);
    let out = active
        .leave(&mode, &mut players, owner.participant)
        .unwrap();
    assert_eq!(calls(&out.departures), vec![(0, 7, None)]);
    assert_eq!(
        &calls(&out.calls)[..3],
        &[
            (2, 1, Some(owner.participant)),
            (3, 0, Some(owner.participant)),
            (0, 4, Some(owner.participant))
        ]
    );
    assert!(out.deleted.contains(&active.ghost));
    assert!(active.shutdown_calls.is_empty());
    let repeated = active
        .leave(&mode, &mut players, owner.participant)
        .unwrap();
    assert!(
        repeated.calls.is_empty() && repeated.departures.is_empty() && repeated.deleted.is_empty()
    );
}

pub(crate) fn result_fixture() -> (Players, Owner, Mode, Active) {
    let (mut players, owner, mut mode) = clock_fixture(false);
    let source = Identity {
        asset: "test/result".into(),
        index: 1,
        instances: vec![],
    };
    let collector = data::bindings::Link {
        identity: Identity {
            asset: "test/collector".into(),
            index: 2,
            instances: vec![],
        },
        port: 0,
        class: "ResultCollectorEntityData".into(),
        channel: None,
    };
    mode.data.completion = Some(intro::results::Completion {
        source: source.clone(),
        collector: collector.clone(),
        objective: 10,
    });
    let (mut active, _) = Active::start(&mode, &mut players, owner, 12.).unwrap();
    active.results.insert(
        owner.participant,
        event_logic::lifecycle::results::Snapshot {
            identity: source,
            collector,
            triggering_participant: u64::from(owner.participant),
            values: BTreeMap::new(),
        },
    );
    active.result_barrier = true;
    let mut out = Output::default();
    active.begin_finish(&players, &mut out).unwrap();
    assert!(out.calls.is_empty());
    (players, owner, mode, active)
}

#[test]
fn story_completion_belongs_to_the_human_and_exact_bound_result() {
    let (_, owner, mode, mut active) = result_fixture();
    let result = active.results.remove(&owner.participant).unwrap();
    active.results.insert(owner.participant + 1, result.clone());
    assert_eq!(
        active.pending_completion(&mode, owner.participant).unwrap(),
        None
    );
    assert!(
        active
            .pending_completion(&mode, owner.participant + 1)
            .is_err()
    );
    active.results.insert(owner.participant, result.clone());
    assert_eq!(
        active.pending_completion(&mode, owner.participant).unwrap(),
        Some(10)
    );
    active
        .results
        .get_mut(&owner.participant)
        .unwrap()
        .collector
        .identity
        .instances
        .push(("other/capability".into(), 9));
    assert!(active.pending_completion(&mode, owner.participant).is_err());
    active.results.insert(owner.participant, result);
    assert!(
        active
            .completion_committed(&mode, owner.participant + 1, 10)
            .is_err()
    );
    assert!(
        active
            .completion_committed(&mode, owner.participant, 20)
            .is_err()
    );
    assert!(active.result_barrier);
    assert_eq!(
        active
            .completion_committed(&mode, owner.participant, 10)
            .unwrap()
            .len(),
        2
    );
    assert!(!active.result_barrier);
    assert!(
        active
            .completion_committed(&mode, owner.participant, 10)
            .unwrap()
            .is_empty()
    );
    active.mark_completion_queued();
    assert_eq!(
        active.pending_completion(&mode, owner.participant).unwrap(),
        None
    );
}

#[test]
fn zero_countdown_stops_before_native_ack_and_does_not_transition_twice() {
    let (mut players, owner, mode) = clock_fixture(true);
    let (mut active, start) = Active::start(&mode, &mut players, owner, 12.).unwrap();
    assert_eq!(
        start.records.len(),
        2,
        "human immediately enters its sequence"
    );
    let intro = active.sequences[&3].ghost(owner.participant).unwrap();
    active
        .sequences
        .get_mut(&3)
        .unwrap()
        .stop(&mode.sequences[&3], &players, owner)
        .unwrap()
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
    let call = Completed {
        sequence,
        participant: owner.participant,
    };
    assert!(active.completed(&mode, &mut players, call, 123).is_err());
    assert!(
        active
            .advance(&mode, &mut players, 12.)
            .unwrap()
            .calls
            .is_empty()
    );
    assert!(active.advance(&mode, &mut players, 11.).is_err());
    let tick = active.advance(&mode, &mut players, 12.01).unwrap();
    assert!(matches!(tick.calls.first(), Some(HostRpc::SequenceStop(_))));
    assert_eq!(
        active.host.machine().members(2).unwrap(),
        &BTreeSet::from([u64::from(owner.participant)])
    );
    let ack = active.completed(&mode, &mut players, call, 123).unwrap();
    assert_eq!(ack.deleted, vec![sequence]);
    assert!(
        ack.calls.is_empty(),
        "native acknowledgement must not advance membership twice"
    );
    assert!(
        active
            .completed(&mode, &mut players, call, 123)
            .unwrap()
            .deleted
            .is_empty()
    );
    assert!(
        active
            .advance(&mode, &mut players, 13.)
            .unwrap()
            .calls
            .is_empty()
    );
}

#[test]
fn authored_timer_stops_once_and_still_requires_the_owned_native_acknowledgement() {
    let (mut players, owner, mode) = clock_fixture(false);
    let (mut active, _) = Active::start(&mode, &mut players, owner, 20.).unwrap();
    let sequence = active.sequences[&1].ghost(owner.participant).unwrap();
    assert!(
        active
            .advance(&mode, &mut players, 21.99)
            .unwrap()
            .calls
            .is_empty()
    );
    assert!(matches!(
        active
            .advance(&mode, &mut players, 22.)
            .unwrap()
            .calls
            .as_slice(),
        [HostRpc::SequenceStop(_)]
    ));
    assert!(
        active
            .advance(&mode, &mut players, 23.)
            .unwrap()
            .calls
            .is_empty()
    );
    assert!(
        active
            .completed(
                &mode,
                &mut players,
                Completed {
                    sequence,
                    participant: owner.participant + 1
                },
                123
            )
            .is_err()
    );
    let out = active
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
    assert_eq!(out.deleted, vec![sequence]);
}
