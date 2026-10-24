// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::events::{
    Field, GameMode, Goto, Layout, Sequence,
    lifecycle::{self as data, bindings::Observer},
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
