// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::{persistent::key, reputation::Thresholds};
use nfs_storage::{AccountId, InventoryRepository, MemoryRepository, SqliteRepository, Timestamp};
use nfs_world_core::items::{
    Definition, DefinitionClass, DefinitionFlags, Derived, InventoryCatalog, OwnershipLevel,
};
mod story;

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn scores(total: i32) -> reputation::Scores {
    reputation::Scores {
        total,
        build: 0,
        crew: 0,
        outlaw: 0,
        speed: 0,
        style: 0,
    }
}
fn completion(batch_id: u64, position: u32) -> Completion {
    Completion {
        event_id: 1_761_118_161,
        kind: "SpeedWall_SprintRace".into(),
        position,
        cash: 2000,
        rep: scores(9000),
        batch_id,
        speedwall: None,
    }
}

struct Fixture {
    tables: persistent::Catalog,
    inventory: InventoryCatalog,
    thresholds: Thresholds,
}
impl Fixture {
    fn new() -> Self {
        let mut data = persistent::tests::content();
        for (name, columns) in [
            ("ProgressionObjective", &["Active", "Completed"][..]),
            (
                "RepValuesTable",
                &[
                    "RepScore",
                    "RepLevel",
                    "BuildScore",
                    "CrewScore",
                    "OutlawScore",
                    "SpeedScore",
                    "StyleScore",
                ][..],
            ),
            (
                STATS_TABLE,
                &[
                    "LargestFineEscaped",
                    "LargestFineBusted",
                    "CashEarned",
                    "DistanceDriven",
                    "DistanceDrifted",
                    "FavoriteVehicleId",
                    "TotalTimePlayed",
                    "TopSpeed",
                    "Spare",
                ][..],
            ),
        ] {
            let table = data["tables"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|t| t["name"] == name)
                .unwrap();
            table["columns"] = serde_json::json!(columns.iter().map(|c| serde_json::json!({"name":c,"key":key(c),"value_type":"int","default":"0"})).collect::<Vec<_>>());
        }
        let stats = data["tables"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t["name"] == "ProgressionStats")
            .unwrap();
        stats["columns"][0] = serde_json::json!({"name":"IntroCompleted","key":key("IntroCompleted"),"value_type":"int","default":"0"});
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
        let inventory = InventoryCatalog::new([([1; 16], controller)], vec![[1; 16]]).unwrap();
        Self {
            tables: persistent::Catalog::from_json(&data).unwrap(),
            thresholds: Thresholds::new(vec![0, 5000, 25_000]).unwrap(),
            inventory,
        }
    }
    fn policy(&self) -> reputation::earned::Policy<'_> {
        reputation::earned::Policy {
            tables: &self.tables,
            inventory: &self.inventory,
            thresholds: &self.thresholds,
            categories: None,
            items: None,
            notifications: None,
        }
    }
    fn initialize(&self, repo: &dyn InventoryRepository, who: AccountId) -> Snapshot {
        let snapshot = inventory::load(repo, who, &self.inventory, 1, Timestamp(1)).unwrap();
        let snapshot = self
            .tables
            .ensure_loaded(repo, who, snapshot, Timestamp(2))
            .unwrap()
            .0;
        awards::ensure_empty(repo, who, snapshot, Timestamp(3)).unwrap()
    }
    fn earned(&self, snapshot: &Snapshot) -> i32 {
        self.tables.view(snapshot).unwrap().stores()[&(key(STATS_TABLE), key(CASH_EARNED))]
    }
    fn rep(&self, snapshot: &Snapshot) -> reputation::Scores {
        reputation::Scores::load(&self.tables.view(snapshot).unwrap()).unwrap()
    }
}
fn level(snapshot: &Snapshot) -> i32 {
    let Value::Int(value) = snapshot.tables[&key("RepValuesTable")].rows[&0][&key("RepLevel")]
    else {
        panic!("level column")
    };
    value
}

#[test]
fn one_completion_pays_cash_rep_statistic_and_record_once() {
    let fixture = Fixture::new();
    let repo = MemoryRepository::default();
    let who = account(1);
    let other = account(2);
    let initial = fixture.initialize(&repo, who);
    let untouched = fixture.initialize(&repo, other);
    let batch = prepare(&fixture.policy(), &initial, &completion(41, 0)).unwrap();
    assert_eq!(batch.expected_generation, initial.generation);
    assert!(!repo.apply(who, &batch, Timestamp(10)).unwrap().replayed);
    // A retried completion is answered from history: nothing is paid twice.
    assert!(repo.apply(who, &batch, Timestamp(11)).unwrap().replayed);
    let after = repo.snapshot(who).unwrap();
    assert_eq!(Balance::from_snapshot(&after), Ok(Balance(2000)));
    assert_eq!(fixture.earned(&after), 2000);
    assert_eq!(fixture.rep(&after), scores(9000));
    assert_eq!(
        level(&after),
        fixture
            .thresholds
            .evaluate(scores(9000))
            .get(reputation::Field::Level)
    );
    assert!(level(&after) > level(&initial));
    let state = awards::State::load(&after).unwrap();
    assert_eq!(
        state.events,
        vec![(
            1_761_118_161,
            awards::Event {
                attempts: 1,
                position: 0,
                record: awards::Record {
                    name: "1761118161".into(),
                    screenshot: 0,
                    modified: 0,
                },
                kind: "SpeedWall_SprintRace".into(),
            }
        )]
    );
    // Another account is isolated, and a stale snapshot conflicts.
    assert_eq!(repo.snapshot(other).unwrap(), untouched);
    let stale = prepare(&fixture.policy(), &initial, &completion(42, 0)).unwrap();
    assert_eq!(
        repo.apply(who, &stale, Timestamp(12)),
        Err(nfs_storage::Error::Conflict)
    );
}

#[test]
fn repeated_completions_count_attempts_and_keep_the_best_position() {
    let fixture = Fixture::new();
    let repo = MemoryRepository::default();
    let who = account(3);
    let mut snapshot = fixture.initialize(&repo, who);
    for (id, position) in [(51, 2), (52, 0), (53, 1)] {
        let batch = prepare(&fixture.policy(), &snapshot, &completion(id, position)).unwrap();
        repo.apply(who, &batch, Timestamp(20 + id)).unwrap();
        snapshot = repo.snapshot(who).unwrap();
    }
    assert_eq!(Balance::from_snapshot(&snapshot), Ok(Balance(6000)));
    assert_eq!(fixture.earned(&snapshot), 6000);
    assert_eq!(fixture.rep(&snapshot).total, 27_000);
    let state = awards::State::load(&snapshot).unwrap();
    let [(id, event)] = state.events.as_slice() else {
        panic!("one race-event record")
    };
    assert_eq!((*id, event.attempts, event.position), (1_761_118_161, 3, 0));
}

#[test]
fn sprint_speedwall_shares_reward_retry_key_without_crediting_driving_rep_twice() {
    let fixture = Fixture::new();
    let repo = MemoryRepository::default();
    let who = account(9);
    let initial = fixture.initialize(&repo, who);
    let mut finish = completion(101, 1);
    finish.speedwall = Some(crate::speedwall::Sprint {
        time_ms: 123_000,
        rep: 13_000,
        vehicle: "ID_CAR_TEST".into(),
    });
    let batch = prepare(&fixture.policy(), &initial, &finish).unwrap();
    repo.apply(who, &batch, Timestamp(10)).unwrap();
    assert!(repo.apply(who, &batch, Timestamp(11)).unwrap().replayed);
    let after = repo.snapshot(who).unwrap();
    assert_eq!(fixture.rep(&after), scores(9000));
    assert_eq!(Balance::from_snapshot(&after), Ok(Balance(2000)));
    assert_eq!(awards::State::load(&after).unwrap().events[0].1.attempts, 1);
    let current = crate::speedwall::Current::from_snapshot(who, &after).unwrap();
    let state = crate::speedwall::State::new(who, vec![])
        .unwrap()
        .with_current(&current)
        .unwrap();
    assert_eq!(state.rows()[0].integers, vec![("Attempts".into(), 1)]);
    assert_eq!(state.rows()[0].floats[0].1, 13_000f32.to_bits());
    assert_eq!(state.rows()[0].floats[1].1, 123_000f32.to_bits());
    let mut invalid = completion(102, 0);
    invalid.speedwall = Some(crate::speedwall::Sprint {
        time_ms: 1,
        rep: -1,
        vehicle: "BAD".into(),
    });
    assert!(prepare(&fixture.policy(), &after, &invalid).is_err());
    assert_eq!(repo.snapshot(who).unwrap(), after);
}

#[test]
fn invalid_or_overflowing_rewards_change_nothing() {
    let fixture = Fixture::new();
    let repo = MemoryRepository::default();
    let who = account(4);
    let initial = fixture.initialize(&repo, who);
    let mut negative = completion(61, 0);
    negative.rep.speed = -1;
    assert!(prepare(&fixture.policy(), &initial, &negative).is_err());
    let mut unnamed = completion(62, 0);
    unnamed.kind.clear();
    assert!(prepare(&fixture.policy(), &initial, &unnamed).is_err());
    let mut huge = completion(63, 0);
    huge.cash = u32::MAX;
    let rich = Balance(1).store();
    let batch = nfs_storage::Batch {
        id: 64,
        expected_generation: initial.generation,
        ops: vec![rich],
    };
    repo.apply(who, &batch, Timestamp(30)).unwrap();
    let funded = repo.snapshot(who).unwrap();
    assert!(prepare(&fixture.policy(), &funded, &huge).is_err());
    // A record of another kind under the same ID is refused, not overwritten.
    let first = prepare(&fixture.policy(), &funded, &completion(65, 0)).unwrap();
    repo.apply(who, &first, Timestamp(31)).unwrap();
    let recorded = repo.snapshot(who).unwrap();
    let mut other_kind = completion(66, 0);
    other_kind.kind = "SpeedWall_TimeTrial".into();
    assert!(prepare(&fixture.policy(), &recorded, &other_kind).is_err());
    // Zero REP still pays the cash and record.
    let mut cash_only = completion(67, 0);
    cash_only.rep = scores(0);
    let batch = prepare(&fixture.policy(), &recorded, &cash_only).unwrap();
    repo.apply(who, &batch, Timestamp(32)).unwrap();
    let after = repo.snapshot(who).unwrap();
    assert_eq!(fixture.rep(&after), fixture.rep(&recorded));
    assert_eq!(Balance::from_snapshot(&after), Ok(Balance(4001)));
}

#[test]
fn rewards_and_record_survive_a_restart() {
    let fixture = Fixture::new();
    let path = std::env::temp_dir().join(format!(
        "nfs-event-rewards-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let who = account(5);
    let expected;
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        let initial = fixture.initialize(&repo, who);
        let batch = prepare(&fixture.policy(), &initial, &completion(71, 0)).unwrap();
        repo.apply(who, &batch, Timestamp(40)).unwrap();
        expected = repo.snapshot(who).unwrap();
    }
    {
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        repo.open(who).unwrap();
        let reopened = repo.snapshot(who).unwrap();
        assert_eq!(reopened, expected);
        assert_eq!(Balance::from_snapshot(&reopened), Ok(Balance(2000)));
        assert_eq!(fixture.earned(&reopened), 2000);
        assert_eq!(awards::State::load(&reopened).unwrap().events.len(), 1);
        // The same completion retried after the restart is still paid once.
        let batch = prepare(&fixture.policy(), &expected, &completion(71, 0)).unwrap();
        assert!(repo.apply(who, &batch, Timestamp(41)).unwrap().replayed);
        assert_eq!(repo.snapshot(who).unwrap(), expected);
    }
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn retained_result_receipt_is_read_before_recalculating_a_full_balance() {
    let f = Fixture::new();
    let repo = MemoryRepository::new();
    let who = account(31);
    f.initialize(&repo, who);
    commit(
        &repo,
        who,
        &f.policy(),
        &completion(91, 0),
        None,
        Timestamp(10),
    )
    .unwrap();
    let before = repo.snapshot(who).unwrap();
    repo.apply(
        who,
        &Batch {
            id: 92,
            expected_generation: before.generation,
            ops: vec![Balance(u32::MAX).store()],
        },
        Timestamp(11),
    )
    .unwrap();
    let full = repo.snapshot(who).unwrap();
    // Rebuilding the already-applied reward would overflow, despite this
    // being an exact retry which should never calculate another payment.
    assert!(prepare(&f.policy(), &full, &completion(91, 0)).is_err());
    assert!(
        commit(
            &repo,
            who,
            &f.policy(),
            &completion(91, 0),
            None,
            Timestamp(12)
        )
        .unwrap()
        .replayed
    );
    assert_eq!(repo.snapshot(who).unwrap(), full);
    assert!(
        commit(
            &repo,
            who,
            &f.policy(),
            &completion(93, 0),
            None,
            Timestamp(12)
        )
        .is_err()
    );
    assert_eq!(repo.snapshot(who).unwrap(), full);
}
