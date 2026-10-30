// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    messages::Manager,
    net::items::{
        Service,
        story::{Action, Request},
    },
    progression::Branch,
};
use nfs_content::{
    messages::{Catalog, Managers, Message},
    progression::{
        conditions::{Condition, Fact},
        results::Completion,
    },
};
use nfs_services::{
    persistent::key,
    reputation::{Scores, Thresholds},
};
use nfs_storage::{AccountId, InventoryRepository, MemoryRepository, Timestamp};
use nfs_world::participants::HostRpc;
use std::sync::Arc;

fn rule() -> Completion {
    Completion {
        objective: 30,
        categories: BTreeSet::from([91]),
        enabled: Condition::all(&[
            Fact::Active(30),
            Fact::MessageRead {
                hash: 71,
                manager: 3,
            },
        ])
        .unwrap(),
    }
}
fn content() -> crate::progression::Content {
    crate::progression::Content::from_json(&crate::progression::tests::combined())
        .unwrap()
        .with_messages(crate::messages::Content {
            catalog: Catalog::new(
                3,
                vec![(
                    ("constructed/result".into(), 0),
                    Message {
                        name: "constructed/result".into(),
                        hash: 71,
                        flat: 10,
                        persisted: true,
                    },
                )],
            )
            .unwrap(),
            managers: Managers {
                scene_key: 909,
                normal: 3,
                underground: 4,
            },
            licensed: vec![],
            active: vec![],
        })
        .with_objective_results(vec![rule()])
        .unwrap()
}
fn tables() -> Arc<crate::persistent::Catalog> {
    let mut definition = crate::progression::tests::table_definition();
    for (name, columns) in [
        (
            "RepValuesTable",
            vec![
                "RepScore",
                "RepLevel",
                "BuildScore",
                "CrewScore",
                "OutlawScore",
                "SpeedScore",
                "StyleScore",
            ],
        ),
        (
            "GameplayStatsTable",
            vec![
                "LargestFineEscaped",
                "LargestFineBusted",
                "CashEarned",
                "DistanceDriven",
                "DistanceDrifted",
                "FavoriteVehicleId",
                "TotalTimePlayed",
                "TopSpeed",
                "Spare",
            ],
        ),
        ("StaticMessage", vec!["Asset", "Read"]),
        ("UgStaticMessages", vec!["MessageHash", "Read"]),
    ] {
        let table = definition["tables"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t["name"] == name)
            .unwrap();
        table["columns"] = serde_json::json!(
            columns
                .iter()
                .map(
                    |c| serde_json::json!({"name":c,"key":key(c),"value_type":"int","default":"0"})
                )
                .collect::<Vec<_>>()
        );
    }
    Arc::new(crate::persistent::Catalog::from_json(&definition).unwrap())
}

#[tokio::test]
async fn authored_result_waits_for_its_message_and_commits_before_retryable_publication() {
    let (mut listener, participant, _) = prologue::tests::setup();
    let progression = Arc::new(content());
    let repository = Arc::new(MemoryRepository::new());
    let account = AccountId::from_owned_config([77; 16]).unwrap();
    let inventory = Arc::new(nfs_world::items::InventoryCatalog::new([], vec![]).unwrap());
    let tables = tables();
    let initial =
        nfs_services::inventory::load(repository.as_ref(), account, &inventory, 1, Timestamp(1))
            .unwrap();
    let initial = tables
        .ensure_loaded(repository.as_ref(), account, initial, Timestamp(2))
        .unwrap()
        .0;
    let profile = Service::new(repository.clone(), inventory)
        .with_persistent(tables.clone())
        .bind(account);
    listener.progression = Some(progression.clone());
    listener.persistent = Some(Arc::new(tables.view(&initial).unwrap()));
    let categories = BTreeSet::from([91, 92]);
    assert!(
        progression
            .result_objective(&categories, listener.persistent.as_ref().unwrap())
            .is_err()
    );
    assert_eq!(
        progression
            .result_objective(&BTreeSet::from([92]), listener.persistent.as_ref().unwrap())
            .unwrap(),
        None
    );
    let mut story_request = Request {
        participant,
        player_index: 0,
        progression: progression.clone(),
        action: Action::Complete { objective: 10 },
    };
    for objective in [10, 20] {
        story_request.action = Action::Complete { objective };
        listener.persistent = Some(
            profile
                .story_for_test(&story_request, Timestamp(3))
                .await
                .unwrap()
                .loaded,
        );
    }
    assert!(
        progression
            .result_objective(&categories, listener.persistent.as_ref().unwrap())
            .is_err()
    );
    // A read invitation in the other manager does not open this reporter.
    for manager in [Manager::Underground, Manager::Normal] {
        story_request.action = Action::Message { hash: 71, manager };
        listener.persistent = Some(
            profile
                .story_for_test(&story_request, Timestamp(4))
                .await
                .unwrap()
                .loaded,
        );
        assert!(
            progression
                .result_objective(&categories, listener.persistent.as_ref().unwrap())
                .is_err()
        );
        story_request.action = Action::MessageRead {
            hash: 71,
            manager,
            read: true,
        };
        listener.persistent = Some(
            profile
                .story_for_test(&story_request, Timestamp(5))
                .await
                .unwrap()
                .loaded,
        );
        if manager == Manager::Underground {
            assert!(
                progression
                    .result_objective(&categories, listener.persistent.as_ref().unwrap())
                    .is_err()
            );
        }
    }
    assert_eq!(
        progression
            .result_objective(&categories, listener.persistent.as_ref().unwrap())
            .unwrap(),
        Some(30)
    );
    let restored = progression
        .catalog
        .restore(listener.persistent.as_ref().unwrap(), progression.settings)
        .unwrap();
    let scene = listener.players.objects().scene(909).unwrap();
    for branch in [Branch::Intro, Branch::Main] {
        let creation = progression
            .construction
            .build(
                branch,
                restored.branch(branch),
                participant,
                scene,
                |_, _| {
                    Some(crate::progression::entities::SceneEndpoint {
                        ghost: scene,
                        selector: 1,
                    })
                },
            )
            .unwrap();
        listener
            .players
            .spawn_entities(vec![creation], progression.construction.content())
            .unwrap();
    }
    let scores = Scores {
        total: 9000,
        build: 0,
        crew: 0,
        outlaw: 0,
        speed: 0,
        style: 0,
    };
    let mut event = crate::event_logic::tests::event();
    event.categories = BTreeSet::from([91, 92]);
    event.rewards = nfs_content::events::Rewards {
        cash: 2000,
        rep: 9000,
    };
    event.placement_rewards = Some([event.rewards; 4]);
    let event = crate::event_logic::Content::new(event, "SpeedWall_SprintRace".into()).unwrap();
    let old_logic = listener.logic.replace(Arc::new(
        crate::garage_logic::GarageLogic::from_json_with_reputation(
            &crate::garage_logic::tests::presentation_json(),
            Some(Thresholds::new(vec![0, 5000, 25000]).unwrap()),
        )
        .unwrap(),
    ));
    let finish = crate::event_logic::Finish {
        race_ms: 123_000,
        position: 3,
        participants: 4,
    };
    assert!(
        listener
            .event_completion(&event, participant + 1, 911, finish, 0, None)
            .is_err()
    );
    let award = listener
        .event_completion(&event, participant, 911, finish, 0, None)
        .unwrap()
        .unwrap();
    listener.logic = old_logic;
    assert_eq!(award.story.as_ref().unwrap().objective, 30);
    assert_eq!(award.event.as_ref().unwrap().position, 2);
    assert_eq!(award.credit, scores);
    let before = listener.persistent.clone().unwrap();
    let calls = listener.pending_rpcs.len();
    let credited = profile.credit_for_test(&award, Timestamp(6)).await.unwrap();
    assert_eq!(listener.pending_rpcs.len(), calls);
    assert_eq!(
        listener.persistent.as_ref().unwrap().generation(),
        before.generation()
    );
    let saved = repository.snapshot(account).unwrap();
    assert!(
        progression
            .catalog
            .story_policy(progression.settings)
            .objective(&saved, 30)
            .unwrap()
            .completed
    );
    assert_eq!(
        nfs_services::bank::Balance::from_snapshot(&saved)
            .unwrap()
            .0,
        2000
    );
    assert_eq!(Scores::load(&credited.loaded).unwrap(), scores);
    let filler = HostRpc::Query(nfs_world::participants::EntityCall {
        entity: scene,
        selector: 1,
        serial: nfs_protocol::world::rpc::Serial::new(1).unwrap(),
        method: 0,
    });
    listener
        .pending_rpcs
        .resize(nfs_world::session::MAX_RPC_QUEUE, filler);
    assert_eq!(
        listener.awarded(&award, credited),
        Err(replication::Error::Bound)
    );
    assert_eq!(
        listener.persistent.as_ref().unwrap().generation(),
        before.generation()
    );
    listener.pending_rpcs.clear();
    let retry = profile.credit_for_test(&award, Timestamp(7)).await.unwrap();
    assert!(retry.replayed);
    listener.awarded(&award, retry).unwrap();
    assert_eq!(repository.snapshot(account).unwrap(), saved);
    assert_eq!(
        listener
            .pending_rpcs
            .iter()
            .filter(|rpc| matches!(rpc, HostRpc::Query(c) if c.method == 2))
            .count(),
        1
    );
    let published = listener.pending_rpcs.len();
    listener
        .awarded(
            &award,
            profile.credit_for_test(&award, Timestamp(8)).await.unwrap(),
        )
        .unwrap();
    assert_eq!(listener.pending_rpcs.len(), published);
    assert!(
        progression
            .result_objective(&categories, listener.persistent.as_ref().unwrap())
            .is_err()
    );
    let mut wrong = award.clone();
    wrong.event.as_mut().unwrap().batch_id += 1;
    assert!(profile.credit_for_test(&wrong, Timestamp(9)).await.is_err());
    assert_eq!(repository.snapshot(account).unwrap(), saved);
}

#[test]
fn objective_result_catalog_rejects_ambiguous_and_foreign_definitions() {
    assert!(
        content()
            .with_objective_results(vec![rule(), rule()])
            .is_err()
    );
    let mut foreign = rule();
    foreign.objective = 20;
    assert!(content().with_objective_results(vec![foreign]).is_err());
    let mut missing = rule();
    missing.enabled = Condition::all(&[Fact::Active(999)]).unwrap();
    assert!(content().with_objective_results(vec![missing]).is_err());
    let mut empty = rule();
    empty.categories.clear();
    assert!(content().with_objective_results(vec![empty]).is_err());
}
