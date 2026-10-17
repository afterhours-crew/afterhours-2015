// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

pub(in crate::net) fn reload_inventory(
    jobs: &Exchanges,
    snapshot: &nfs_storage::Snapshot,
) -> nfs_world::items::Collection {
    nfs_services::inventory::collection(snapshot, jobs.profile.as_ref().unwrap().bindings())
        .unwrap()
}
use nfs_storage::{Batch, MemoryRepository, Op};
use nfs_world::{
    application::{Listener, Queued},
    bits::BitWriter,
    files::{Receiver, Record},
    items::{
        self, Definition, DefinitionClass, DefinitionFlags, Derived, Envelope, OwnershipLevel,
    },
};

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn catalog() -> Arc<InventoryCatalog> {
    Arc::new(
        InventoryCatalog::new(
            [(
                [1; 16],
                Definition {
                    class: DefinitionClass::CurrencyItemData,
                    buy_price: 200,
                    sell_price: 100,
                    quantity: 1,
                    ownership_level: OwnershipLevel::Owned,
                    flags: DefinitionFlags::default(),
                    sub_items: vec![],
                    additional_items: vec![],
                    default_derived: Some(Derived::Empty),
                },
            )],
            vec![[1; 16]],
        )
        .unwrap(),
    )
}
fn request(sequence: u32) -> Vec<u8> {
    items::Request {
        sequence,
        operation: items::LOAD_INVENTORY,
    }
    .encode()
    .to_vec()
}

#[test]
fn unsupported_file_and_items_warn_without_payloads_or_storage_access() {
    let repo = Arc::new(MemoryRepository::default());
    let service = Service::new(repo.clone(), catalog());
    let output = crate::diagnostics::testing::capture(|| {
        let mut exchanges = Exchanges::new(Some(service.bind(account(3))), 19);
        let mut listener = PlayerListener::new(999);
        for _ in 0..2 {
            transfer(
                &mut listener,
                0,
                b"do-not-log-file-name",
                b"do-not-log-file-body".to_vec(),
            );
            exchanges.start(&mut listener, 0, Timestamp(0));
        }
        transfer(&mut listener, 1, b"Items", vec![0; 5]);
        exchanges.start(&mut listener, 0, Timestamp(0));
        assert!(!exchanges.working());
        assert_eq!(repo.snapshot(account(3)), Err(nfs_storage::Error::Absent));
    });
    assert_eq!(output.matches("WARN").count(), 2, "{output}");
    for value in [
        "connection=19",
        "handler: 4",
        "handler: 5",
        "operation: 0",
        "unsupported_total=3",
        "suppressed_total=1",
    ] {
        assert!(output.contains(value), "{output}");
    }
    assert!(!output.contains("do-not-log"), "{output}");
}

/// Exercise File framing/acknowledgement for the item update integration tests.
pub(in crate::net) fn deliver_notification(exchanges: &mut Exchanges) -> items::UpdateNotification {
    let response = exchanges.pending[0].take().unwrap();
    let mut sender =
        nfs_world::files::sender::Sender::new(response.clone(), Default::default(), 0).unwrap();
    let mut receiver = Receiver::default();
    for ticket in 0..64 {
        let Some(record) = sender.offer(ticket).unwrap() else {
            break;
        };
        let wire = record.encode(sender.size()).unwrap();
        let (decoded, _) = nfs_world::files::decode(wire.span(), receiver.size()).unwrap();
        receiver.receive(&decoded).unwrap();
        sender.sent(&record, ticket, ticket).unwrap();
        sender.acknowledge(ticket, true, ticket).unwrap();
    }
    assert!(sender.done());
    let received = receiver.take_completed().unwrap();
    assert_eq!(received, response);
    exchanges.started[0] = None; // accepted FileComplete for this delivery
    items::UpdateNotification::decode(
        &received.bytes,
        exchanges.profile.as_ref().unwrap().bindings(),
    )
    .unwrap()
}

#[tokio::test]
async fn purchase_wire_reply_contains_only_purchased_item_and_retry_preserves_cash() {
    let repo = Arc::new(MemoryRepository::default());
    let car = Definition {
        class: DefinitionClass::RaceVehicleItemData,
        buy_price: 100,
        sell_price: 50,
        quantity: 1,
        ownership_level: OwnershipLevel::Owned,
        flags: DefinitionFlags::default(),
        sub_items: vec![],
        additional_items: vec![],
        default_derived: Some(Derived::decode(items::Layout::RaceVehicle, &[0; 68]).unwrap()),
    };
    let part = Definition {
        class: DefinitionClass::AirFilterItemData,
        buy_price: 23,
        sell_price: 10,
        default_derived: Some(Derived::Empty),
        flags: DefinitionFlags {
            purchasable: true,
            requires_owner: true,
            ..Default::default()
        },
        ..car.clone()
    };
    let catalog =
        Arc::new(InventoryCatalog::new([([1; 16], car), ([2; 16], part)], vec![[1; 16]]).unwrap());
    let profile = Service::new(repo.clone(), catalog.clone())
        .with_purchase_offers(nfs_services::purchase::Offers::new([[2; 16]]).unwrap())
        .bind(account(19));
    profile
        .answer(request(2), Timestamp(1))
        .await
        .unwrap()
        .unwrap();
    repo.apply(
        account(19),
        &Batch {
            id: 77,
            expected_generation: 1,
            ops: vec![nfs_services::bank::Balance(30).store()],
        },
        Timestamp(2),
    )
    .unwrap();
    let purchase = items::PurchaseRequest {
        sequence: 3,
        owner: 1,
        selection: items::PurchaseSelection::Definitions(vec![[2; 16]]),
    };
    let bought = profile
        .answer(purchase.encode().unwrap(), Timestamp(3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bought.kind, ReplyKind::Purchase);
    let wire = items::PurchaseReply::decode(&bought.reply.bytes, catalog.bindings()).unwrap();
    assert_eq!(wire.sequence, 3);
    assert_eq!(wire.purchased.unwrap().roots, [2]);
    assert_eq!(bought.items.items.len(), 2);
    let before = repo.snapshot(account(19)).unwrap();
    assert_eq!(
        nfs_services::bank::Balance::from_snapshot(&before)
            .unwrap()
            .0,
        7
    );
    let retry = profile
        .answer(purchase.encode().unwrap(), Timestamp(4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.reply.bytes, bought.reply.bytes);
    assert_eq!(repo.snapshot(account(19)).unwrap(), before);
    let rejected = profile
        .answer(
            items::PurchaseRequest {
                owner: 77,
                ..purchase
            }
            .encode()
            .unwrap(),
            Timestamp(5),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rejected.kind, ReplyKind::Rejected);
    assert_eq!(rejected.reply.bytes, [3, 0, 0, 0, 3]);
    assert_eq!(repo.snapshot(account(19)).unwrap(), before);
}
fn deliver(
    listener: &mut PlayerListener,
    index: usize,
    record: Record,
    size: Option<usize>,
) -> bool {
    let mut bits = BitWriter::new();
    bits.put(1 << (index + 4), 6)
        .put_span(record.encode(size).unwrap().span())
        .align();
    listener.frame(Queued {
        advertised: None,
        frame: 0,
        data: bits.span(),
    })
}
fn transfer(listener: &mut PlayerListener, index: usize, name: &[u8], bytes: Vec<u8>) {
    let size = bytes.len();
    assert!(deliver(
        listener,
        index,
        Record::Start {
            name: name.to_vec(),
            bytes: size
        },
        None
    ));
    assert!(deliver(
        listener,
        index,
        Record::Blocks {
            first: 0,
            blocks: vec![bytes]
        },
        Some(size)
    ));
    assert!(deliver(listener, index, Record::Finish, Some(size)));
}

#[tokio::test]
async fn complete_file_generates_committed_reply_and_backpressure_preserves_next_request() {
    let repo = Arc::new(MemoryRepository::default());
    let catalog = catalog();
    let service = Service::new(repo.clone(), catalog.clone());
    let mut exchanges = Exchanges::new(Some(service.bind(account(1))), 7);
    let mut listener = PlayerListener::new(123);
    assert!(listener.files_alive(10));
    transfer(&mut listener, 0, b"Items", request(21));
    exchanges.start(&mut listener, 11, Timestamp(1));
    assert!(exchanges.working());
    assert!(exchanges.completed(&mut listener).await);
    let state = repo.snapshot(account(1)).unwrap();
    assert_eq!(state.generation, 1);
    let response = exchanges.pending[0].as_ref().unwrap();
    let parsed = Envelope::decode(&response.bytes).unwrap();
    assert_eq!(parsed.sequence, 21);
    assert_eq!(
        parsed.collection(catalog.bindings()).unwrap().items.len(),
        1
    );
    let mut sender =
        nfs_world::files::sender::Sender::new(response.clone(), Default::default(), 11).unwrap();
    let mut received = Receiver::default();
    for ticket in 0..16 {
        let Some(record) = sender.offer(12 + ticket).unwrap() else {
            break;
        };
        let wire = record.encode(sender.size()).unwrap();
        let (decoded, _) = nfs_world::files::decode(wire.span(), received.size()).unwrap();
        received.receive(&decoded).unwrap();
        sender.sent(&record, ticket, 12 + ticket).unwrap();
        sender.acknowledge(ticket, true, 12 + ticket).unwrap();
    }
    assert!(sender.done());
    assert_eq!(received.take_completed().unwrap(), *response);

    transfer(&mut listener, 0, b"Items", request(22));
    exchanges.start(&mut listener, 30, Timestamp(2));
    assert!(!exchanges.working());
    assert!(listener.files_alive(31));
    assert_eq!(listener.take_file(0).unwrap().bytes, request(22));
    assert_eq!(repo.snapshot(account(1)).unwrap().generation, 1);
}

#[tokio::test]
async fn profiles_reconnect_preserve_changes_and_unknown_requests_do_not_open_storage() {
    let repo = Arc::new(MemoryRepository::default());
    let service = Service::new(repo.clone(), catalog());
    let a = service.bind(account(1));
    let b = service.bind(account(2));
    a.answer(request(1), Timestamp(0)).await.unwrap().unwrap();
    b.answer(request(1), Timestamp(0)).await.unwrap().unwrap();
    let mut item = repo
        .snapshot(account(1))
        .unwrap()
        .items
        .into_values()
        .next()
        .unwrap();
    item.buy_price = 900;
    repo.apply(
        account(1),
        &Batch {
            id: 2,
            expected_generation: 1,
            ops: vec![Op::Update(item)],
        },
        Timestamp(1),
    )
    .unwrap();
    drop(a);
    let again = service
        .bind(account(1))
        .answer(request(2), Timestamp(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.reply.generation, 2);
    let parsed = Envelope::decode(&again.reply.bytes)
        .unwrap()
        .collection(service.catalog.bindings())
        .unwrap();
    assert_eq!(parsed.items[&1].buy_price, 900);
    assert_eq!(*again.items, parsed);
    assert_eq!(
        repo.snapshot(account(2))
            .unwrap()
            .items
            .values()
            .next()
            .unwrap()
            .buy_price,
        200
    );
    let c = service.bind(account(3));
    assert!(c.answer(vec![0; 5], Timestamp(0)).await.unwrap().is_none());
    assert!(c.answer(vec![0; 6], Timestamp(0)).await.is_err());
    assert_eq!(repo.snapshot(account(3)), Err(nfs_storage::Error::Absent));
    let mut exchanges = Exchanges::new(Some(c), 1);
    let mut listener = PlayerListener::new(999);
    transfer(&mut listener, 1, b"Unknown", request(1));
    exchanges.start(&mut listener, 0, Timestamp(0));
    assert!(!exchanges.working());
    assert_eq!(repo.snapshot(account(3)), Err(nfs_storage::Error::Absent));
}

#[test]
fn incoming_deadline_is_fixed_and_covers_completed_backpressure() {
    let mut a = PlayerListener::new(1);
    let mut b = PlayerListener::new(2);
    let start = Record::Start {
        name: b"Items".to_vec(),
        bytes: 5,
    };
    assert!(a.files_alive(100));
    assert!(deliver(&mut a, 0, start.clone(), None));
    assert!(a.files_alive(59_999));
    assert!(deliver(&mut a, 0, start, None));
    assert!(deliver(
        &mut a,
        0,
        Record::Blocks {
            first: 0,
            blocks: vec![request(1)]
        },
        Some(5)
    ));
    assert!(deliver(&mut a, 0, Record::Finish, Some(5)));
    assert!(a.files_alive(60_099));
    assert!(!a.files_alive(60_100));
    assert!(b.files_alive(60_100));
    transfer(&mut b, 1, b"Items", request(2));
    b.take_file(1).unwrap();
    assert!(b.files_alive(120_100));
    assert!(!b.files_alive(120_099));
}

struct Blocked {
    signal: tokio::sync::mpsc::UnboundedSender<std::thread::ThreadId>,
    release: std::sync::Mutex<bool>,
    ready: std::sync::Condvar,
}
impl InventoryRepository for Blocked {
    fn open(&self, _: AccountId) -> Result<nfs_storage::Snapshot, nfs_storage::Error> {
        self.signal.send(std::thread::current().id()).unwrap();
        let guard = self.release.lock().unwrap();
        let _ = self
            .ready
            .wait_timeout_while(guard, Duration::from_secs(2), |released| !*released)
            .unwrap();
        Err(nfs_storage::Error::Storage)
    }
    fn snapshot(&self, _: AccountId) -> Result<nfs_storage::Snapshot, nfs_storage::Error> {
        unreachable!()
    }
    fn apply(
        &self,
        _: AccountId,
        _: &Batch,
        _: Timestamp,
    ) -> Result<nfs_storage::Applied, nfs_storage::Error> {
        unreachable!()
    }
}

#[tokio::test]
async fn cancellation_keeps_running_workers_bounded_and_does_not_block_executor() {
    let executor = std::thread::current().id();
    let (signal, mut signals) = tokio::sync::mpsc::unbounded_channel();
    let repo = Arc::new(Blocked {
        signal,
        release: std::sync::Mutex::new(false),
        ready: std::sync::Condvar::new(),
    });
    let service = Service::new(repo.clone(), catalog());
    let mut jobs = JoinSet::new();
    for n in 1..=12 {
        let profile = service.bind(account(n));
        jobs.spawn(async move { profile.answer(request(1), Timestamp(0)).await });
    }
    for _ in 0..WORKERS {
        let thread = timeout(Duration::from_secs(1), signals.recv())
            .await
            .unwrap()
            .unwrap();
        assert_ne!(thread, executor);
    }
    jobs.shutdown().await;
    assert_eq!(service.workers.available_permits(), 0);
    assert!(signals.try_recv().is_err());
    *repo.release.lock().unwrap() = true;
    repo.ready.notify_all();
    timeout(Duration::from_secs(1), async {
        loop {
            if service.workers.available_permits() == WORKERS {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(signals.try_recv().is_err());
}

fn record(id: u64, state: u8) -> items::Item {
    items::Item {
        id,
        definition: [1; 16],
        owner: 0,
        defaults: vec![],
        children: vec![],
        state,
        buy_price: 215,
        sell_price: 139,
        derived: Derived::Empty,
    }
}

#[test]
fn credit_announces_each_new_record_then_each_changed_root() {
    let bindings = catalog();
    let bindings = bindings.bindings();
    let before = items::Collection {
        roots: vec![1, 2],
        items: [(1, record(1, items::OWNED)), (2, record(2, items::OWNED))].into(),
    };
    assert_eq!(
        credit_notifications(2, &before, &before, bindings),
        Ok((vec![], 0))
    );
    // A level-up: the controller (1) changes, two offers and a grant arrive.
    let mut after = before.clone();
    after.items.get_mut(&1).unwrap().buy_price = 1;
    for (id, state) in [
        (5, items::PURCHASABLE),
        (4, items::PURCHASABLE),
        (6, items::OWNED),
    ] {
        after.roots.push(id);
        after.items.insert(id, record(id, state));
    }
    let (sent, unannounced) = credit_notifications(2, &before, &after, bindings).unwrap();
    assert_eq!(unannounced, 0);
    assert_eq!(sent.len(), 4);
    for (bytes, id) in sent[..3].iter().zip([5, 4, 6]) {
        let change = items::ChangeNotification::decode(bytes, bindings).unwrap();
        assert_eq!(change.player_index, 2);
        assert_eq!(change.added, after.for_roots(vec![id]).unwrap());
        assert_eq!(change.removed, items::Collection::default());
    }
    let update = items::UpdateNotification::decode(&sent[3], bindings).unwrap();
    assert_eq!(update.items, after.for_roots(vec![1]).unwrap());
    // Many grants share a bounded number of notifications; removals are
    // never described.
    let mut many = before.clone();
    many.roots.retain(|&id| id != 2);
    many.items.remove(&2);
    for id in 10..30 {
        many.roots.push(id);
        many.items.insert(id, record(id, items::PURCHASABLE));
    }
    let (sent, unannounced) = credit_notifications(2, &before, &many, bindings).unwrap();
    assert_eq!(unannounced, 1);
    assert_eq!(sent.len(), 10);
    let added: Vec<_> = sent
        .iter()
        .flat_map(|bytes| {
            items::ChangeNotification::decode(bytes, bindings)
                .unwrap()
                .added
                .roots
        })
        .collect();
    assert_eq!(added, (10..30).collect::<Vec<_>>());
}

#[tokio::test]
async fn queued_notifications_go_out_one_at_a_time_ahead_of_requests() {
    let repo = Arc::new(MemoryRepository::default());
    let service = Service::new(repo, catalog());
    let mut exchanges = Exchanges::new(Some(service.bind(account(4))), 7);
    let mut listener = PlayerListener::new(999);
    for bytes in [vec![1], vec![2]] {
        exchanges.notifications.push_back(Completed {
            name: b"Items".to_vec(),
            bytes,
        });
    }
    transfer(&mut listener, 0, b"Items", request(1));
    // A request waits until every queued notification went out.
    exchanges.start(&mut listener, 0, Timestamp(0));
    assert!(!exchanges.working());
    exchanges.promote();
    assert_eq!(exchanges.pending[0].as_ref().unwrap().bytes, [1]);
    exchanges.promote();
    assert_eq!(exchanges.pending[0].as_ref().unwrap().bytes, [1]);
    // Queued on the wire: the next waits for its FileComplete.
    exchanges.pending[0] = None;
    exchanges.started[0] = Some(0);
    exchanges.promote();
    assert!(exchanges.pending[0].is_none());
    exchanges.started[0] = None;
    exchanges.promote();
    assert_eq!(exchanges.pending[0].take().unwrap().bytes, [2]);
    exchanges.start(&mut listener, 0, Timestamp(0));
    assert!(exchanges.working());
}
