// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Field, Scores, Thresholds};
use crate::persistent::{Catalog, key};
use nfs_storage::{
    AccountId, Batch, Error, InventoryRepository, Op, Snapshot, Timestamp, tables::Value,
};

/// Explicit local test-profile adjustment with a generation guard. Call only
/// with the account's game and server stopped. This is not an earned reward:
/// the total score and derived level change, while the five driving-style
/// scores, inventory, money and all other tables remain intact.
pub fn set_for_operator(
    repository: &dyn InventoryRepository,
    account: AccountId,
    catalog: &Catalog,
    thresholds: &Thresholds,
    expected: u64,
    level: u32,
    now: Timestamp,
) -> Result<Snapshot, Error> {
    const NAMESPACE: u64 = 1 << 59;
    if level == 0 {
        return Err(Error::Config);
    }
    let score = thresholds
        .score_for_level(level)
        .map_err(|_| Error::Config)?;
    let mut snapshot = repository.snapshot(account)?;
    let loaded = catalog.view(&snapshot)?;
    Scores::load(&loaded).map_err(|_| Error::Config)?;
    if snapshot.generation != expected || expected >= NAMESPACE {
        return Err(Error::Conflict);
    }
    let mut table = snapshot
        .tables
        .get(&key("RepValuesTable"))
        .ok_or(Error::Config)?
        .clone();
    let row = table.rows.entry(0).or_default();
    row.insert(key("RepScore"), Value::Int(score));
    row.insert(key("RepLevel"), Value::Int(level as i32));
    snapshot.tables.insert(key("RepValuesTable"), table.clone());
    let check = catalog.view(&snapshot)?;
    if thresholds
        .evaluate(Scores::load(&check).map_err(|_| Error::Config)?)
        .get(Field::Level)
        != level as i32
    {
        return Err(Error::Config);
    }
    let applied = repository.apply(
        account,
        &Batch {
            id: NAMESPACE | expected,
            expected_generation: expected,
            ops: vec![Op::SetTable(key("RepValuesTable"), table)],
        },
        now,
    )?;
    // A concurrent operator adjustment at this generation must not be reported
    // as this operation's success, even if its batch id already exists.
    if applied.replayed {
        return Err(Error::Conflict);
    }
    snapshot.generation = applied.generation;
    snapshot.updated_at = now;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_storage::{GarageSlots, MemoryRepository, SqliteRepository};
    fn catalog() -> Catalog {
        let mut value = crate::persistent::tests::content();
        let rep = value["tables"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|v| v["name"] == "RepValuesTable")
            .unwrap();
        rep["columns"] = serde_json::json!(["RepScore", "RepLevel", "BuildScore", "CrewScore", "OutlawScore", "SpeedScore", "StyleScore"].map(|name| serde_json::json!({"name":name,"key":key(name),"value_type":"int","default":"7"})));
        Catalog::from_json(&value).unwrap()
    }
    fn initialize(
        repo: &dyn InventoryRepository,
        account: AccountId,
        catalog: &Catalog,
    ) -> Snapshot {
        repo.open(account).unwrap();
        repo.apply(
            account,
            &Batch {
                id: 1,
                expected_generation: 0,
                ops: vec![
                    Op::SetGarage(GarageSlots::default()),
                    crate::bank::Balance(500).store(),
                ],
            },
            Timestamp(1),
        )
        .unwrap();
        catalog
            .ensure_loaded(repo, account, repo.snapshot(account).unwrap(), Timestamp(2))
            .unwrap()
            .0
    }
    #[test]
    fn level_adjustment_is_atomic_preserves_styles_and_rejects_stale_or_invalid_input() {
        let repo = MemoryRepository::default();
        let catalog = catalog();
        let thresholds = Thresholds::new(vec![0, 10, 25]).unwrap();
        let account = AccountId::from_owned_config([1; 16]).unwrap();
        let other = AccountId::from_owned_config([2; 16]).unwrap();
        let before = initialize(&repo, account, &catalog);
        let other_before = initialize(&repo, other, &catalog);
        for level in [0, 4, u32::MAX] {
            assert_eq!(
                set_for_operator(
                    &repo,
                    account,
                    &catalog,
                    &thresholds,
                    before.generation,
                    level,
                    Timestamp(3)
                ),
                Err(Error::Config)
            );
            assert_eq!(repo.snapshot(account).unwrap(), before);
        }
        let saved = set_for_operator(
            &repo,
            account,
            &catalog,
            &thresholds,
            before.generation,
            3,
            Timestamp(3),
        )
        .unwrap();
        let scores = Scores::load(&catalog.view(&saved).unwrap()).unwrap();
        assert_eq!(
            scores,
            Scores {
                total: 25,
                build: 7,
                crew: 7,
                outlaw: 7,
                speed: 7,
                style: 7
            }
        );
        let mut expected = before.clone();
        expected.tables.insert(
            key("RepValuesTable"),
            saved.tables[&key("RepValuesTable")].clone(),
        );
        expected.generation += 1;
        expected.updated_at = Timestamp(3);
        assert_eq!(saved, expected);
        assert_eq!(repo.snapshot(other).unwrap(), other_before);
        assert_eq!(
            set_for_operator(
                &repo,
                account,
                &catalog,
                &thresholds,
                before.generation,
                1,
                Timestamp(4)
            ),
            Err(Error::Conflict)
        );
        assert_eq!(repo.snapshot(account).unwrap(), saved);
        let reset = set_for_operator(
            &repo,
            account,
            &catalog,
            &thresholds,
            saved.generation,
            1,
            Timestamp(5),
        )
        .unwrap();
        assert_eq!(
            Scores::load(&catalog.view(&reset).unwrap()).unwrap().total,
            0
        );
    }
    #[test]
    fn level_and_score_survive_database_reopen() {
        let path = std::env::temp_dir().join(format!(
            "afterhours-rep-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let catalog = catalog();
        let thresholds = Thresholds::new(vec![0, 10, 25]).unwrap();
        let account = AccountId::from_owned_config([1; 16]).unwrap();
        let saved = {
            let repo = SqliteRepository::open_owned_directory(&path).unwrap();
            let before = initialize(&repo, account, &catalog);
            set_for_operator(
                &repo,
                account,
                &catalog,
                &thresholds,
                before.generation,
                2,
                Timestamp(3),
            )
            .unwrap()
        };
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        assert_eq!(repo.snapshot(account).unwrap(), saved);
        let values = thresholds.evaluate(Scores::load(&catalog.view(&saved).unwrap()).unwrap());
        assert_eq!(values.get(Field::Level), 2);
        assert_eq!(values.get(Field::Total), 10);
        drop(repo);
        std::fs::remove_dir_all(path).unwrap();
    }
}
