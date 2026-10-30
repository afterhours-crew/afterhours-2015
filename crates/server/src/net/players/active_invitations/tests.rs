// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::net::players::prologue::tests::setup;
use nfs_content::messages::{Catalog, Managers, Message, active::Invitation};
use nfs_storage::{InventoryRepository, Timestamp};
use nfs_world::participants::HostRpc;
use std::sync::Arc;

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
    let (mut listener, participant, _) = setup();
    let (repository, account, snapshot) = crate::progression::tests::local();
    let repository = Arc::new(repository);
    let progression = Arc::new(
        crate::progression::Content::from_json(&crate::progression::tests::combined())
            .unwrap()
            .with_messages(messages()),
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
    listener.advance_active_invitations().unwrap();
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
    listener.advance_active_invitations().unwrap();
    assert_eq!(listener.story_pending.len(), MAX_PENDING);
    listener.story_pending.clear();
    listener.advance_active_invitations().unwrap();
    let send = listener.take_story().unwrap();
    assert_eq!(
        send.action,
        Action::Message {
            hash: HASH,
            manager: Manager::Normal
        }
    );
    assert_eq!(send.participant, participant);
    let before = repository.snapshot(account).unwrap();
    let calls = listener.pending_rpcs.len();
    listener.advance_active_invitations().unwrap();
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
    listener.advance_active_invitations().unwrap();
    assert!(listener.take_story().is_none());
    let committed = profile.story_for_test(&send, Timestamp(12)).await.unwrap();
    listener.story_committed(&send, committed.clone()).unwrap();
    let added:Vec<_>=listener.pending_rpcs.iter().skip(calls).filter(|rpc|
        matches!(rpc,HostRpc::GameMode(c) if c.method==2 && matches!(c.arguments,nfs_world::game_mode::Arguments::Message{flat:10,flag:false,..}))).collect();
    assert_eq!(added.len(), 1);
    let published = listener.pending_rpcs.len();
    listener.story_committed(&send, committed).unwrap();
    assert_eq!(listener.pending_rpcs.len(), published);
    // A new connection to this saved account must restore, not re-announce.
    let (mut restarted, _, _) = setup();
    restarted.progression = Some(progression);
    restarted.persistent = Some(Arc::new(
        tables.view(&repository.snapshot(account).unwrap()).unwrap(),
    ));
    restarted.advance_active_invitations().unwrap();
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
    assert!(listener.advance_active_invitations().is_err());
    assert!(listener.active_invitations.generation.is_none());
    assert!(listener.story_pending.is_empty());
}
