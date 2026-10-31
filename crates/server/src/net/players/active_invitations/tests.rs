// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::net::players::prologue::tests::setup;
use nfs_content::messages::{Catalog, Managers, Message, active::Invitation};
use nfs_storage::{InventoryRepository, Timestamp};
use nfs_world::participants::HostRpc;
use std::sync::Arc;

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

const HASH: u32 = 71;
fn messages() -> crate::messages::Content {
    crate::messages::Content {
        catalog: Catalog::new(
            3,
            vec![(
                ("constructed/invitation".into(), 0),
                Message {
                    name: "constructed/invitation".into(),
                    hash: HASH,
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
        power: vec![],
        result: vec![],
    }
    .with_active(vec![Invitation {
        objective: 30,
        hash: HASH,
        manager: 3,
    }])
    .unwrap()
}

#[tokio::test]
async fn invitation_waits_for_committed_activation_and_survives_backpressure_and_restart() {
    exercise_messages(false, false).await;
}

#[tokio::test]
async fn power_invitation_waits_for_authored_delay_and_excludes_competing_tiers_after_restart() {
    exercise_messages(true, false).await;
}

#[tokio::test]
async fn result_invitation_default_is_committed_once_and_restored_despite_competing_choices() {
    exercise_messages(false, true).await;
}

fn result_messages() -> crate::messages::Content {
    let mut content = power_messages();
    content.power.clear();
    content
        .with_result(vec![
            nfs_content::messages::result::Invitation::new(
                30,
                20,
                3,
                [HASH, HASH + 1, HASH + 2],
                123,
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(7),
            )
            .unwrap(),
        ])
        .unwrap()
}

fn power_messages() -> crate::messages::Content {
    let mut content = messages();
    content.active.clear();
    content.catalog = Catalog::new(
        3,
        (0..3)
            .map(|i| {
                (
                    (format!("constructed/{i}"), 0),
                    Message {
                        name: format!("constructed/{i}"),
                        hash: HASH + i,
                        flat: 10 + i as u16,
                        persisted: true,
                    },
                )
            })
            .collect(),
    )
    .unwrap();
    content
        .with_power(vec![
            nfs_content::messages::power::Invitation::new(
                30,
                3,
                [HASH, HASH + 1, HASH + 2],
                [10, 30],
                std::time::Duration::from_secs(7),
            )
            .unwrap(),
        ])
        .unwrap()
}

async fn exercise_messages(power: bool, result: bool) {
    let (mut listener, participant, _) = setup();
    let (repository, account, snapshot) = crate::progression::tests::local();
    let repository = Arc::new(Failable {
        inner: repository,
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    let progression = Arc::new(
        crate::progression::Content::from_json(&crate::progression::tests::combined())
            .unwrap()
            .with_messages(if result {
                result_messages()
            } else if power {
                power_messages()
            } else {
                messages()
            }),
    );
    let mut definition = crate::progression::tests::table_definition();
    for table in definition["tables"].as_array_mut().unwrap() {
        if table["name"] == "StaticMessage" || table["name"] == "UgStaticMessages" {
            let column = if table["name"] == "StaticMessage" {
                "Asset"
            } else {
                "MessageHash"
            };
            table["columns"] = serde_json::json!([
                {"name":column,"key":crate::persistent::key(column),"value_type":"int","default":"0"},
                {"name":"Read","key":crate::persistent::key("Read"),"value_type":"int","default":"0"}
            ]);
        }
    }
    let tables = Arc::new(crate::persistent::Catalog::from_json(&definition).unwrap());
    let service = crate::net::items::Service::new(
        repository.clone(),
        Arc::new(nfs_world::items::InventoryCatalog::new([], vec![]).unwrap()),
    )
    .with_persistent(tables.clone());
    let profile = service.bind(account);
    listener.progression = Some(progression.clone());
    listener.persistent = Some(Arc::new(tables.view(&snapshot).unwrap()));
    listener
        .advance_active_invitations(listener.world_ms)
        .unwrap();
    assert!(listener.story_pending.is_empty());
    let mut request = Request {
        participant,
        player_index: 0,
        progression: progression.clone(),
        action: Action::Complete { objective: 10 },
    };
    for objective in [10, 20] {
        request.action = Action::Complete { objective };
        listener.persistent = Some(
            profile
                .story_for_test(&request, Timestamp(10))
                .await
                .unwrap()
                .loaded,
        );
    }
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
    listener.story_pending = VecDeque::from(vec![request.clone(); MAX_PENDING]);
    listener
        .advance_active_invitations(listener.world_ms)
        .unwrap();
    assert_eq!(listener.story_pending.len(), MAX_PENDING);
    listener.story_pending.clear();
    if result {
        listener
            .advance_active_invitations(listener.world_ms + 6999)
            .unwrap();
        assert!(listener.take_story().is_none());
        assert!(
            listener.active_invitations.result[&(Manager::Normal, 30)]
                .hash
                .is_none()
        );
    }
    if power {
        listener
            .advance_active_invitations(listener.world_ms + 6999)
            .unwrap();
        assert!(listener.take_story().is_none());
        assert!(
            listener.active_invitations.power[&(Manager::Normal, 30)]
                .hash
                .is_none()
        );
        // An existing car with unavailable/invalid figures is not permission to
        // choose the default. Failed evaluation leaves the choice unmodified.
        listener.world_cars.insert(8190, (participant, true));
        assert!(
            listener
                .advance_active_invitations(listener.world_ms + 7000)
                .is_err()
        );
        assert!(
            listener.active_invitations.power[&(Manager::Normal, 30)]
                .hash
                .is_none()
        );
        listener.world_cars.clear();
        listener.story_pending = VecDeque::from(vec![request.clone(); MAX_PENDING]);
        listener
            .advance_active_invitations(listener.world_ms + 7000)
            .unwrap();
        assert_eq!(
            listener.active_invitations.power[&(Manager::Normal, 30)].hash,
            Some(HASH)
        );
        assert!(!listener.active_invitations.power[&(Manager::Normal, 30)].queued);
        listener.story_pending.clear();
    }
    listener
        .advance_active_invitations(listener.world_ms + 7000)
        .unwrap();
    let send = listener.take_story().unwrap();
    listener.world_ms += 7000;
    assert_eq!(
        send.action,
        if power || result {
            Action::ConditionalMessage {
                objective: 30,
                hash: HASH,
                manager: Manager::Normal,
            }
        } else {
            Action::Message {
                hash: HASH,
                manager: Manager::Normal,
            }
        }
    );
    assert_eq!(send.participant, participant);
    let before = repository.snapshot(account).unwrap();
    let calls = listener.pending_rpcs.len();
    listener
        .advance_active_invitations(listener.world_ms)
        .unwrap();
    assert!(listener.take_story().is_none());
    assert_eq!(repository.snapshot(account).unwrap(), before);
    assert_eq!(listener.pending_rpcs.len(), calls);
    // The same hash in another manager is an independent durable message;
    // this generation change must not duplicate our in-flight normal send.
    request.action = Action::Message {
        hash: HASH,
        manager: Manager::Underground,
    };
    listener.persistent = Some(
        profile
            .story_for_test(&request, Timestamp(11))
            .await
            .unwrap()
            .loaded,
    );
    listener
        .advance_active_invitations(listener.world_ms)
        .unwrap();
    assert!(listener.take_story().is_none());
    let before_failure = repository.snapshot(account).unwrap();
    let calls_before_failure = listener.pending_rpcs.len();
    repository
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(profile.story_for_test(&send, Timestamp(12)).await.is_err());
    assert_eq!(repository.snapshot(account).unwrap(), before_failure);
    assert_eq!(listener.pending_rpcs.len(), calls_before_failure);
    repository
        .fail
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let lagging_loaded = listener.persistent.clone();
    let committed = profile.story_for_test(&send, Timestamp(12)).await.unwrap();
    listener.story_committed(&send, committed.clone()).unwrap();
    let added:Vec<_>=listener.pending_rpcs.iter().skip(calls).filter(|rpc|
        matches!(rpc,HostRpc::GameMode(c) if c.method==2 && matches!(c.arguments,nfs_world::game_mode::Arguments::Message{flat:10,flag:false,..}))).collect();
    assert_eq!(added.len(), 1);
    let mut published = listener.pending_rpcs.len();
    listener.story_committed(&send, committed).unwrap();
    assert_eq!(listener.pending_rpcs.len(), published);
    if power || result {
        let saved = repository.snapshot(account).unwrap();
        let mut competing = send.clone();
        competing.action = Action::ConditionalMessage {
            objective: 30,
            hash: HASH + 1,
            manager: Manager::Normal,
        };
        let committed = profile
            .story_for_test(&competing, Timestamp(13))
            .await
            .unwrap();
        listener
            .story_committed(&competing, committed.clone())
            .unwrap();
        assert_eq!(repository.snapshot(account).unwrap(), saved);
        assert_eq!(listener.pending_rpcs.len(), published);
        // A connection whose view predates the winner must announce the
        // committed winner, even though its own queued choice lost the race.
        listener.persistent = lagging_loaded;
        listener.story_committed(&competing, committed).unwrap();
        assert_eq!(listener.pending_rpcs.len(), published + 1);
        assert!(
            matches!(listener.pending_rpcs.back(),Some(HostRpc::GameMode(c))
            if c.method==2 && matches!(c.arguments,nfs_world::game_mode::Arguments::Message{flat:10,flag:false,..}))
        );
        published += 1;
        competing.action = Action::ConditionalMessage {
            objective: 30,
            hash: 999,
            manager: Manager::Normal,
        };
        assert!(
            profile
                .story_for_test(&competing, Timestamp(14))
                .await
                .is_err()
        );
        assert_eq!(repository.snapshot(account).unwrap(), saved);
    }
    // A new connection to this saved account must restore, not re-announce.
    assert_eq!(listener.pending_rpcs.len(), published);
    let (mut restarted, _, _) = setup();
    restarted.progression = Some(progression);
    restarted.persistent = Some(Arc::new(
        tables.view(&repository.snapshot(account).unwrap()).unwrap(),
    ));
    restarted
        .advance_active_invitations(restarted.world_ms)
        .unwrap();
    assert!(restarted.take_story().is_none());
    assert!(restarted.active_invitations.queued.is_empty());
    let rows = nfs_services::messages::rows(
        listener.persistent.as_ref().unwrap().tables(),
        Manager::Normal.table(),
    )
    .unwrap();
    assert_eq!(
        rows,
        vec![nfs_services::messages::Row {
            index: 0,
            hash: HASH,
            read: false
        }]
    );
    assert_eq!(
        nfs_services::messages::rows(
            listener.persistent.as_ref().unwrap().tables(),
            Manager::Underground.table()
        )
        .unwrap(),
        rows
    );
}

#[test]
fn invalid_objectives_reject_atomically() {
    let (mut listener, _, _) = setup();
    let (_, _, snapshot) = crate::progression::tests::local();
    let mut content = messages();
    content.active[0].objective = 999;
    listener.progression = Some(Arc::new(
        crate::progression::Content::from_json(&crate::progression::tests::combined())
            .unwrap()
            .with_messages(content),
    ));
    listener.persistent = Some(Arc::new(
        crate::progression::tests::tables().view(&snapshot).unwrap(),
    ));
    assert!(
        listener
            .advance_active_invitations(listener.world_ms)
            .is_err()
    );
    assert!(listener.active_invitations.generation.is_none());
    assert!(listener.story_pending.is_empty());
}

#[test]
fn power_choice_latches_before_queueing_and_preserves_missing_invalid_and_delayed_inputs() {
    let plan = power_messages().power.remove(0);
    let initial = Choice {
        started_ms: 100,
        hash: None,
        queued: false,
    };
    let mut mid = initial.clone();
    mid.select(&plan, 100, Some((110, 100))).unwrap();
    assert_eq!(mid.hash, Some(HASH + 1));
    mid.select(&plan, 7200, Some((200, 100))).unwrap();
    assert_eq!(
        mid.hash,
        Some(HASH + 1),
        "a car change cannot send another tier"
    );
    let mut high = initial.clone();
    high.select(&plan, 101, Some((130, 100))).unwrap();
    assert_eq!(high.hash, Some(HASH + 2));
    let mut missing = initial.clone();
    missing.select(&plan, 7099, None).unwrap();
    assert!(missing.hash.is_none());
    missing.select(&plan, 7100, None).unwrap();
    assert_eq!(missing.hash, Some(HASH));
    let mut invalid = initial;
    assert!(invalid.select(&plan, 100, Some((1501, 100))).is_err());
    assert!(invalid.hash.is_none());
    assert!(invalid.select(&plan, 99, Some((110, 100))).is_err());
    assert!(invalid.hash.is_none());
    invalid.select(&plan, 101, Some((109, 100))).unwrap();
    assert_eq!(invalid.hash, Some(HASH));
    let tiny = nfs_content::messages::power::Invitation::new(
        30,
        3,
        [HASH, HASH + 1, HASH + 2],
        [10, 30],
        std::time::Duration::from_micros(500),
    )
    .unwrap();
    let mut pending = Choice {
        started_ms: 100,
        hash: None,
        queued: false,
    };
    pending.select(&tiny, 100, None).unwrap();
    assert!(pending.hash.is_none());
    pending.select(&tiny, 101, None).unwrap();
    assert_eq!(pending.hash, Some(HASH));
}
