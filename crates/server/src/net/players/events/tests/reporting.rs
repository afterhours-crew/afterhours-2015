// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::net::items::story::Action;
use nfs_storage::{InventoryRepository, Timestamp};

fn story_finish() -> (
    Harness,
    nfs_storage::MemoryRepository,
    nfs_storage::AccountId,
) {
    let mut h = started();
    let mut event = h.content.event().clone();
    event.categories = BTreeSet::from([91]);
    h.content = Arc::new(event_logic::Content::new(event, h.content.category().into()).unwrap());
    h.listener.events = Some(Arc::new(h.content.as_ref().clone().into()));
    let progression = Arc::new(
        crate::progression::Content::from_json(&crate::progression::tests::combined())
            .unwrap()
            .with_objective_results(vec![nfs_content::progression::results::Completion {
                objective: 30,
                categories: BTreeSet::from([91]),
                enabled: nfs_content::progression::conditions::Condition::all(&[
                    nfs_content::progression::conditions::Fact::Active(30),
                ])
                .unwrap(),
            }])
            .unwrap(),
    );
    let (repository, account, _) = crate::progression::tests::local();
    let policy = progression.catalog.story_policy(progression.settings);
    for objective in [10, 20] {
        let before = repository.snapshot(account).unwrap();
        let nfs_services::story::Prepared::Mutation(batch) = policy
            .complete(
                &before,
                before.generation,
                policy.next_operation(&before).unwrap(),
                objective,
                vec![],
            )
            .unwrap()
        else {
            panic!("new completion");
        };
        repository.apply(account, &batch, Timestamp(3)).unwrap();
    }
    h.listener.persistent = Some(Arc::new(
        crate::progression::tests::tables()
            .view(&repository.snapshot(account).unwrap())
            .unwrap(),
    ));
    h.listener.progression = Some(progression);
    h.run(6000, vec![h.checkpoint(6, 30.)]);
    h.run(7000, vec![h.checkpoint(7, 60.)]);
    (h, repository, account)
}

#[test]
fn story_reporting_waits_for_outro_and_paid_finish_then_blocks_departure_until_commit() {
    let (mut h, repository, account) = story_finish();
    let award = h
        .listener
        .scoring_state
        .pending_awards
        .back()
        .unwrap()
        .clone();
    assert_eq!(award.story.as_ref().unwrap().objective, 30);
    assert!(h.listener.story_pending.is_empty());
    assert!(!h.race().reporting_started);
    let outro = h.race().sequence.unwrap().1;
    h.run(50000, vec![]);
    assert!(
        h.listener.story_pending.is_empty(),
        "elapsed time cannot invent outro completion"
    );
    h.run(50100, vec![logic(OUTRO_READY, outro)]);
    h.run(50200, vec![h.completed(outro)]);
    h.run(50400, vec![]);
    assert!(
        !h.race().reporting_started,
        "unpaid result cannot start the story handoff"
    );
    h.listener
        .event_credit_committed(h.participant, award.batch_id);
    let reporting = h.run(50600, vec![]);
    let gm = h.race().game_mode;
    assert!(h.race().reporting_started);
    assert!(reporting.calls().contains(&(
        gm,
        h.content.state(Gm::ResultReporting) as u16,
        0,
        Some(h.participant)
    )));
    assert!(!h.race().awaiting_leave);
    let request = h.listener.story_pending.pop_front().unwrap();
    assert_eq!(request.action, Action::Complete { objective: 30 });
    for now in [50800, 51000, 52000] {
        h.run(now, vec![h.leave()]);
        assert!(!h.race().awaiting_leave);
        assert!(
            h.listener.story_pending.is_empty(),
            "result request is emitted once"
        );
    }
    let loaded = h.listener.persistent.as_ref().unwrap();
    assert!(h.listener.event_story_reported(&request, loaded).is_err());
    let policy = request
        .progression
        .catalog
        .story_policy(request.progression.settings);
    let before = repository.snapshot(account).unwrap();
    let nfs_services::story::Prepared::Mutation(batch) = policy
        .complete(
            &before,
            before.generation,
            policy.next_operation(&before).unwrap(),
            30,
            vec![],
        )
        .unwrap()
    else {
        panic!("new report");
    };
    repository.apply(account, &batch, Timestamp(4)).unwrap();
    let completed = crate::progression::tests::tables()
        .view(&repository.snapshot(account).unwrap())
        .unwrap();
    assert!(
        h.listener
            .event_story_reported(&request, &completed)
            .unwrap()
    );
    h.listener.release_event_story(h.participant);
    let ending = h.run(52200, vec![]);
    assert!(ending.calls().contains(&(gm, 0, 1, None)));
    assert!(h.race().awaiting_leave);
    h.run(52400, vec![h.leave()]);
    assert!(
        h.listener.event_state.entries[&h.participant]
            .race
            .is_none()
    );
    assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
    assert!(h.listener.story_pending.is_empty());
}

#[test]
fn full_story_queue_rolls_back_reporting_entry_and_retries_without_duplicate_credit() {
    let (mut h, _, _) = story_finish();
    let award = h
        .listener
        .scoring_state
        .pending_awards
        .back()
        .unwrap()
        .clone();
    h.listener
        .event_credit_committed(h.participant, award.batch_id);
    let outro = h.race().sequence.unwrap().1;
    h.run(8000, vec![logic(OUTRO_READY, outro)]);
    h.run(8100, vec![h.completed(outro)]);
    let filler = crate::net::items::story::Request {
        participant: h.participant,
        player_index: 0,
        progression: award.story.as_ref().unwrap().progression.clone(),
        action: Action::Complete { objective: 30 },
    };
    h.listener.story_pending = std::iter::repeat_n(filler, MAX_PENDING).collect();
    let blocked = h.run(8300, vec![]);
    assert!(blocked.calls().is_empty());
    assert!(!h.race().reporting_started);
    assert!(!h.race().awaiting_leave);
    h.listener.story_pending.clear();
    h.run(8500, vec![]);
    assert!(h.race().reporting_started);
    assert_eq!(h.listener.story_pending.len(), 1);
    assert_eq!(h.listener.scoring_state.pending_awards.len(), 1);
}
