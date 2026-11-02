// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_protocol::world::rpc::Serial;
use nfs_storage::{InventoryRepository, Timestamp};
use nfs_world::{bits::BitWriter, game_mode::Arguments};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn binding() -> Endpoint {
    Endpoint {
        scene: 17,
        selector: 3,
        serial: Serial::new(4).unwrap(),
    }
}

fn values(calls: impl IntoIterator<Item = HostRpc>) -> Vec<Option<u32>> {
    calls
        .into_iter()
        .map(|call| match call {
            HostRpc::GameMode(c) => {
                assert_eq!(c.reference, None);
                match (c.method, c.arguments) {
                    (0, Arguments::None) => None,
                    (1, Arguments::Word(id)) => Some(id),
                    _ => panic!("unexpected map call"),
                }
            }
            _ => panic!("unexpected call kind"),
        })
        .collect()
}

#[test]
fn large_restore_waits_for_capacity_and_rebinds_without_leaking_old_acknowledgements() {
    let ids: BTreeSet<_> = (0..nfs_services::map_points::MAX_IDS as u32).collect();
    let mut state = State::default();
    state.bind(29, binding());
    let before = state.clone();
    assert!(state.publish(&ids, 0).is_empty());
    assert_eq!(state, before);
    let mut delivered = values(state.publish(&ids, 1));
    assert_eq!(delivered, [None]);
    while delivered.len() <= ids.len() {
        let calls = state.publish(&ids, 17);
        assert!(!calls.is_empty() && calls.len() <= 17);
        delivered.extend(values(calls));
    }
    assert_eq!(
        delivered,
        std::iter::once(None)
            .chain(ids.iter().copied().map(Some))
            .collect::<Vec<_>>()
    );
    assert!(state.publish(&ids, 10).is_empty());
    state.acknowledgements.insert(18);
    assert_eq!(values(state.publish(&ids, 10)), [Some(18)]);
    state.acknowledgements.insert(19);
    let next = Endpoint {
        scene: 41,
        ..binding()
    };
    state.bind(29, next);
    assert!(state.acknowledgements.is_empty());
    let calls = state.publish(&ids, 2);
    assert!(
        calls
            .iter()
            .all(|c| matches!(c, HostRpc::GameMode(call) if call.target == 41))
    );
    assert_eq!(values(calls), [None, Some(0)]);
    // Removing durable progress clears the projection, rather than retaining
    // stale markers from a previous save.
    assert_eq!(values(state.publish(&BTreeSet::new(), 10)), [None]);
    assert!(state.published.is_empty());
    state.bind(30, next);
    assert_eq!(
        values(state.publish(&BTreeSet::from([99]), 10)),
        [None, Some(99)]
    );
}

struct Failable {
    inner: nfs_storage::MemoryRepository,
    fail: AtomicBool,
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
        if self.fail.load(Ordering::SeqCst) {
            Err(nfs_storage::Error::Storage)
        } else {
            self.inner.apply(a, b, now)
        }
    }
}

fn setup() -> (PlayerListener, u16, crate::persistent::Catalog) {
    let (mut listener, participant, _) = super::super::prologue::tests::setup();
    listener.prologue = None;
    listener.population = None;
    listener.inventory = None;
    listener.roles = Some(
        crate::scene_roles::SceneRoles::from_json(&serde_json::json!({
            "level":"test/world","gameplay":101,"startup":909,"garage":103,"progression":104,
            "traffic":[11,12,13,14,15,16,17,18,19,20],
            "customization_timer":{"bundle":2,"type_id":3,"local_index":4},
            "streaming_gate":{"bundle":5,"type_id":6,"local_index":7},
            "seen_points":{"scene":909,"serializer":1}
        }))
        .unwrap(),
    );
    let progression = Arc::new(
        crate::progression::Content::from_json(&crate::progression::tests::combined()).unwrap(),
    );
    let mut definition = crate::progression::tests::table_definition();
    for table in definition["tables"].as_array_mut().unwrap() {
        if table["name"] == nfs_services::map_points::TABLE {
            table["columns"] = serde_json::json!([{"name":"PoiId","key":crate::persistent::key("PoiId"),"value_type":"int","default":"0"}]);
        }
    }
    let tables = crate::persistent::Catalog::from_json(&definition).unwrap();
    let (_, _, snapshot) = crate::progression::tests::local();
    let loaded = Arc::new(tables.view(&snapshot).unwrap());
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
    listener.persistent = Some(loaded);
    (listener, participant, tables)
}

fn wire(endpoint: Endpoint, participant: u16, id: u32) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(endpoint.selector.into(), 9)
        .put(2, 32)
        .put(id.into(), 32)
        .put(0x55, 7);
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(endpoint.scene.into(), 13)
        .put(participant.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}

fn send(listener: &mut PlayerListener, sequence: u8, body: &BitWriter) -> bool {
    let packet = frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(sequence),
                messages: vec![frame::Message::Opaque {
                    index: 47,
                    body: body.span(),
                }],
            }],
        )
        .build()
        .unwrap();
    listener.frame(Queued {
        advertised: None,
        frame: sequence.into(),
        data: packet.span(),
    })
}

#[tokio::test]
async fn worker_commits_before_ack_retries_under_pressure_and_restores_on_reconnect() {
    let (mut listener, participant, tables) = setup();
    let (repo, account, _) = crate::progression::tests::local();
    let repository = Arc::new(Failable {
        inner: repo,
        fail: AtomicBool::new(false),
    });
    let profile = crate::net::items::Service::new(
        repository.clone(),
        Arc::new(nfs_world::items::InventoryCatalog::new([], vec![]).unwrap()),
    )
    .with_persistent(Arc::new(tables.clone()))
    .bind(account);
    let endpoint = endpoint(
        &listener.players,
        listener.roles.as_ref().unwrap().seen_points.unwrap(),
    )
    .unwrap()
    .unwrap();
    listener.advance_map_points().unwrap();
    assert_eq!(values(listener.pending_rpcs.drain(..)), [None]);
    let input = wire(endpoint, participant, u32::MAX);
    assert!(send(&mut listener, 1, &input));
    assert!(listener.pending_rpcs.is_empty());
    let request = listener.take_story().unwrap();
    let before = repository.snapshot(account).unwrap();
    repository.fail.store(true, Ordering::SeqCst);
    assert!(
        profile
            .story_for_test(&request, Timestamp(10))
            .await
            .is_err()
    );
    listener.story_failed(&request);
    assert_eq!(repository.snapshot(account).unwrap(), before);
    listener.advance_map_points().unwrap();
    assert!(listener.pending_rpcs.is_empty());
    repository.fail.store(false, Ordering::SeqCst);
    let committed = profile
        .story_for_test(&request, Timestamp(11))
        .await
        .unwrap();
    let saved = repository.snapshot(account).unwrap();
    assert_eq!(
        stored(&committed.loaded).unwrap(),
        BTreeSet::from([u32::MAX])
    );
    let mut expected = before.tables.clone();
    expected.insert(
        crate::persistent::key(nfs_services::map_points::TABLE),
        saved.tables[&crate::persistent::key(nfs_services::map_points::TABLE)].clone(),
    );
    assert_eq!(saved.tables, expected);
    assert_eq!(saved.items, before.items);
    assert_eq!(saved.garage, before.garage);
    let filler = HostRpc::GameMode(nfs_world::map_points::publication(endpoint, None));
    listener.pending_rpcs = VecDeque::from(vec![filler; nfs_world::session::MAX_RPC_QUEUE]);
    listener.story_committed(&request, committed).unwrap();
    let pending = listener.seen_points.clone();
    listener.advance_map_points().unwrap();
    assert_eq!(listener.seen_points, pending);
    listener.pending_rpcs.clear();
    listener.advance_map_points().unwrap();
    assert_eq!(values(listener.pending_rpcs.drain(..)), [Some(u32::MAX)]);
    let repeated = profile
        .story_for_test(&request, Timestamp(12))
        .await
        .unwrap();
    assert_eq!(repository.snapshot(account).unwrap(), saved);
    listener.story_committed(&request, repeated).unwrap();
    listener.advance_map_points().unwrap();
    assert_eq!(values(listener.pending_rpcs.drain(..)), [Some(u32::MAX)]);
    let (mut restarted, _, _) = setup();
    restarted.persistent = Some(Arc::new(tables.view(&saved).unwrap()));
    restarted.advance_map_points().unwrap();
    assert_eq!(
        values(restarted.pending_rpcs.drain(..)),
        [None, Some(u32::MAX)]
    );
    restarted.persistent = Some(Arc::new(tables.view(&before).unwrap()));
    restarted.advance_map_points().unwrap();
    assert_eq!(values(restarted.pending_rpcs.drain(..)), [None]);
}

#[test]
fn listener_checks_reference_kind_owner_and_frame_admission_before_queueing_work() {
    let (mut listener, participant, _) = setup();
    let endpoint = endpoint(
        &listener.players,
        listener.roles.as_ref().unwrap().seen_points.unwrap(),
    )
    .unwrap()
    .unwrap();
    let player = listener
        .players
        .objects()
        .iter()
        .find_map(|(id, o)| matches!(o.initial, replication::Initial::Player(_)).then_some(id))
        .unwrap();
    let foreign_player = listener
        .players
        .create(
            HOST_SELECTOR as u8,
            listener.persona + 1,
            nfs_world::replication::players::Request {
                name: b"Other".to_vec(),
                flag: false,
                slot: 1,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let foreign_participant = listener
        .players
        .join(HOST_SELECTOR as u8, listener.persona + 1, foreign_player)
        .unwrap()
        .unwrap()
        .id;
    for bad in [0, player, foreign_player, foreign_participant, 8191] {
        assert!(!send(&mut listener, 1, &wire(endpoint, bad, 31)));
        assert!(listener.story_pending.is_empty());
    }
    let input = wire(endpoint, participant, 31);
    let request = listener
        .map_point_request(&listener.players, input.span())
        .unwrap()
        .unwrap();
    listener.story_pending = VecDeque::from(vec![request; MAX_PENDING]);
    assert!(!send(&mut listener, 1, &input));
    assert_eq!(listener.story_pending.len(), MAX_PENDING);
    listener.story_pending.clear();
    assert!(
        send(&mut listener, 1, &input),
        "refusal must not consume the reliable input"
    );
    assert_eq!(listener.story_pending.len(), 1);
    assert!(send(&mut listener, 1, &input));
    assert_eq!(listener.story_pending.len(), 1);
    let foreign = wire(
        Endpoint {
            scene: 8191,
            ..endpoint
        },
        participant,
        31,
    );
    assert!(
        listener
            .map_point_request(&listener.players, foreign.span())
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn replaced_scene_and_uncommitted_result_cannot_publish_a_stale_acknowledgement() {
    let (mut listener, participant, tables) = setup();
    let (repository, account, _) = crate::progression::tests::local();
    let endpoint = endpoint(
        &listener.players,
        listener.roles.as_ref().unwrap().seen_points.unwrap(),
    )
    .unwrap()
    .unwrap();
    let input = wire(endpoint, participant, 81);
    let request = listener
        .map_point_request(&listener.players, input.span())
        .unwrap()
        .unwrap();
    assert!(
        listener
            .committed_map_points(&request, listener.persistent.as_ref().unwrap())
            .is_err()
    );
    assert_eq!(listener.seen_points, State::default());
    let profile = crate::net::items::Service::new(
        Arc::new(repository),
        Arc::new(nfs_world::items::InventoryCatalog::new([], vec![]).unwrap()),
    )
    .with_persistent(Arc::new(tables))
    .bind(account);
    let result = profile
        .story_for_test(&request, Timestamp(10))
        .await
        .unwrap();
    listener
        .roles
        .as_mut()
        .unwrap()
        .seen_points
        .as_mut()
        .unwrap()
        .serializer = 2;
    listener.story_committed(&request, result).unwrap();
    assert!(listener.seen_points.acknowledgements.is_empty());
    listener.advance_map_points().unwrap();
    assert!(
        listener
            .pending_rpcs
            .iter()
            .all(|call| matches!(call,HostRpc::GameMode(c) if c.selector != endpoint.selector))
    );
    assert_eq!(values(listener.pending_rpcs.drain(..)), [None, Some(81)]);
}
