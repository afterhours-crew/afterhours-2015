// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::garage_logic::{GarageLogic, Which};
use crate::net::players::scoring::{AwardRequest, Credited};
use nfs_world::game_mode::Arguments;
use nfs_world::replication::entity::Initial;

struct Case {
    listener: PlayerListener,
    profile: crate::net::items::Profile,
    participant: u16,
    player: u16,
    ghost: u16,
    selector: u16,
}

fn world() -> (nfs_world::replication::players::Players, u16) {
    let (mut players, _) = crate::garage_logic::tests::world();
    let player = players
        .create(
            HOST_SELECTOR as u8,
            100,
            nfs_world::replication::players::Request {
                name: b"Owned".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players
        .join(HOST_SELECTOR as u8, 100, player)
        .unwrap()
        .unwrap()
        .id;
    (players, participant)
}

fn case() -> Case {
    let (mut players, participant) = world();
    let replication::Initial::Participant(owner) =
        &players.objects().get(participant).unwrap().initial
    else {
        panic!()
    };
    let player = owner.player;
    assert_ne!(player, participant);
    assert!(matches!(
        players.objects().get(player).unwrap().initial,
        replication::Initial::Player(_)
    ));
    let logic =
        Arc::new(GarageLogic::from_json(&crate::garage_logic::tests::reputation_json()).unwrap());
    let mut ghosts = BTreeMap::new();
    logic
        .produce_with_reputation(
            Which::Spawn,
            &mut players,
            participant,
            &mut ghosts,
            Some(scores(0)),
        )
        .unwrap();
    let ghost = ghosts[&277];
    let replication::Initial::Entity { fields, .. } =
        &players.objects().get(ghost).unwrap().initial
    else {
        panic!()
    };
    let selector = fields[3].rpc().unwrap().selector;
    let repository = Arc::new(MemoryRepository::new());
    let account = AccountId::from_owned_config([72; 16]).unwrap();
    let inventory = Arc::new(nfs_world::items::InventoryCatalog::new([], vec![]).unwrap());
    let catalog = tables();
    let initial =
        nfs_services::inventory::load(repository.as_ref(), account, &inventory, 1, Timestamp(1))
            .unwrap();
    let initial = catalog
        .ensure_loaded(repository.as_ref(), account, initial, Timestamp(2))
        .unwrap()
        .0;
    let profile = Service::new(repository, inventory)
        .with_persistent(catalog.clone())
        .bind(account);
    let mut listener = PlayerListener::new(100);
    listener.players = players;
    assert!(
        listener
            .players
            .owns_participant(HOST_SELECTOR as u8, 100, participant)
    );
    listener.logic = Some(logic);
    listener.logic_ghosts.insert(participant, ghosts);
    listener.persistent = Some(Arc::new(catalog.view(&initial).unwrap()));
    Case {
        listener,
        profile,
        participant,
        player,
        ghost,
        selector,
    }
}

fn scores(total: i32) -> Scores {
    Scores {
        total,
        build: 0,
        crew: 0,
        outlaw: 0,
        speed: total,
        style: 0,
    }
}

impl Case {
    fn request(&self, amount: i32, batch_id: u64) -> AwardRequest {
        AwardRequest {
            participant: self.participant,
            credit: scores(amount),
            // Inventory initialization owns batch 1. Runtime scoring supplies
            // its own disjoint session/sequence namespace.
            batch_id: 0x1000_0000 + batch_id,
            thresholds: Thresholds::new(vec![0, 10, 25]).unwrap(),
            player_index: Some(0),
            event: None,
            story: None,
        }
    }
    async fn commit(&self, request: &AwardRequest) -> Credited {
        self.profile
            .credit_for_test(request, Timestamp(3))
            .await
            .unwrap()
    }
    fn levels(&self) -> Vec<u32> {
        self.listener
            .pending_rpcs
            .iter()
            .map(|rpc| {
                let HostRpc::GameMode(call) = rpc else {
                    panic!("unexpected call")
                };
                assert_eq!(call.target, self.ghost);
                assert_eq!(call.selector, self.selector);
                assert_eq!(call.method, 0);
                assert_eq!(call.reference, Some(self.player));
                assert_ne!(call.reference, Some(self.participant));
                let Arguments::Word(value) = call.arguments else {
                    panic!()
                };
                // Exercise the production envelope encoder with the actual owned
                // target/player and the serializer's current publication serial.
                call.encode().unwrap();
                value
            })
            .collect()
    }
}

#[tokio::test]
async fn level_stat_follows_committed_property_changes_not_each_award_or_retry() {
    let mut c = case();
    let untouched = case();
    for (amount, batch, expected) in [
        (9, 1, vec![]),
        (1, 2, vec![2]),
        (1, 3, vec![2]),
        (20, 4, vec![2, 3]),
    ] {
        let request = c.request(amount, batch);
        let before = c.listener.players.objects().snapshot();
        let committed = c.commit(&request).await;
        assert!(
            !committed.replayed,
            "a fresh award cannot collide with initialization"
        );
        assert_eq!(
            c.listener.players.objects().snapshot(),
            before,
            "storage cannot publish"
        );
        c.listener.awarded(&request, committed).unwrap();
        assert_eq!(c.levels(), expected);
        let repeat = c.commit(&request).await;
        assert!(repeat.replayed);
        c.listener.awarded(&request, repeat).unwrap();
        assert_eq!(
            c.levels(),
            expected,
            "re-adoption must not repeat a changed event"
        );
    }
    assert!(untouched.levels().is_empty());
    assert_eq!(
        Scores::load(untouched.listener.persistent.as_ref().unwrap()).unwrap(),
        scores(0)
    );
}

#[tokio::test]
async fn newer_loaded_view_does_not_suppress_an_unpublished_level_change() {
    let mut c = case();
    let request = c.request(25, 1);
    let committed = c.commit(&request).await;
    c.listener.persistent = Some(committed.loaded.clone());
    c.listener.awarded(&request, committed).unwrap();
    assert_eq!(
        c.levels(),
        [3],
        "a multi-threshold credit publishes its resulting level once"
    );
}

#[tokio::test]
async fn full_queue_preserves_old_projection_and_retries_the_committed_level() {
    let mut c = case();
    let request = c.request(10, 1);
    let committed = c.commit(&request).await;
    let before = c.listener.players.objects().snapshot();
    let generation = c.listener.persistent.as_ref().unwrap().generation();
    let filler = HostRpc::Query(nfs_world::participants::EntityCall {
        entity: c.ghost,
        selector: c.selector,
        serial: nfs_protocol::world::rpc::Serial::new(0).unwrap(),
        method: 0,
    });
    c.listener
        .pending_rpcs
        .resize(nfs_world::session::MAX_RPC_QUEUE, filler);
    assert_eq!(
        c.listener.awarded(&request, committed),
        Err(replication::Error::Bound)
    );
    assert_eq!(c.listener.players.objects().snapshot(), before);
    assert_eq!(
        c.listener.persistent.as_ref().unwrap().generation(),
        generation
    );
    assert!(c.listener.scoring_state.dirty.is_empty());
    c.listener.pending_rpcs.clear();
    let retry = c.commit(&request).await;
    assert!(retry.replayed);
    c.listener.awarded(&request, retry).unwrap();
    assert_eq!(c.levels(), [2]);
}

#[tokio::test]
async fn foreign_participant_mapping_cannot_publish_or_mutate_its_projection() {
    let mut c = case();
    let request = c.request(10, 1);
    let committed = c.commit(&request).await;
    // A foreign participant cannot borrow this local account's mapping.
    c.listener.persona = 200;
    let before = c.listener.players.objects().snapshot();
    c.listener.awarded(&request, committed).unwrap();
    assert!(c.levels().is_empty());
    assert_eq!(c.listener.players.objects().snapshot(), before);
}

#[tokio::test]
async fn reconnect_initializes_the_stat_from_saved_level_without_repeating_the_change() {
    let mut c = case();
    let request = c.request(10, 1);
    let committed = c.commit(&request).await;
    c.listener.awarded(&request, committed).unwrap();
    let durable = Scores::load(c.listener.persistent.as_ref().unwrap()).unwrap();
    let logic = c.listener.logic.as_ref().unwrap();
    let (mut players, participant) = world();
    let mut ghosts = BTreeMap::new();
    logic
        .produce_with_reputation(
            Which::Spawn,
            &mut players,
            participant,
            &mut ghosts,
            Some(durable),
        )
        .unwrap();
    let replication::Initial::Entity { fields, .. } =
        &players.objects().get(ghosts[&277]).unwrap().initial
    else {
        panic!()
    };
    assert_eq!(fields[2], Initial::I32Property(Some(2)));
    assert!(matches!(
        fields[3],
        Initial::RpcVariant {
            value: Some((0, Some(2))),
            ..
        }
    ));
    c.listener.players = players;
    c.listener.logic_ghosts = BTreeMap::from([(participant, ghosts)]);
    c.listener.pending_rpcs.clear();
    let repeated = c.commit(&request).await;
    c.listener.awarded(&request, repeated).unwrap();
    assert!(c.levels().is_empty());
}
