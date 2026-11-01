// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Constructed race transactions with more spot subscriptions than a normal
//! per-tick sweep. No capture bytes or installed content are required.
use super::*;
use crate::prologue::volumes::WorldVolumes;
use nfs_content::events::discovery::{Endpoint, volumes::Binding};
use nfs_content::scoring::events::Identity;
use nfs_world::replication::sublevel;

const SPOTS: u32 = 9_000;
const COUNT: usize = 20;

fn categories(h: &mut Harness) {
    let mut event = h.content.event().clone();
    event.categories = BTreeSet::from([41]);
    h.content = Arc::new(event_logic::Content::new(event, h.content.category().into()).unwrap());
    h.listener.events = Some(Arc::new(h.content.as_ref().clone().into()));
}

fn configure(h: &mut Harness) {
    categories(h);
    let mut kinds = vec![sublevel::Kind::RpcReferences; COUNT + 1];
    kinds[0] = sublevel::Kind::Noop;
    let profiles = sublevel::Content::new(vec![(
        SPOTS,
        sublevel::Profile::new(false, &kinds).unwrap(),
    )])
    .unwrap();
    let ghost = 60;
    assert!(h.world().get(ghost).is_none());
    h.listener
        .players
        .create_scenes(
            &[ghost],
            vec![
                sublevel::ordinary::creation(
                    ghost,
                    SPOTS,
                    None,
                    Serial::new(0).unwrap(),
                    &sublevel::ordinary::unpopulated(profiles.profile(SPOTS).unwrap()).unwrap(),
                    &profiles,
                )
                .unwrap(),
            ],
            &profiles,
        )
        .unwrap();
    let manager = Identity {
        asset: "test/player-manager".into(),
        index: 7,
        instances: vec![],
    };
    let program: nfs_content::events::discovery::membership::Program =
        serde_json::from_value(serde_json::json!({
            "ops": [
                {"Fact": {"Player": manager}},
                {"Fact": {"GameMode": {"categories": [41], "all": false}}},
                {"Combine": ["Difference", 0, 1]}
            ], "ranges": [], "empty_sets": []
        }))
        .unwrap();
    let spots: BTreeSet<_> = (1..=COUNT)
        .map(|serializer| Endpoint {
            scene_key: SPOTS,
            serializer,
        })
        .collect();
    let (template, _, _) = crate::net::players::prologue::tests::setup();
    let mut content = template.prologue.as_deref().unwrap().clone();
    content.membership = None;
    content.mode_notifications.clear();
    content.volumes = WorldVolumes {
        spot_memberships: spots
            .iter()
            .map(|&endpoint| Binding {
                endpoint,
                program: program.clone(),
            })
            .collect(),
        spot_player_manager: Some(manager),
        driving_order: spots.iter().rev().copied().collect(),
        spots,
        ..WorldVolumes::default()
    };
    h.listener.prologue = Some(Arc::new(content));
    h.listener.world_volumes =
        crate::net::players::prologue::volumes::State::normal_for_test(h.participant);
}

fn members(h: &Harness) -> usize {
    h.listener
        .prologue
        .as_ref()
        .unwrap()
        .volumes
        .spots
        .iter()
        .filter(|&&s| h.listener.world_volumes.member(h.participant, s))
        .count()
}

fn spot_calls(h: &Harness, delivered: &Delivered, method: u32) -> Vec<usize> {
    let scene = h.world().scene(SPOTS).unwrap();
    delivered
        .rpcs
        .iter()
        .enumerate()
        .filter_map(|(i, rpc)| match rpc {
            HostRpc::GameMode(call) if call.target == scene => {
                assert_eq!(call.method, method);
                assert_eq!(call.reference, Some(h.participant));
                assert_eq!(
                    call.arguments,
                    if method == 0 {
                        Arguments::VolumeEntry {
                            value: 0,
                            flag: false,
                        }
                    } else {
                        Arguments::None
                    }
                );
                Some(i)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn race_start_reserves_all_spot_removals_or_retries_the_unchanged_transaction() {
    let mut h = Harness::new();
    configure(&mut h);
    h.listener.pending_rpcs.clear();
    let initial = h.run(0, vec![]);
    assert_eq!(spot_calls(&h, &initial, 0).len(), COUNT);
    assert_eq!(members(&h), COUNT);
    h.run(1000, vec![h.start_crossing(2)]);
    let area = h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()]
        .area
        .clone();
    let before_world = h.world().snapshot();
    let before_volumes = h.listener.world_volumes.clone();
    let dummy = initial.rpcs[0];
    h.listener.pending_rpcs =
        std::iter::repeat_n(dummy, nfs_world::session::MAX_RPC_QUEUE - 10).collect();
    let full = h.run(2000, vec![logic(START, area[1])]);
    assert!(full.created().is_empty());
    assert_eq!(h.world().snapshot(), before_world);
    assert_eq!(h.listener.world_volumes, before_volumes);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
    assert_eq!(
        h.listener.event_state.inputs.len(),
        1,
        "retry retains the start input"
    );
    let started = h.run(2000, vec![]);
    assert_eq!(
        spot_calls(&h, &started, 1).len(),
        COUNT,
        "not a partial 15-call sweep"
    );
    assert_eq!(members(&h), 0);
    assert!(h.listener.event_state.inputs.is_empty());
    let later = h.run(2100, vec![]);
    assert!(
        spot_calls(&h, &later, 1).is_empty(),
        "no duplicate retirement"
    );
    assert!(h.race().flags);
}

#[test]
fn paid_finish_restores_at_host_release_before_delayed_client_leave() {
    let mut h = started();
    configure(&mut h);
    let mut h = speedwall::finish(h);
    let gm = h.race().game_mode;
    let batch = h.race().credit_batch.unwrap();
    assert!(
        !h.listener.event_suspended.contains(&h.participant),
        "outro has released driving"
    );
    assert!(h.race().flags, "mode is still held until its paid result");
    assert_eq!(members(&h), 0);
    let unpaid = h.run(8300, vec![]);
    assert!(spot_calls(&h, &unpaid, 0).is_empty());
    h.listener.event_credit_committed(h.participant, batch);
    let dummy = HostRpc::GameMode(
        h.content
            .state_call(h.world(), gm, Gm::Root, 2, None, Arguments::None)
            .unwrap(),
    );
    h.listener.pending_rpcs =
        std::iter::repeat_n(dummy, nfs_world::session::MAX_RPC_QUEUE - 10).collect();
    let before = h.listener.world_volumes.clone();
    h.run(8400, vec![]);
    assert!(h.race().flags);
    assert!(!h.race().awaiting_leave);
    assert_eq!(h.listener.world_volumes, before);
    let released = h.run(8500, vec![]);
    let release_index = released
        .rpcs
        .iter()
        .position(|rpc| {
            matches!(rpc,
        HostRpc::GameMode(c) if c.target == gm && c.selector == 0 && c.method == 1)
        })
        .unwrap();
    let restored = spot_calls(&h, &released, 0);
    assert_eq!(restored.len(), COUNT);
    assert!(restored.iter().all(|&index| index > release_index));
    assert_eq!(members(&h), COUNT);
    assert!(!h.race().flags);
    assert!(h.race().awaiting_leave);
    assert!(
        h.world().get(gm).is_some(),
        "release precedes object deletion"
    );
    h.listener.pending_speedwalls.clear();
    let waiting = h.run(9000, vec![]);
    assert!(spot_calls(&h, &waiting, 0).is_empty());
    let left = h.run(12000, vec![h.leave()]);
    assert!(spot_calls(&h, &left, 0).is_empty());
    assert_eq!(members(&h), COUNT);
}

#[test]
fn abandoned_or_restarting_mode_releases_eligible_spots_without_touching_another_session() {
    let mut other = Harness::new();
    configure(&mut other);
    other.run(0, vec![]);
    let other_before = other.listener.world_volumes.clone();
    for (choice, target_kind) in [(LEAVE, Logic::Leave), (RESTART, Logic::Restart)] {
        let mut h = started();
        if choice == RESTART {
            restart::configure(&mut h);
        }
        configure(&mut h);
        let target = h.race().logics[&target_kind];
        h.run(5000, vec![logic(choice, target)]);
        assert_eq!(members(&h), 0);
        let released = h.run(5100, vec![]);
        assert_eq!(spot_calls(&h, &released, 0).len(), COUNT);
        assert!(h.race().awaiting_leave);
        assert_eq!(members(&h), COUNT);
    }
    assert_eq!(other.listener.world_volumes, other_before);
    assert_eq!(members(&other), COUNT);
}

#[test]
fn held_mode_categories_distinguish_any_all_release_and_unknown_content() {
    let mut h = started();
    categories(&mut h);
    let both = BTreeSet::from([41, 42]);
    let catalog = h.listener.events.as_deref();
    let state = &h.listener.event_state;
    assert_eq!(
        state.matches_held_mode(h.participant, catalog, &both, false),
        Some(true)
    );
    assert_eq!(
        state.matches_held_mode(h.participant, catalog, &both, true),
        Some(false)
    );
    assert_eq!(
        state.matches_held_mode(h.participant, catalog, &BTreeSet::from([42]), false),
        Some(false)
    );
    assert_eq!(
        state.matches_held_mode(h.participant + 1, catalog, &both, false),
        Some(false)
    );
    assert_eq!(
        state.matches_held_mode(h.participant, None, &both, false),
        None
    );
    h.listener
        .event_state
        .entries
        .get_mut(&h.participant)
        .unwrap()
        .race
        .as_mut()
        .unwrap()
        .flags = false;
    assert_eq!(
        h.listener
            .event_state
            .matches_held_mode(h.participant, None, &both, false),
        Some(false)
    );
}
