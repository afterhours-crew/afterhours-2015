// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::{bank::Balance, persistent::key};
use nfs_storage::{
    AccountId, Error, InventoryRepository, MemoryRepository, SqliteRepository, Timestamp,
};
use nfs_world_core::items::{
    self, Definition, DefinitionClass, DefinitionFlags, Derived, OwnershipLevel,
};

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn credit(total: i32) -> Scores {
    Scores {
        total,
        build: 0,
        crew: 0,
        outlaw: 0,
        speed: total,
        style: 0,
    }
}
struct Fixture {
    tables: persistent::Catalog,
    inventory: InventoryCatalog,
    thresholds: Thresholds,
    categories: category_unlocks::Policy,
    items: item_unlocks::Policy,
}
impl Fixture {
    fn new() -> Self {
        let mut data = persistent::tests::content();
        let table = data["tables"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t["name"] == "RepValuesTable")
            .unwrap();
        table["columns"] = serde_json::json!(["RepScore", "RepLevel", "BuildScore", "CrewScore", "OutlawScore", "SpeedScore", "StyleScore"].map(|name| serde_json::json!({"name":name,"key":key(name),"value_type":"int","default":"0"})));
        let controller = Definition {
            class: DefinitionClass::CategoryUnlockControllerItemData,
            buy_price: 0,
            sell_price: 0,
            quantity: 1,
            ownership_level: OwnershipLevel::Owned,
            flags: DefinitionFlags::default(),
            sub_items: vec![],
            additional_items: vec![],
            default_derived: Some(Derived::CategoryUnlockController {
                unlock_mask: 0x8000_0001,
            }),
        };
        let part = Definition {
            class: DefinitionClass::ForcedInductionItemData,
            buy_price: 19,
            sell_price: 9,
            ownership_level: OwnershipLevel::Claimable,
            flags: DefinitionFlags {
                requires_owner: true,
                ..Default::default()
            },
            default_derived: Some(Derived::Empty),
            ..controller.clone()
        };
        let inventory = InventoryCatalog::new(
            [
                ([1; 16], controller),
                ([2; 16], part.clone()),
                ([3; 16], part),
            ],
            vec![[1; 16]],
        )
        .unwrap();
        Self {
            tables: persistent::Catalog::from_json(&data).unwrap(),
            thresholds: Thresholds::new(vec![0, 10, 25]).unwrap(),
            categories: category_unlocks::Policy::new(&inventory, [1; 16], vec![(2, 10), (4, 25)])
                .unwrap(),
            items: item_unlocks::Policy::new(&inventory, [([2; 16], 10), ([3; 16], 25)]).unwrap(),
            inventory,
        }
    }
    fn policy(&self) -> Policy<'_> {
        Policy {
            tables: &self.tables,
            inventory: &self.inventory,
            thresholds: &self.thresholds,
            categories: Some(&self.categories),
            items: Some(&self.items),
        }
    }
    fn initialize(&self, repo: &dyn InventoryRepository, who: AccountId) -> Snapshot {
        let snapshot = inventory::load(repo, who, &self.inventory, 1, Timestamp(1)).unwrap();
        self.tables
            .ensure_loaded(repo, who, snapshot, Timestamp(2))
            .unwrap()
            .0
    }
    fn scores(&self, snapshot: &Snapshot) -> Scores {
        Scores::load(&self.tables.view(snapshot).unwrap()).unwrap()
    }
}
fn level(snapshot: &Snapshot) -> i32 {
    let Value::Int(value) = snapshot.tables[&key("RepValuesTable")].rows[&0][&key("RepLevel")]
    else {
        panic!()
    };
    value
}
fn mask(snapshot: &Snapshot, catalog: &InventoryCatalog) -> u32 {
    let collection = inventory::collection(snapshot, catalog.bindings()).unwrap();
    let Derived::CategoryUnlockController { unlock_mask } = collection.items[&1].derived else {
        panic!()
    };
    unlock_mask
}

#[test]
fn threshold_credit_grants_are_one_atomic_generation_and_account_isolated() {
    let fixture = Fixture::new();
    let repo = MemoryRepository::default();
    let other = fixture.initialize(&repo, account(2));
    let initial = fixture.initialize(&repo, account(1));
    let mut before = initial.clone();
    for (id, delta, total, expected_level, expected_mask, count) in [
        (11, 9, 9, 1, 0x8000_0001, 1),
        (12, 1, 10, 2, 0x8000_0003, 2),
        (13, 40, 50, 3, 0x8000_0007, 3),
        (14, 1, 51, 3, 0x8000_0007, 3),
    ] {
        let batch = fixture
            .policy()
            .prepare(&before, credit(delta), id)
            .unwrap();
        assert_eq!(
            repo.snapshot(account(1)).unwrap(),
            before,
            "planning must not write"
        );
        let applied = repo.apply(account(1), &batch, Timestamp(id)).unwrap();
        assert!(!applied.replayed);
        let after = repo.snapshot(account(1)).unwrap();
        assert_eq!(after.generation, before.generation + 1);
        assert_eq!(fixture.scores(&after), credit(total));
        assert_eq!(level(&after), expected_level);
        assert_eq!(mask(&after, &fixture.inventory), expected_mask);
        assert_eq!(after.items.len(), count);
        assert_eq!(
            Balance::from_snapshot(&after).unwrap(),
            Balance::from_snapshot(&initial).unwrap()
        );
        assert_eq!(after.garage, initial.garage);
        for (key, table) in &initial.tables {
            if *key != persistent::key("RepValuesTable") {
                assert_eq!(&after.tables[key], table);
            }
        }
        assert_eq!(repo.snapshot(account(2)).unwrap(), other);
        assert!(
            repo.apply(account(1), &batch, Timestamp(id + 1))
                .unwrap()
                .replayed
        );
        assert_eq!(repo.snapshot(account(1)).unwrap(), after);
        before = after;
    }
    for item in before.items.values().filter(|i| i.definition.0 != [1; 16]) {
        assert_eq!(item.state, items::PURCHASABLE);
        assert_eq!(item.owner, None);
        assert_eq!(item.buy_price, 19, "an unlock is not a free purchase");
    }
}

#[test]
fn competing_credits_and_invalid_reward_roll_back_rep_and_all_grants() {
    let fixture = Fixture::new();
    let repo = MemoryRepository::default();
    let before = fixture.initialize(&repo, account(1));
    let first = fixture.policy().prepare(&before, credit(10), 31).unwrap();
    let second = fixture.policy().prepare(&before, credit(25), 32).unwrap();
    let mut invalid_reward = first.clone();
    invalid_reward
        .ops
        .push(Op::Remove(nfs_storage::ItemId::new(999).unwrap()));
    assert_eq!(
        repo.apply(account(1), &invalid_reward, Timestamp(3)),
        Err(Error::UnknownItem)
    );
    assert_eq!(repo.snapshot(account(1)).unwrap(), before);
    // Failed writes do not consume the retry ID.
    repo.apply(account(1), &first, Timestamp(4)).unwrap();
    let saved = repo.snapshot(account(1)).unwrap();
    assert_eq!(
        repo.apply(account(1), &second, Timestamp(5)),
        Err(Error::Conflict)
    );
    assert_eq!(repo.snapshot(account(1)).unwrap(), saved);
    // Even after the bounded replay history expires, retaining the original
    // generation prevents an old credit from being applied again.
    let mut generation = saved.generation;
    for n in 0..nfs_storage::MAX_BATCH_HISTORY {
        generation = repo
            .apply(
                account(1),
                &Batch {
                    id: 1000 + n as u64,
                    expected_generation: generation,
                    ops: vec![Balance(1).store()],
                },
                Timestamp(6),
            )
            .unwrap()
            .generation;
    }
    assert_eq!(
        repo.apply(account(1), &first, Timestamp(7)),
        Err(Error::Conflict)
    );
    assert_eq!(
        fixture.scores(&repo.snapshot(account(1)).unwrap()),
        credit(10)
    );
}

#[test]
fn rejects_negative_empty_overflow_and_invalid_unlock_state_without_mutation() {
    let fixture = Fixture::new();
    let repo = MemoryRepository::default();
    let before = fixture.initialize(&repo, account(1));
    for delta in [
        credit(0),
        credit(-1),
        Scores {
            style: -1,
            ..credit(1)
        },
    ] {
        assert!(fixture.policy().prepare(&before, delta, 41).is_err());
    }
    assert!(fixture.policy().prepare(&before, credit(1), 0).is_err());
    let mut bad = before.clone();
    bad.items.clear();
    assert!(fixture.policy().prepare(&bad, credit(25), 41).is_err());
    for name in [
        "RepScore",
        "BuildScore",
        "CrewScore",
        "OutlawScore",
        "SpeedScore",
        "StyleScore",
    ] {
        let mut high = before.clone();
        high.tables
            .get_mut(&key("RepValuesTable"))
            .unwrap()
            .rows
            .get_mut(&0)
            .unwrap()
            .insert(key(name), Value::Int(i32::MAX));
        assert!(
            fixture
                .policy()
                .prepare(
                    &high,
                    Scores {
                        total: 1,
                        build: 1,
                        crew: 1,
                        outlaw: 1,
                        speed: 1,
                        style: 1
                    },
                    41
                )
                .is_err()
        );
    }
    assert_eq!(repo.snapshot(account(1)).unwrap(), before);
}

#[test]
fn sqlite_restart_retains_scores_level_unlocks_and_retry_result() {
    let fixture = Fixture::new();
    let path = std::env::temp_dir().join(format!(
        "afterhours-earned-rep-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let (saved, batch) = {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let before = fixture.initialize(&repo, account(1));
        let mixed = Scores {
            total: 26,
            build: 3,
            crew: 4,
            outlaw: 5,
            speed: 6,
            style: 8,
        };
        let batch = fixture.policy().prepare(&before, mixed, 51).unwrap();
        repo.apply(account(1), &batch, Timestamp(3)).unwrap();
        let saved = repo.snapshot(account(1)).unwrap();
        assert_eq!(fixture.scores(&saved), mixed);
        assert_eq!(level(&saved), 3);
        assert_eq!(saved.items.len(), 3);
        (saved, batch)
    };
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        assert!(
            repo.apply(account(1), &batch, Timestamp(4))
                .unwrap()
                .replayed
        );
        assert_eq!(repo.snapshot(account(1)).unwrap(), saved);
        let request = items::Request {
            sequence: 9,
            operation: items::LOAD_INVENTORY,
        }
        .encode();
        let reply = inventory::answer_load_with_unlocks(
            &request,
            &repo,
            account(1),
            &fixture.inventory,
            1,
            Timestamp(5),
            Some(&fixture.tables),
            Some(&fixture.categories),
            Some(&fixture.items),
        )
        .unwrap()
        .unwrap();
        assert_eq!(reply.generation, saved.generation);
        assert_eq!(repo.snapshot(account(1)).unwrap(), saved);
        assert_eq!(
            items::Envelope::decode(&reply.bytes)
                .unwrap()
                .collection(fixture.inventory.bindings())
                .unwrap(),
            inventory::collection(&saved, fixture.inventory.bindings()).unwrap()
        );
    }
    std::fs::remove_dir_all(path).unwrap();
}
