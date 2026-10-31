// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::messages::unlocks::{Delivery, Notification, Trigger};
use nfs_services::persistent::key;
use nfs_storage::{InventoryRepository, Timestamp};
use nfs_world::participants::HostRpc;
fn delivery() -> Delivery {
    Delivery {
        notifications: vec![
            Notification {
                hash: 17,
                manager: 4,
                delay: std::time::Duration::from_millis(2500),
                trigger: Trigger::Part {
                    include: false,
                    ids: BTreeSet::new(),
                },
            },
            Notification {
                hash: 18,
                manager: 4,
                delay: std::time::Duration::from_secs(5),
                trigger: Trigger::Categories { mask: 4 },
            },
        ],
        controller: [1; 16],
        parts: vec![],
        dynamic_manager: 4,
        dynamic_capacity: 7,
    }
}
#[test]
fn committed_signal_replaces_delay_but_property_change_does_not_and_ready_waits() {
    let content = delivery();
    let mut state = State::default();
    let first = Pending {
        hash: 17,
        signal: 5,
        part: Some(100),
    };
    state.advance(&content, &[first], 1000).unwrap();
    state
        .advance(
            &content,
            &[Pending {
                part: Some(200),
                ..first
            }],
            2000,
        )
        .unwrap();
    assert!(state.ready.is_empty());
    state.advance(&content, &[first], 3500).unwrap();
    assert_eq!(state.ready, [(17, 5)]);
    state.advance(&content, &[first], 20_000).unwrap();
    assert_eq!(
        state.ready,
        [(17, 5)],
        "downstream pressure does not drop or repeat a ready intent"
    );
    let replacement = Pending { signal: 7, ..first };
    state.advance(&content, &[replacement], 20_001).unwrap();
    assert!(state.ready.is_empty());
    state.advance(&content, &[replacement], 22_501).unwrap();
    assert_eq!(state.ready, [(17, 7)]);
    state.advance(&content, &[], 22_502).unwrap();
    assert!(state.ready.is_empty());
    assert!(state.advance(&content, &[], 22_501).is_err());
}

#[test]
fn independent_senders_keep_order_and_fresh_session_restarts_pending_delay() {
    let content = delivery();
    let rows = [
        Pending {
            hash: 17,
            signal: 5,
            part: Some(100),
        },
        Pending {
            hash: 18,
            signal: 5,
            part: None,
        },
    ];
    let mut old = State::default();
    old.advance(&content, &rows, 10_000).unwrap();
    old.advance(&content, &rows, 12_000).unwrap();
    let mut restarted = State::default();
    restarted.advance(&content, &rows, 50).unwrap();
    restarted.advance(&content, &rows, 2550).unwrap();
    assert_eq!(restarted.ready, [(17, 5)]);
    restarted.advance(&content, &rows, 5050).unwrap();
    assert_eq!(restarted.ready, [(17, 5), (18, 5)]);
    assert!(
        old.ready.is_empty(),
        "session/account timer instances are isolated"
    );
}

struct Failable {
    inner: nfs_storage::MemoryRepository,
    fail: std::sync::atomic::AtomicBool,
}
impl InventoryRepository for Failable {
    fn open(&self, a: nfs_storage::AccountId) -> Result<nfs_storage::Snapshot, nfs_storage::Error> {
        self.inner.open(a)
    }
    fn snapshot(
        &self,
        a: nfs_storage::AccountId,
    ) -> Result<nfs_storage::Snapshot, nfs_storage::Error> {
        self.inner.snapshot(a)
    }
    fn receipt(
        &self,
        a: nfs_storage::AccountId,
        id: u64,
    ) -> Result<Option<nfs_storage::Applied>, nfs_storage::Error> {
        self.inner.receipt(a, id)
    }
    fn apply(
        &self,
        a: nfs_storage::AccountId,
        b: &nfs_storage::Batch,
        now: Timestamp,
    ) -> Result<nfs_storage::Applied, nfs_storage::Error> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            Err(nfs_storage::Error::Storage)
        } else {
            self.inner.apply(a, b, now)
        }
    }
}

fn intent(
    repo: &dyn InventoryRepository,
    account: nfs_storage::AccountId,
    part: u32,
    previous_signal: Option<u64>,
) -> u64 {
    use nfs_storage::tables::{Table, Value};
    let snapshot = repo.snapshot(account).unwrap();
    let signal = previous_signal.unwrap_or(snapshot.generation + 1);
    let table = Table {
        rows: [(
            17,
            [
                (key("Version"), Value::Int(1)),
                (key("Signal"), Value::String(signal.to_string())),
                (key("Kind"), Value::Int(1)),
                (key("Part"), Value::Int(part as i32)),
            ]
            .into(),
        )]
        .into(),
    };
    repo.apply(
        account,
        &nfs_storage::Batch {
            id: 10_000 + snapshot.generation,
            expected_generation: snapshot.generation,
            ops: vec![nfs_storage::Op::SetTable(
                key(nfs_services::messages::unlocks::TABLE),
                table,
            )],
        },
        Timestamp(10),
    )
    .unwrap();
    signal
}

#[tokio::test]
async fn worker_delivery_waits_for_commit_and_capacity_then_restores_and_reads_the_first_delivery()
{
    use crate::net::players::prologue::tests::setup;
    use nfs_content::messages::{Catalog, Managers, Message};
    use nfs_world::game_mode::Arguments;
    use std::sync::{Arc, atomic::Ordering};
    let (mut listener, participant, _) = setup();
    let (repository, account, _) = crate::progression::tests::local();
    let repository = Arc::new(Failable {
        inner: repository,
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    let mut plan = delivery();
    plan.parts = vec![
        nfs_content::messages::unlocks::Part {
            definition: [2; 16],
            id: 100,
        },
        nfs_content::messages::unlocks::Part {
            definition: [3; 16],
            id: 200,
        },
    ];
    let messages = crate::messages::Content {
        catalog: Catalog::new(
            3,
            [17, 18]
                .into_iter()
                .enumerate()
                .map(|(i, hash)| {
                    (
                        (format!("constructed/{hash}"), 0),
                        Message {
                            name: format!("constructed/{hash}"),
                            hash,
                            flat: 10 + i as u16,
                            persisted: true,
                        },
                    )
                })
                .collect(),
        )
        .unwrap(),
        managers: Managers {
            scene_key: 909,
            normal: 3,
            underground: 4,
        },
        licensed: vec![],
        active: vec![],
        power: vec![],
        result: vec![],
        unlocks: None,
    }
    .with_unlocks(plan)
    .unwrap();
    let progression = Arc::new(
        crate::progression::Content::from_json(&crate::progression::tests::combined())
            .unwrap()
            .with_messages(messages.clone()),
    );
    let mut definition = crate::progression::tests::table_definition();
    for table in definition["tables"].as_array_mut().unwrap() {
        let columns = match table["name"].as_str().unwrap() {
            "DynamicMessage" => vec!["Asset", "Read", "Data"],
            "StaticMessage" => vec!["Asset", "Read"],
            "UgStaticMessages" => vec!["MessageHash", "Read"],
            _ => continue,
        };
        table["columns"] = serde_json::json!(columns.into_iter().map(|name| serde_json::json!({"name":name,"key":key(name),"value_type":if name == "Data" {"string"} else {"int"},"default":if name == "Data" {""} else {"0"}})).collect::<Vec<_>>());
    }
    let tables = Arc::new(crate::persistent::Catalog::from_json(&definition).unwrap());
    let profile = crate::net::items::Service::new(
        repository.clone(),
        Arc::new(nfs_world::items::InventoryCatalog::new([], vec![]).unwrap()),
    )
    .with_persistent(tables.clone())
    .bind(account);
    let signal = intent(repository.as_ref(), account, 100, None);
    listener.progression = Some(progression.clone());
    listener.persistent = Some(Arc::new(
        tables.view(&repository.snapshot(account).unwrap()).unwrap(),
    ));
    let restored = progression
        .catalog
        .restore(listener.persistent.as_ref().unwrap(), progression.settings)
        .unwrap();
    let scene = listener.players.objects().scene(909).unwrap();
    for branch in [
        crate::progression::Branch::Intro,
        crate::progression::Branch::Main,
    ] {
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
    let dummy = Request {
        participant,
        player_index: 0,
        progression: progression.clone(),
        action: Action::UnlockMessage { hash: 17, signal },
    };
    listener
        .advance_unlock_notifications(listener.world_ms)
        .unwrap();
    listener.story_pending = VecDeque::from(vec![dummy; MAX_PENDING]);
    listener.world_ms += 2500;
    listener
        .advance_unlock_notifications(listener.world_ms)
        .unwrap();
    assert_eq!(listener.story_pending.len(), MAX_PENDING);
    assert!(listener.unlock_notifications.queued.is_empty());
    listener.story_pending.clear();
    listener
        .advance_unlock_notifications(listener.world_ms)
        .unwrap();
    let request = listener.take_story().unwrap();
    assert_eq!(request.action, Action::UnlockMessage { hash: 17, signal });
    let before = repository.snapshot(account).unwrap();
    repository.fail.store(true, Ordering::SeqCst);
    assert!(
        profile
            .story_for_test(&request, Timestamp(11))
            .await
            .is_err()
    );
    assert_eq!(repository.snapshot(account).unwrap(), before);
    listener.story_failed(&request);
    repository.fail.store(false, Ordering::SeqCst);
    listener
        .advance_unlock_notifications(listener.world_ms)
        .unwrap();
    assert_eq!(listener.take_story().unwrap().action, request.action);
    // The attachment is sampled from the worker's current account snapshot.
    intent(repository.as_ref(), account, 200, Some(signal));
    let committed = profile
        .story_for_test(&request, Timestamp(12))
        .await
        .unwrap();
    let old_generation = listener.persistent.as_ref().unwrap().generation();
    let filler = messages
        .added(
            listener.players.objects(),
            crate::messages::Manager::Underground,
            18,
        )
        .unwrap();
    listener.pending_rpcs = VecDeque::from(vec![filler; nfs_world::session::MAX_RPC_QUEUE]);
    assert_eq!(
        listener.story_committed(&request, committed.clone()),
        Err(replication::Error::Bound)
    );
    assert_eq!(
        listener.persistent.as_ref().unwrap().generation(),
        old_generation
    );
    assert!(
        listener
            .unlock_notifications
            .first
            .as_ref()
            .unwrap()
            .is_empty()
    );
    listener.pending_rpcs.clear();
    listener
        .story_committed(&request, committed.clone())
        .unwrap();
    let parts = |listener: &PlayerListener| {
        listener
            .pending_rpcs
            .iter()
            .filter_map(|r| match r {
                HostRpc::GameMode(c) if c.method == 2 => match c.arguments {
                    Arguments::CarPartMessage { part, .. } => Some(part.item_data_id),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(parts(&listener), [200]);
    listener.story_committed(&request, committed).unwrap();
    let retry = profile
        .story_for_test(&request, Timestamp(13))
        .await
        .unwrap();
    listener.story_committed(&request, retry).unwrap();
    assert_eq!(parts(&listener), [200]);
    let first = listener
        .unlock_notifications
        .first(17, &messages, listener.persistent.as_ref().unwrap())
        .unwrap()
        .unwrap();
    let mut read = request.clone();
    read.action = Action::DynamicMessageRead {
        identity: first.clone(),
        read: true,
    };
    let committed = profile.story_for_test(&read, Timestamp(14)).await.unwrap();
    listener.story_committed(&read, committed).unwrap();
    let second_signal = intent(repository.as_ref(), account, 100, None);
    let mut second = request.clone();
    second.action = Action::UnlockMessage {
        hash: 17,
        signal: second_signal,
    };
    let committed = profile
        .story_for_test(&second, Timestamp(15))
        .await
        .unwrap();
    listener.story_committed(&second, committed).unwrap();
    assert_eq!(parts(&listener), [200, 100]);
    assert_eq!(
        listener
            .unlock_notifications
            .first(17, &messages, listener.persistent.as_ref().unwrap())
            .unwrap()
            .unwrap(),
        first
    );
    read.action = Action::DynamicMessageRead {
        identity: first,
        read: false,
    };
    let committed = profile.story_for_test(&read, Timestamp(16)).await.unwrap();
    listener.story_committed(&read, committed).unwrap();
    let saved = repository.snapshot(account).unwrap();
    let loaded = tables.view(&saved).unwrap();
    let dynamic = messages.dynamic_rows(&loaded).unwrap();
    assert_eq!(
        dynamic.iter().map(|r| r.read).collect::<Vec<_>>(),
        [false, false]
    );
    let replay = messages
        .session_start(listener.players.objects(), &[], &[], &dynamic)
        .unwrap();
    assert!(matches!(replay.last(), Some(HostRpc::GameMode(c)) if c.method == 1));
    let restored: Vec<_> = replay
        .iter()
        .filter_map(|r| match r {
            HostRpc::GameMode(c) => match c.arguments {
                Arguments::CarPartMessage { part, .. } => {
                    assert_eq!(c.method, 0);
                    Some(part.item_data_id)
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(restored, [200, 100]);
    let (mut restarted, _, _) = setup();
    restarted.progression = Some(progression);
    restarted.persistent = Some(Arc::new(loaded));
    restarted
        .advance_unlock_notifications(restarted.world_ms)
        .unwrap();
    restarted
        .advance_unlock_notifications(restarted.world_ms + 60_000)
        .unwrap();
    assert!(restarted.take_story().is_none());
    assert_eq!(repository.snapshot(account).unwrap(), saved);
}
