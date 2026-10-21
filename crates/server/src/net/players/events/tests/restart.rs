// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::{
    participants::{Bindings, Lifecycle, Outcome, Stage},
    replication::sublevel,
};

fn presentation_fixture(h: &mut Harness) {
    use crate::garage_logic::{GarageLogic, Which};
    use serde_json::json;
    let mut v = crate::garage_logic::tests::presentation_json();
    let mut definition = v["entry"]["entities"][0].clone();
    definition["blueprint"] = json!({"scene_content_key": fixture::RACE});
    definition["roles"][1] =
        json!({"kind":"channel", "scene_content_key":fixture::RACE, "scene_serializer":2});
    let entities = |ids: &[u16]| {
        ids.iter()
            .map(|id| {
                let mut d = definition.clone();
                d["capture_id"] = json!(id);
                d
            })
            .collect::<Vec<_>>()
    };
    v["spawn"] = json!({"scene_updates":[], "entities": entities(&[18,19,20,21])});
    v["entry"] = json!({"scene_updates":[], "entities":[]});
    v["vehicles"] = json!({"scene_updates":[], "entities":[]});
    for phase in [
        "exit",
        "arrive",
        "arrived",
        "world_ready",
        "world_spawn",
        "world_settled",
        "free_roam",
        "in_world_teleport",
        "in_world_arrival",
    ] {
        for event in v[phase]["events"].as_array_mut().unwrap() {
            event["target"] = json!({"scene_content_key": fixture::RACE});
        }
    }
    v["arrive"]["delete_capture_ids"] = json!([18, 19]);
    v["arrived"]["delete_capture_ids"] = json!([20]);
    v["arrived"]["entities"] = json!(entities(&[22, 23, 24]));
    v["in_world_teleport"]["delete_capture_ids"] = json!([21, 22, 23, 24]);
    v["in_world_teleport"]["entities"] = json!(entities(&[18, 19, 20, 21]));
    v["in_world_arrival"]["delete_capture_ids"] = json!([18, 19]);
    let logic = Arc::new(GarageLogic::from_json(&v).unwrap());
    for which in [Which::Spawn, Which::Arrive, Which::Arrived] {
        PlayerListener::arrive_logic(
            Some(&logic),
            which,
            &mut h.listener.players,
            h.participant,
            &mut h.listener.logic_ghosts,
        )
        .unwrap();
    }
    h.listener.logic = Some(logic);
}

pub(super) fn configure(h: &mut Harness) {
    presentation_fixture(h);
    let mut event = h.content.event().clone();
    event.restart = Some(vec![
        nfs_content::events::restart::Destination {
            position: [20., 3., -15.],
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        },
        nfs_content::events::restart::Destination {
            position: [-20., 3., -15.],
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        },
    ]);
    let mut arrival = event.sequences[0].clone();
    arrival.layout.blueprint = names::RESTART_ARRIVAL.into();
    arrival.layout.asset.local_index = 39;
    arrival.layout.fields.truncate(1);
    arrival.layout.bus_count = 1;
    arrival.spawner = None;
    arrival.goto = None;
    arrival.wander = None;
    event.sequences.push(arrival);
    h.content = Arc::new(event_logic::Content::new(event, "test".into()).unwrap());
    h.listener.events = Some(h.content.clone());
    let mut gameplay = vec![sublevel::Kind::RpcReferences; 13];
    gameplay[0] = sublevel::Kind::Noop;
    gameplay[8..13].fill(sublevel::Kind::Rpc);
    let mut startup = vec![sublevel::Kind::RpcReferences; 117];
    startup[0] = sublevel::Kind::Noop;
    let mut garage = vec![sublevel::Kind::Rpc; 10];
    garage[0] = sublevel::Kind::Noop;
    let profiles = sublevel::Content::new(vec![
        (995, sublevel::Profile::new(false, &gameplay).unwrap()),
        (996, sublevel::Profile::new(false, &startup).unwrap()),
        (997, sublevel::Profile::new(false, &garage).unwrap()),
    ])
    .unwrap();
    let creations = [995, 996, 997]
        .into_iter()
        .enumerate()
        .map(|(i, key)| {
            sublevel::ordinary::creation(
                70 + i as u16,
                key,
                None,
                Serial::new(2).unwrap(),
                &sublevel::ordinary::unpopulated(profiles.profile(key).unwrap()).unwrap(),
                &profiles,
            )
            .unwrap()
        })
        .collect();
    h.listener
        .players
        .create_scenes(&[70, 71, 72], creations, &profiles)
        .unwrap();
    let records = h.world().snapshot();
    let mut life = Lifecycle::default();
    life.bind(&records, 995, 996).unwrap();
    life.bind_exit(&records, 996, 997).unwrap();
    let b = Bindings::from_records(&records, 995, 996).unwrap().unwrap();
    let receive = |life: &mut Lifecycle, endpoint: Endpoint, method| {
        let Input::Rpc(bytes, bits) = rpc(
            &[endpoint.scene, h.participant],
            endpoint.selector,
            method,
            &[(0, 7)],
        ) else {
            unreachable!()
        };
        life.receive(BitSpan::new(&bytes, 0, bits).unwrap(), |p| {
            p == h.participant
        })
        .unwrap()
    };
    life.begin(h.participant).unwrap();
    life.set_garage([None; 5]).unwrap();
    receive(&mut life, b.inventory, 3);
    life.take_garage_bindings();
    life.finish_persistent(|_| true);
    assert!(life.begin_progression(h.participant));
    life.enter_garage_loading(h.participant);
    assert!(matches!(
        life.enter_customization(h.participant),
        Outcome::Advanced(_)
    ));
    let x = life.exit_bindings().unwrap();
    for (endpoint, method) in [(x.exit_request, 0), (x.state_77, 3), (x.ready_request, 0)] {
        assert!(matches!(
            receive(&mut life, endpoint, method),
            Outcome::Advanced(_)
        ));
    }
    life.take_exited();
    life.take_arriving();
    assert!(matches!(life.arrived(h.participant), Outcome::Advanced(_)));
    h.listener.participants = life;
    h.listener.event_state.active.insert(h.participant);
    h.listener.free_roam_presented.insert(h.participant);
}

#[test]
fn restart_waits_for_acknowledgements_resumes_and_can_start_again_without_a_reward() {
    let mut h = started();
    configure(&mut h);
    let p = h.participant;
    let target = h.race().logics[&Logic::Restart];
    // A blocked queue must retain both the request and its presentation state.
    h.listener
        .pending_entries
        .push_back((packed(Vec::new(), Vec::new()), Vec::new()));
    h.listener
        .event_state
        .inputs
        .push_back(logic(RESTART, target));
    let snapshot = h.world().snapshot();
    let roles = h.listener.logic_ghosts.clone();
    h.listener.advance_events(5000).unwrap();
    assert_eq!(h.world().snapshot(), snapshot);
    assert_eq!(h.listener.logic_ghosts, roles);
    assert_eq!(h.race().exit, None);
    h.listener.pending_entries.clear();
    let exiting = h.run(5000, vec![]);
    assert_eq!(
        exiting
            .entries
            .iter()
            .flat_map(|e| &e.1)
            .filter_map(|rpc| match rpc {
                HostRpc::Event(fire) => Some(fire.event),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![22, 23],
        "hide the world and hold black before the delayed race HUD release",
    );
    assert!(h.listener.event_suspended.contains(&p));
    let loading = h.listener.logic_ghosts[&p].clone();
    assert_eq!(
        loading.keys().copied().collect::<Vec<_>>(),
        [18, 19, 20, 21]
    );
    assert!(roles[&p].values().all(|id| h.world().get(*id).is_none()));
    let transition = &exiting.entries[0].0;
    assert!(roles[&p].values().all(|id| transition.deleted.contains(id)));
    assert!(
        loading
            .values()
            .all(|id| transition.records.iter().any(|r| r.id == *id))
    );
    h.run(5100, vec![]);
    let leave = h.leave();
    h.listener
        .pending_entries
        .push_back((packed(Vec::new(), Vec::new()), Vec::new()));
    h.listener.event_state.inputs.push_back(leave);
    let snapshot = h.world().snapshot();
    h.listener.advance_events(5200).unwrap();
    assert_eq!(h.world().snapshot(), snapshot);
    assert_eq!(h.listener.participants.stage(p), Some(Stage::FreeRoam));
    assert!(h.listener.event_state.restarts.is_empty());
    assert_eq!(h.listener.event_state.restart_cursor, 0);
    h.listener.pending_entries.clear();
    let left = h.run(5200, vec![]);
    assert_eq!(h.listener.participants.stage(p), Some(Stage::Relocating));
    assert!(h.listener.pending_teleports.is_empty());
    assert_eq!(left.entries.len(), 2, "mode teardown precedes relocation");
    assert_eq!(h.listener.event_state.restart_cursor, 1);
    let x = h.listener.participants.exit_bindings().unwrap();
    let ack = || rpc(&[x.state_77.scene, p], x.state_77.selector, 3, &[(0, 7)]);
    let ready = || {
        rpc(
            &[x.ready_request.scene, p],
            x.ready_request.selector,
            0,
            &[(0, 7)],
        )
    };
    // Early ready/teleport acknowledgements do not advance.
    h.run(5250, vec![ready(), ack()]);
    assert!(h.listener.pending_teleports.is_empty());
    h.run(5300, vec![]);
    h.run(5400, vec![]);
    let command = h.listener.due_teleport(5400).unwrap();
    assert_eq!(command.position, [20., 3., -15.]);
    assert_eq!((command.participant, command.vehicle), (p, h.car));
    h.listener.teleport_sent();
    h.run(5500, vec![ack(), ack()]);
    assert!(h.listener.pending_teleports.is_empty());
    let arrived = h.run(5600, vec![ready(), ready()]);
    let sequence = arrived
        .entries
        .iter()
        .flat_map(|e| &e.0.records)
        .find_map(|r| {
            matches!(
                &r.initial,
                Some(nfs_world::replication::Initial::Entity { .. })
            )
            .then_some(r.id)
        })
        .unwrap();
    assert_eq!(arrived.entries.iter().flat_map(|e| &e.0.records).count(), 1);
    let arrival_entry = arrived
        .entries
        .iter()
        .find(|e| !e.0.records.is_empty())
        .unwrap();
    assert!(matches!(&arrival_entry.1[0], HostRpc::Event(fire) if fire.event == 24));
    assert_eq!(
        arrival_entry.1.len(),
        3,
        "black-screen release and state calls share the arrival creation"
    );
    assert_eq!(h.listener.participants.stage(p), Some(Stage::Relocating));
    let complete = h.completed(sequence);
    assert_eq!(
        h.listener.logic_ghosts[&p]
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [20, 21]
    );
    assert!(arrival_entry.0.deleted.contains(&loading[&18]));
    assert!(arrival_entry.0.deleted.contains(&loading[&19]));
    h.run(5700, vec![complete.clone(), complete]);
    assert!(h.world().get(sequence).is_none());
    assert_eq!(
        h.listener.logic_ghosts[&p]
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [21, 22, 23, 24]
    );
    assert!(h.world().get(loading[&20]).is_none());
    let preparing = h.run(5750, vec![]);
    assert!(preparing.rpcs.iter().any(|rpc| matches!(
        rpc, HostRpc::WorldActivation(n) if n.participant == Some(p)
            && matches!(n.call, world_activation::Call::Subscribe { .. })
    )));
    assert_eq!(h.listener.participants.stage(p), Some(Stage::Relocating));
    assert!(h.listener.event_suspended.contains(&p));
    h.run(5800, vec![]);
    assert_eq!(h.listener.participants.stage(p), Some(Stage::FreeRoam));
    assert!(!h.listener.event_suspended.contains(&p));
    h.run(5900, vec![]);
    h.run(6000, vec![h.start_crossing(2)]);
    let prompt = h.listener.event_state.entries[&p].area[1];
    h.run(6100, vec![logic(START, prompt)]);
    assert_eq!(h.race().phase, Phase::Intro);
    assert!(h.listener.scoring_state.pending_awards.is_empty());
    // A new restart selects the next authored slot; duplicate requests do not
    // consume another slot. Other listener sessions begin at their own slot 0.
    h.run(6200, vec![]);
    let target = h.race().logics[&Logic::Restart];
    h.run(6300, vec![logic(RESTART, target), logic(RESTART, target)]);
    h.run(6400, vec![]);
    h.run(6500, vec![h.leave()]);
    h.run(6600, vec![]);
    h.run(6700, vec![]);
    assert_eq!(
        h.listener.due_teleport(6700).unwrap().position,
        [-20., 3., -15.]
    );
    assert_eq!(h.listener.event_state.restart_cursor, 0);
    assert_eq!(Harness::new().listener.event_state.restart_cursor, 0);
    assert!(h.listener.scoring_state.pending_awards.is_empty());
    // A client that never acknowledges cannot retain a relocation forever.
    assert!(h.listener.advance_events(186_700).is_err());
    assert_eq!(h.listener.participants.stage(p), Some(Stage::Relocating));
    assert!(h.listener.disable_events());
    assert!(h.listener.advance_events(186_701).is_err());
    assert!(
        !h.listener.disable_events(),
        "a repeated cleanup fault closes the session"
    );
}

#[test]
fn restart_content_rejects_unbounded_invalid_or_unbound_destinations() {
    let mut h = Harness::new();
    configure(&mut h);
    let event = h.content.event().clone();
    let one = event.restart.as_ref().unwrap()[0];
    for destinations in [
        vec![],
        vec![one; nfs_content::events::restart::MAX_LOCATIONS + 1],
        vec![nfs_content::events::restart::Destination {
            position: [f32::NAN, 0., 0.],
            ..one
        }],
        vec![nfs_content::events::restart::Destination {
            basis: [[0.; 3]; 3],
            ..one
        }],
    ] {
        let mut invalid = event.clone();
        invalid.restart = Some(destinations);
        assert!(event_logic::Content::new(invalid, "test".into()).is_err());
    }
    let mut invalid = event.clone();
    invalid
        .sequences
        .retain(|s| s.layout.blueprint != names::RESTART_ARRIVAL);
    assert!(event_logic::Content::new(invalid, "test".into()).is_err());
    let mut invalid = event.clone();
    invalid
        .sequences
        .push(event.sequences.last().unwrap().clone());
    assert!(event_logic::Content::new(invalid, "test".into()).is_err());
}
