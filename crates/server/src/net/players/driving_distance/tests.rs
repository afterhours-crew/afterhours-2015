// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::{
    garage_logic::{GarageLogic, Which},
    persistent::key,
};
use nfs_storage::{AccountId, InventoryRepository, MemoryRepository, Timestamp};
use nfs_world::game_mode::Arguments;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct Repository {
    inner: MemoryRepository,
    fail: AtomicBool,
}
impl InventoryRepository for Repository {
    fn open(&self, a: AccountId) -> Result<nfs_storage::Snapshot, nfs_storage::Error> {
        self.inner.open(a)
    }
    fn snapshot(&self, a: AccountId) -> Result<nfs_storage::Snapshot, nfs_storage::Error> {
        self.inner.snapshot(a)
    }
    fn receipt(
        &self,
        a: AccountId,
        id: u64,
    ) -> Result<Option<nfs_storage::Applied>, nfs_storage::Error> {
        self.inner.receipt(a, id)
    }
    fn apply(
        &self,
        a: AccountId,
        b: &nfs_storage::Batch,
        n: Timestamp,
    ) -> Result<nfs_storage::Applied, nfs_storage::Error> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(nfs_storage::Error::Storage);
        }
        self.inner.apply(a, b, n)
    }
}

struct Case {
    listener: PlayerListener,
    repository: Arc<Repository>,
    profile: crate::net::items::Profile,
    account: AccountId,
    participant: u16,
    player: u16,
    car: u16,
}

fn case() -> Case {
    let (mut listener, participant, items) = super::super::prologue::tests::setup();
    let owner = crate::vehicle_content::population::Owner {
        connection: HOST_SELECTOR as u8,
        persona: listener.persona,
        participant,
    };
    let content = listener.vehicles.clone().unwrap();
    let layout = crate::vehicle_content::population::tests::world_layout(&content.layout);
    let inventory = crate::vehicle_content::population::tests::inventory(items);
    let population = listener.population.as_mut().unwrap();
    population
        .spawn(
            &mut listener.players,
            owner,
            inventory.clone(),
            &content.vehicles,
            &layout,
            [0.; 3],
        )
        .unwrap();
    let spawned = population
        .spawn_world(
            &mut listener.players,
            owner,
            &inventory,
            &content.vehicles,
            &layout,
        )
        .unwrap();
    let car = spawned
        .records
        .iter()
        .find(|r| matches!(r.initial, Some(replication::Initial::Vehicle { .. })))
        .unwrap()
        .id;
    listener.world_cars.insert(car, (participant, true));
    listener.prologue = None;
    listener.population = None;
    listener.inventory = None;
    listener.replication_origin_ms = Some(0);
    listener.world_ms = 0;
    let scene_key = layout.item_scene_key;
    let logic = Arc::new(GarageLogic::from_json(&json!({
        "format":"nfs-garage-logic","version":1,"build_sha256":crate::content::BUILD,
        "content":{"catalogs":[{"bundle":87,"entries":[[14,1]]}],
            "profiles":[{"asset":{"bundle":87,"type_id":14,"local_index":0},
                "serializers":["root","rpc_variant","rpc_variant"]}]},
        "spawn":{"entities":[],"scene_updates":[]}, "entry":{"entities":[],"scene_updates":[]},
        "exit":{"entities":[],"scene_updates":[],"events":[{"target":{"scene_content_key":scene_key},"entity":1,"event":1}]},
        "arrived":{"entities":[{"capture_id":8,"asset":{"bundle":87,"type_id":14,"local_index":0},
            "blueprint":{"scene_content_key":scene_key},"roles":[{"kind":"root","bus_count":1},
                {"kind":"variant","tag":null,"value":null},{"kind":"variant","tag":null,"value":null}]}],"scene_updates":[]},
        "driving_distance":{"role":8,"kilometres":1,"miles":2,"period_seconds":1,"metres_per_mile":1609.344}
    })).unwrap());
    let mut ghosts = BTreeMap::new();
    logic
        .produce(
            Which::Arrived,
            &mut listener.players,
            participant,
            &mut ghosts,
        )
        .unwrap();
    listener.logic = Some(logic);
    listener.logic_ghosts.insert(participant, ghosts);
    let progression = Arc::new(
        crate::progression::Content::from_json(&crate::progression::tests::combined()).unwrap(),
    );
    let mut definition = crate::progression::tests::table_definition();
    let stats = definition["tables"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["name"] == "GameplayStatsTable")
        .unwrap();
    stats["columns"][0] = json!({"name":"DistanceDriven","key":key("DistanceDriven"),"value_type":"int","default":"0"});
    let tables = Arc::new(crate::persistent::Catalog::from_json(&definition).unwrap());
    let repository = Arc::new(Repository {
        inner: MemoryRepository::default(),
        fail: AtomicBool::new(false),
    });
    let account = AccountId::from_owned_config([66; 16]).unwrap();
    let catalog = Arc::new(nfs_world::items::InventoryCatalog::new([], vec![]).unwrap());
    let initial =
        nfs_services::inventory::load(repository.as_ref(), account, &catalog, 1, Timestamp(1))
            .unwrap();
    let (_, loaded) = tables
        .ensure_loaded(repository.as_ref(), account, initial, Timestamp(2))
        .unwrap();
    let restored = progression
        .catalog
        .restore(&loaded, progression.settings)
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
    listener.progression = Some(progression);
    listener.persistent = Some(Arc::new(loaded));
    let profile = crate::net::items::Service::new(repository.clone(), catalog)
        .with_persistent(tables)
        .bind(account);
    let replication::Initial::Participant(owner) =
        &listener.players.objects().get(participant).unwrap().initial
    else {
        panic!()
    };
    let player = owner.player;
    Case {
        listener,
        repository,
        profile,
        account,
        participant,
        player,
        car,
    }
}

impl Case {
    fn move_to(&mut self, world_ms: u64, x: f32) {
        let physics = self
            .listener
            .players
            .vehicle_physics(HOST_SELECTOR as u8, self.listener.persona, self.car)
            .unwrap();
        let mut motion = physics.decode([0.; 3]).unwrap();
        motion.position = [x, 0., 0.];
        let controls = replication::vehicle::chassis::Controls::new(0).unwrap();
        let physics = motion.encode([0.; 3], &controls).unwrap();
        self.listener
            .players
            .set_vehicle_physics(
                HOST_SELECTOR as u8,
                self.listener.persona,
                self.car,
                physics,
            )
            .unwrap();
        self.tick(world_ms);
    }
    fn tick(&mut self, world_ms: u64) {
        self.listener.world_ms = world_ms;
        self.listener.advance_driving_distance(world_ms);
    }
    async fn commit(&mut self) -> Request {
        let request = self.listener.take_story().expect("distance job");
        let committed = self
            .profile
            .story_for_test(&request, Timestamp(20))
            .await
            .unwrap();
        self.listener.story_committed(&request, committed).unwrap();
        request
    }
    fn metres(&self) -> i32 {
        let snapshot = self.repository.snapshot(self.account).unwrap();
        match snapshot.tables[&key("GameplayStatsTable")].rows[&0][&key("DistanceDriven")] {
            nfs_storage::tables::Value::Int(value) => value,
            _ => panic!(),
        }
    }
    fn publications(&mut self) -> Vec<(u16, u32)> {
        let ghost = self.listener.logic_ghosts[&self.participant][&8];
        self.listener
            .pending_rpcs
            .drain(..)
            .map(|call| {
                let HostRpc::GameMode(call) = call else {
                    panic!("unexpected call")
                };
                assert_eq!(call.target, ghost);
                assert_eq!(call.reference, Some(self.player));
                assert_ne!(call.reference, Some(self.participant));
                assert_eq!(call.method, 0);
                let Arguments::Word(value) = call.arguments else {
                    panic!()
                };
                (call.selector, value)
            })
            .collect()
    }
}

#[tokio::test]
async fn publishes_only_committed_whole_units_to_the_owned_player_and_coalesces_jobs() {
    let mut c = case();
    c.move_to(0, 0.);
    c.move_to(1000, 999.);
    assert_eq!(c.listener.distance.cumulative, 999);
    assert_eq!(c.metres(), 0);
    assert!(c.publications().is_empty());
    c.move_to(2000, 1609.5);
    assert_eq!(c.listener.story_pending.len(), 1);
    c.commit().await;
    assert_eq!(c.metres(), 999);
    c.tick(2000);
    assert_eq!(c.publications(), [(1, 0), (2, 0)]);
    c.commit().await;
    c.tick(2000);
    assert_eq!(c.metres(), 1609);
    assert_eq!(c.publications(), [(1, 1)]);
    c.move_to(3000, 1610.);
    c.commit().await;
    c.tick(3000);
    assert_eq!(c.publications(), [(2, 1)]);
    c.tick(3500);
    assert!(c.publications().is_empty());
    assert!(c.listener.take_story().is_none());
}

#[tokio::test]
async fn continuous_new_samples_do_not_starve_committed_publication() {
    let mut c = case();
    c.move_to(0, 0.);
    c.move_to(1000, 1000.);
    c.move_to(2000, 2000.);
    c.commit().await;
    c.tick(2000);
    assert_eq!(c.metres(), 1000);
    assert_eq!(c.publications(), [(1, 1), (2, 0)]);
    assert_eq!(c.listener.distance.queued, Some(2000));
    c.move_to(3000, 3000.);
    c.commit().await;
    c.tick(3000);
    assert_eq!(c.metres(), 2000);
    assert_eq!(c.publications(), [(1, 2), (2, 1)]);
    assert_eq!(c.listener.distance.queued, Some(3000));
}

#[tokio::test]
async fn storage_failure_retries_without_publishing_or_refusing_movement() {
    let mut c = case();
    c.move_to(0, 0.);
    c.move_to(1000, 2000.);
    let request = c.listener.take_story().unwrap();
    c.repository.fail.store(true, Ordering::SeqCst);
    assert!(
        c.profile
            .story_for_test(&request, Timestamp(30))
            .await
            .is_err()
    );
    c.listener.story_failed(&request);
    assert_eq!(c.metres(), 0);
    assert!(c.publications().is_empty());
    c.tick(1500);
    assert!(c.listener.take_story().is_none());
    c.repository.fail.store(false, Ordering::SeqCst);
    c.move_to(2000, 2100.);
    c.commit().await;
    c.tick(2000);
    assert_eq!(c.metres(), 2100);
    assert_eq!(c.publications(), [(1, 2), (2, 1)]);
    assert_eq!(c.listener.refused, 0);
    assert_eq!(c.listener.distance.failures, 1);
}

#[tokio::test]
async fn retiring_logic_keeps_unsaved_distance_and_new_lifetime_reanchors() {
    let mut c = case();
    c.move_to(0, 0.);
    c.move_to(1000, 1200.);
    let old = c
        .listener
        .logic_ghosts
        .get_mut(&c.participant)
        .unwrap()
        .remove(&8)
        .unwrap();
    c.listener.players.remove_entities(&[old]).unwrap();
    c.tick(1100);
    assert!(c.listener.distance.lifetimes.is_empty());
    c.commit().await;
    assert_eq!(c.metres(), 1200);
    c.tick(1200);
    assert!(c.publications_retired().is_empty());
    let logic = c.listener.logic.clone().unwrap();
    logic
        .produce(
            Which::Arrived,
            &mut c.listener.players,
            c.participant,
            c.listener.logic_ghosts.get_mut(&c.participant).unwrap(),
        )
        .unwrap();
    c.move_to(2000, 20000.);
    c.move_to(3000, 20100.);
    c.commit().await;
    c.tick(3000);
    assert_eq!(c.metres(), 1300);
    assert_eq!(c.publications(), [(1, 1), (2, 0)]);
}

impl Case {
    fn publications_retired(&mut self) -> Vec<HostRpc> {
        self.listener.pending_rpcs.drain(..).collect()
    }
}

#[tokio::test]
async fn full_publication_queue_retries_both_units_and_drops_calls_to_a_retired_target() {
    let mut c = case();
    c.move_to(0, 0.);
    c.move_to(1000, 1800.);
    c.commit().await;
    let ghost = c.listener.logic_ghosts[&c.participant][&8];
    let filler = HostRpc::GameMode(
        crate::event_logic::call(
            c.listener.players.objects(),
            ghost,
            1,
            1,
            Some(c.player),
            Arguments::Word(99),
        )
        .unwrap(),
    );
    c.listener.pending_rpcs =
        std::iter::repeat_n(filler, nfs_world::session::MAX_RPC_QUEUE).collect();
    c.tick(1000);
    assert_eq!(
        c.listener.distance.lifetimes[&c.participant].published,
        None
    );
    c.listener.pending_rpcs.clear();
    c.tick(1000);
    assert_eq!(c.listener.pending_rpcs.len(), 2);
    c.listener.pending_rpcs.push_back(filler);
    c.listener.players.remove_entities(&[ghost]).unwrap();
    c.listener
        .logic_ghosts
        .get_mut(&c.participant)
        .unwrap()
        .remove(&8);
    c.listener.prune_distance_publications();
    assert_eq!(c.listener.pending_rpcs, VecDeque::from([filler]));
    assert_eq!(c.metres(), 1800);
    assert_eq!(c.listener.distance.failures, 0);
}

#[tokio::test]
async fn full_job_queue_preserves_distance_until_an_account_checkpoint_can_run() {
    let mut c = case();
    c.move_to(0, 0.);
    c.move_to(1000, 100.);
    c.commit().await;
    let filler = Request {
        participant: c.participant,
        player_index: 0,
        progression: c.listener.progression.clone().unwrap(),
        action: Action::Tutorial { key: 77 },
    };
    c.listener.story_pending = std::iter::repeat_n(filler, MAX_PENDING).collect();
    c.move_to(2000, 1000.);
    c.move_to(3000, 1700.);
    assert_eq!(c.listener.distance.cumulative, 1700);
    assert_eq!(c.listener.distance.queued, None);
    assert_eq!(c.metres(), 100);
    c.listener.story_pending.clear();
    c.tick(3000);
    c.commit().await;
    assert_eq!(c.metres(), 1700);
    assert_eq!(c.listener.distance.failures, 0);
}

#[test]
fn foreign_logic_or_writer_cannot_publish_or_acknowledge_owned_distance() {
    let mut c = case();
    c.move_to(0, 0.);
    let player = c
        .listener
        .players
        .create(
            2,
            999,
            replication::players::Request {
                name: b"Other".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = c.listener.players.join(2, 999, player).unwrap().unwrap().id;
    let mut ghosts = BTreeMap::new();
    c.listener
        .logic
        .as_ref()
        .unwrap()
        .produce(
            Which::Arrived,
            &mut c.listener.players,
            participant,
            &mut ghosts,
        )
        .unwrap();
    c.listener
        .logic_ghosts
        .get_mut(&c.participant)
        .unwrap()
        .insert(8, ghosts[&8]);
    c.move_to(1000, 2000.);
    assert_eq!(c.listener.distance.cumulative, 0);
    assert!(c.listener.pending_rpcs.is_empty());
    assert!(c.listener.story_pending.is_empty());
    assert_eq!(c.listener.refused, 0);
    assert_eq!(
        c.listener
            .observation(1000, 0, 0, false)
            .health
            .driving_distance_failures,
        1
    );
    assert!(
        c.listener
            .validate_distance_commit(&Action::Distance {
                writer: Writer::default(),
                cumulative: 0
            })
            .is_err()
    );
}

#[tokio::test]
async fn an_explicit_teleport_drops_the_anchor_without_erasing_prior_uncommitted_distance() {
    let mut c = case();
    c.move_to(0, 0.);
    c.move_to(1000, 100.);
    c.listener.pending_teleports.push_back((
        1500,
        nfs_world::teleport::Command {
            endpoint: nfs_world::participants::Endpoint {
                scene: 1,
                selector: 1,
                serial: nfs_protocol::world::rpc::Serial::new(0).unwrap(),
            },
            participant: c.participant,
            vehicle: c.car,
            position: [20000., 0., 0.],
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            speed: 0.,
        },
    ));
    c.move_to(1500, 20000.);
    c.listener.pending_teleports.clear();
    c.move_to(2000, 20000.);
    c.move_to(3000, 20100.);
    assert_eq!(c.listener.distance.cumulative, 200);
    c.commit().await;
    c.tick(3000);
    c.commit().await;
    assert_eq!(c.metres(), 200);
    assert_eq!(c.listener.distance.failures, 0);
}
