// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

fn add_second(h: &mut Harness) -> event_logic::Content {
    let mut event = fixture::event();
    event.game_mode_id = "constructed/sprint_second".into();
    event.start_volume = 12;
    event.game_mode.checkpoints = vec![(fixture::RACE, 8), (fixture::RACE, 9)];
    event.placement_rewards = None;
    event.rewards.cash = 321;
    event.rewards.rep = 654;
    let second = event_logic::Content::new(event, "second_category".into()).unwrap();
    let roster: Vec<_> = [h.content.as_ref(), &second]
        .into_iter()
        .map(|c| nfs_content::events::discovery::Entry {
            id: c.event().game_mode_id.clone(),
            scene: c.event().scene.clone(),
            game_mode: c.event().game_mode.layout.blueprint.clone(),
            volume: nfs_content::events::discovery::Endpoint {
                scene_key: c.event().scene_content_key,
                serializer: c.event().start_volume,
            },
            notifications: vec![],
        })
        .collect();
    h.listener.events = Some(Arc::new(
        Catalog::new(vec![h.content.as_ref().clone(), second.clone()], &roster).unwrap(),
    ));
    let mut state = h.listener.event_state.clone();
    h.listener
        .event_join(
            &second,
            &h.listener.players,
            &mut state,
            h.participant,
            &mut Out::default(),
        )
        .unwrap();
    h.listener.event_state = state;
    second
}
fn crossing(h: &Harness, slot: usize, inside: bool) -> Input {
    let t = 12.5f64.to_bits();
    rpc(
        &[h.scene(), h.participant],
        h.selector(slot),
        if inside { 2 } else { 3 },
        &[(t >> 32, 32), (t & 0xffff_ffff, 32), (0, 32), (0, 7)],
    )
}
fn race_from_intro(h: &mut Harness, t: u64) {
    h.run(t + 100, vec![]);
    let intro = h.race().sequence.unwrap().1;
    h.run(t + 1000, vec![logic(INTRO_READY, intro)]);
    h.run(t + 1100, vec![h.completed(intro)]);
    h.run(t + 1200, vec![]);
    let countdown = h.race().sequence.unwrap().1;
    h.run(t + 1300, vec![h.completed(countdown)]);
    h.run(t + 1400, vec![]);
    assert_eq!(h.race().phase, Phase::Racing);
}

#[test]
fn each_start_area_receives_its_eligibility_once_per_participant() {
    let h = Harness::new();
    let mut event = fixture::event();
    event.game_mode_id = "constructed/second_eligibility".into();
    event.start_volume = 12;
    event.area_notifications = vec![6, 7];
    let second = event_logic::Content::new(event, "second_category".into()).unwrap();
    let mut state = h.listener.event_state.clone();
    let mut out = Out::default();
    h.listener
        .event_join(
            &second,
            &h.listener.players,
            &mut state,
            h.participant,
            &mut out,
        )
        .unwrap();
    let notifications = |out: &Out| {
        out.rpcs
            .iter()
            .filter_map(|rpc| match rpc {
                HostRpc::GameMode(call)
                    if [h.selector(6), h.selector(7)].contains(&call.selector) =>
                {
                    Some((call.target, call.selector, call.method, call.reference))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        notifications(&out),
        vec![
            (h.scene(), h.selector(6), 0, None),
            (h.scene(), h.selector(7), 0, None)
        ]
    );
    let mut repeated = Out::default();
    h.listener
        .event_join(
            &second,
            &h.listener.players,
            &mut state,
            h.participant,
            &mut repeated,
        )
        .unwrap();
    assert!(notifications(&repeated).is_empty());
    assert_eq!(state.eligible.len(), 2);
    assert_eq!(
        h.listener.event_state.eligible.len(),
        1,
        "uncommitted state must stay isolated"
    );
    let mut other = State::default();
    let mut other_out = Out::default();
    h.listener
        .event_join(
            &second,
            &h.listener.players,
            &mut other,
            h.participant,
            &mut other_out,
        )
        .unwrap();
    assert_eq!(notifications(&other_out), notifications(&out));
}

#[test]
fn overlapping_starts_choose_the_prompt_identity_and_discard_stale_prompt_inputs() {
    let mut h = Harness::new();
    let second = add_second(&mut h);
    h.run(1000, vec![h.start_crossing(2), crossing(&h, 12, true)]);
    let entries = &h.listener.event_state.entries[&h.participant].areas;
    let first_prompt = entries[&h.content.record_id()].area[1];
    let second_prompt = entries[&second.record_id()].area[1];
    h.run(
        2000,
        vec![logic(START, second_prompt), logic(START, first_prompt)],
    );
    assert_eq!(h.race().event_id, second.record_id());
    assert!(
        h.listener.event_state.entries[&h.participant]
            .areas
            .values()
            .all(|a| a.area.is_empty())
    );
    assert!(h.world().get(first_prompt).is_none());
    race_from_intro(&mut h, 2000);
    h.run(5000, vec![h.checkpoint(6, 30.), h.checkpoint(7, 40.)]);
    assert!(
        h.race().finish.is_none(),
        "another event's checkpoints cannot advance this race"
    );
    h.run(6000, vec![h.checkpoint(8, 50.), h.checkpoint(9, 60.)]);
    assert_eq!(h.race().finish.unwrap().position, 1);
    let credit = h.listener.scoring_state.pending_awards.back().unwrap();
    let completion = credit.event.as_ref().unwrap();
    assert_eq!(
        (
            completion.event_id,
            completion.cash,
            completion.rep.total,
            completion.kind.as_str()
        ),
        (second.record_id(), 321, 654, "second_category")
    );
    h.run(6100, vec![h.checkpoint(9, 61.), logic(START, first_prompt)]);
    assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
}

#[test]
fn finish_inside_another_start_area_restores_its_prompt_then_switches_identity() {
    let mut h = started();
    let first_id = h.race().event_id;
    let second = add_second(&mut h);
    h.run(5000, vec![crossing(&h, 12, true)]);
    assert!(
        h.listener.event_state.entries[&h.participant].areas[&second.record_id()]
            .area
            .is_empty()
    );
    h.run(6000, vec![h.checkpoint(6, 30.)]);
    h.run(7000, vec![h.checkpoint(7, 60.)]);
    let outro = h.race().sequence.unwrap().1;
    h.run(8000, vec![logic(OUTRO_READY, outro)]);
    h.run(8100, vec![h.completed(outro)]);
    h.run(8300, vec![]);
    let old_leave = h.leave();
    h.run(8400, vec![old_leave.clone()]);
    let prompt = h.listener.event_state.entries[&h.participant].areas[&second.record_id()].area[1];
    h.run(8500, vec![logic(START, prompt), old_leave]);
    assert_eq!(h.race().event_id, second.record_id());
    race_from_intro(&mut h, 8500);
    h.run(11000, vec![h.checkpoint(8, 70.), h.checkpoint(9, 80.)]);
    let awards = &h.listener.scoring_state.pending_awards;
    assert_eq!(awards.len(), 2);
    assert_eq!(awards[0].event.as_ref().unwrap().event_id, first_id);
    assert_eq!(
        awards[1].event.as_ref().unwrap().event_id,
        second.record_id()
    );
    assert_ne!(awards[0].batch_id, awards[1].batch_id);
}

#[test]
fn selection_and_both_prompts_roll_back_together_under_queue_pressure() {
    let mut h = Harness::new();
    let second = add_second(&mut h);
    h.run(1000, vec![h.start_crossing(2), crossing(&h, 12, true)]);
    let prompt = h.listener.event_state.entries[&h.participant].areas[&second.record_id()].area[1];
    let call = HostRpc::GameMode(h.content.flag_call(h.world(), 0, 0).unwrap());
    h.listener.pending_rpcs =
        std::iter::repeat_n(call, nfs_world::session::MAX_RPC_QUEUE).collect();
    h.listener
        .event_state
        .inputs
        .push_back(logic(START, prompt));
    h.listener.advance_events(2000).unwrap();
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
    assert!(h.world().get(prompt).is_some());
    assert_eq!(h.listener.event_state.inputs.len(), 1);
    h.listener.pending_rpcs.clear();
    h.run(2000, vec![]);
    assert_eq!(h.race().event_id, second.record_id());
    assert!(h.listener.event_state.inputs.is_empty());
    assert!(
        Harness::new().listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
}
