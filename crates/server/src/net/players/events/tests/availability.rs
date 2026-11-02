// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::persistent::key;
use nfs_storage::tables::Value;

fn saved(active: bool, completed: bool) -> Arc<crate::persistent::Loaded> {
    let (_, _, mut snapshot) = crate::progression::tests::local();
    snapshot
        .tables
        .get_mut(&key("ProgressionObjective"))
        .unwrap()
        .rows
        .insert(
            30,
            BTreeMap::from([
                (key("Active"), Value::Int(i32::from(active))),
                (key("Completed"), Value::Int(i32::from(completed))),
            ]),
        );
    Arc::new(crate::progression::tests::tables().view(&snapshot).unwrap())
}
fn setup() -> Harness {
    let mut h = Harness::new();
    restart::configure(&mut h);
    h.listener.event_state = State::default();
    h.listener.pending_rpcs.clear();
    h.listener.progression = Some(Arc::new(
        crate::progression::Content::from_json(&crate::progression::tests::combined()).unwrap(),
    ));
    h.listener.persistent = Some(saved(false, false));
    let availability = event_logic::availability::gated_fixture(&h.content);
    h.listener.events = Some(Arc::new(
        Catalog::new_gated(vec![h.content.as_ref().clone()], availability).unwrap(),
    ));
    h
}

#[test]
fn save_unlock_publishes_once_and_completion_removes_prompts_and_stale_inputs() {
    let mut h = setup();
    let locked = h.run(1000, vec![h.start_crossing(2)]);
    assert!(locked.created().is_empty());
    assert!(locked.rpcs.is_empty());
    assert!(!h.listener.event_available(&h.content, h.participant));
    h.listener.persistent = Some(saved(true, false));
    let enabled = h.run(2000, vec![]);
    assert!(h.listener.event_available(&h.content, h.participant));
    assert_eq!(
        enabled
            .rpcs
            .iter()
            .filter(|r| matches!(r, HostRpc::WorldActivation(_)))
            .count(),
        1
    );
    assert_eq!(
        enabled.calls().len(),
        3,
        "map notification owner publishes once, not again from event_join"
    );
    assert!(h.run(2100, vec![]).rpcs.is_empty());
    h.run(2200, vec![h.start_crossing(2)]);
    let area = &h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()];
    let prompt = area.area[1];
    let old: Vec<_> = area.area.iter().chain(&area.filters).copied().collect();
    h.listener.persistent = Some(saved(true, true));
    let disabled = h.run(2300, vec![logic(START, prompt)]);
    assert!(old.iter().all(|g| disabled.deleted().contains(g)));
    assert_eq!(disabled.calls().iter().filter(|c| c.2 == 1).count(), 3);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
    assert!(!h.listener.event_available(&h.content, h.participant));
    assert!(
        h.run(2400, vec![h.start_crossing(2), logic(START, prompt)])
            .created()
            .is_empty()
    );
    assert_eq!(
        h.listener
            .observation(0, 0, 0, false)
            .health
            .event_availability_failures,
        0
    );
    // Rebuilt owners use saved completion, while another account stays isolated.
    let mut restarted = setup();
    restarted.listener.persistent = h.listener.persistent.clone();
    assert!(restarted.run(2500, vec![]).rpcs.is_empty());
    let mut other = setup();
    other.listener.persistent = Some(saved(true, false));
    assert!(!other.run(2500, vec![]).rpcs.is_empty());
}

#[test]
fn projection_failure_blocks_stale_start_under_backpressure_and_remains_in_health_after_recovery() {
    let mut h = setup();
    h.listener.persistent = Some(saved(true, false));
    h.run(1000, vec![]);
    h.run(1100, vec![h.start_crossing(2)]);
    let prompt =
        h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()].area[1];
    h.listener.persistent = None;
    h.listener
        .pending_entries
        .push_back((returning::empty_section(), vec![]));
    h.listener.advance_event_discovery().unwrap();
    assert!(!h.listener.event_available(&h.content, h.participant));
    // A direct stale start is denied even before queued removal can publish.
    let mut state = h.listener.event_state.clone();
    let mut players = h.listener.players.clone();
    let mut out = Out::default();
    h.listener
        .event_input(
            &h.content,
            &mut players,
            &mut state,
            &mut BTreeSet::new(),
            logic(START, prompt),
            1200,
            1.2,
            &mut out,
            &mut vec![],
        )
        .unwrap();
    assert!(state.entries[&h.participant].race.is_none());
    assert!(out.records.is_empty());
    h.listener.pending_entries.clear();
    h.run(1300, vec![]);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .areas
            .is_empty()
    );
    let before = h.listener.observation(0, 0, 0, false).health;
    assert!(before.event_availability_failures > 0);
    assert_eq!(before.refused_frames, 0);
    h.listener.persistent = Some(saved(true, false));
    h.run(1400, vec![]);
    assert!(h.listener.event_available(&h.content, h.participant));
    assert_eq!(
        h.listener
            .observation(0, 0, 0, false)
            .health
            .event_availability_failures,
        before.event_availability_failures
    );
}

#[test]
fn completion_keeps_running_race_cleanup_but_denies_restart_of_locked_variant() {
    let mut h = setup();
    h.listener.persistent = Some(saved(true, false));
    h.run(1000, vec![]);
    h.run(1100, vec![h.start_crossing(2)]);
    let prompt =
        h.listener.event_state.entries[&h.participant].areas[&h.content.record_id()].area[1];
    h.run(1200, vec![logic(START, prompt)]);
    catalog::race_from_intro(&mut h, 1200);
    let game_mode = h.race().game_mode;
    let restart = h.race().logics[&Logic::Restart];
    h.listener.persistent = Some(saved(true, true));
    h.run(3000, vec![]);
    assert_eq!(h.race().game_mode, game_mode);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .areas
            .is_empty()
    );
    h.run(3100, vec![logic(RESTART, restart)]);
    assert_eq!(h.race().exit, Some(Exit::Abandon));
    h.run(3200, vec![]);
    h.run(3300, vec![h.leave()]);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
    assert!(h.world().get(game_mode).is_none());
    assert!(h.listener.scoring_state.pending_awards.is_empty());
}
