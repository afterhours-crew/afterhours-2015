// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! REP category grants are additive inventory state, separate from the HUD level.
use crate::{inventory, persistent::key};
use nfs_storage::{AccountId, Batch, InventoryRepository, Op, Snapshot, Timestamp, tables::Value};
use nfs_world_core::items::{self, DefinitionClass, Derived, Guid, InventoryCatalog};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    pub(crate) controller: Guid,
    pub(crate) requirements: Vec<(u32, i32)>,
}

impl Policy {
    pub fn new(
        catalog: &InventoryCatalog,
        controller: Guid,
        mut requirements: Vec<(u32, i32)>,
    ) -> Result<Self, nfs_storage::Error> {
        let definition = catalog
            .definition(&controller)
            .map_err(|_| nfs_storage::Error::Config)?;
        if definition.class != DefinitionClass::CategoryUnlockControllerItemData
            || definition.flags.requires_owner
            || definition.quantity != 1
            || requirements.is_empty()
            || requirements.len() > 32
        {
            return Err(nfs_storage::Error::Config);
        }
        let mut seen = 0;
        for &(mask, score) in &requirements {
            if !mask.is_power_of_two() || seen & mask != 0 || score < 0 {
                return Err(nfs_storage::Error::Config);
            }
            seen |= mask;
        }
        requirements.sort_unstable();
        Ok(Self {
            controller,
            requirements,
        })
    }

    /// Reconcile only the controller's derived mask. Never revoke previously
    /// unlocked categories, award items, or change money/REP/vehicle equipment.
    pub fn ensure(
        &self,
        repository: &dyn InventoryRepository,
        account: AccountId,
        catalog: &InventoryCatalog,
        mut snapshot: Snapshot,
        now: Timestamp,
    ) -> Result<Snapshot, inventory::Error> {
        const NAMESPACE: u64 = 1 << 58;
        for _ in 0..4 {
            let score = match snapshot
                .tables
                .get(&key("RepValuesTable"))
                .and_then(|t| t.rows.get(&0))
                .and_then(|r| r.get(&key("RepScore")))
            {
                Some(Value::Int(score)) => *score,
                _ => return Err(nfs_storage::Error::Config.into()),
            };
            let grants = self
                .requirements
                .iter()
                .filter(|(_, minimum)| score >= *minimum)
                .fold(0, |m, (bit, _)| m | bit);
            let collection = inventory::collection(&snapshot, catalog.bindings())?;
            let mut controllers = collection
                .items
                .values()
                .filter(|i| i.definition == self.controller);
            let item = controllers.next().ok_or(nfs_storage::Error::Config)?;
            if controllers.next().is_some()
                || item.owner != 0
                || item.state != items::OWNED
                || !item.children.is_empty()
            {
                return Err(nfs_storage::Error::Config.into());
            }
            let Derived::CategoryUnlockController { unlock_mask } = item.derived else {
                return Err(nfs_storage::Error::Config.into());
            };
            if unlock_mask & grants == grants {
                return Ok(snapshot);
            }
            if snapshot.generation >= NAMESPACE {
                return Err(nfs_storage::Error::Conflict.into());
            }
            let mut record = snapshot.items
                [&nfs_storage::ItemId::new(item.id).ok_or(nfs_storage::Error::Invalid)?]
                .clone();
            record.derived = Derived::CategoryUnlockController {
                unlock_mask: unlock_mask | grants,
            }
            .encode()?;
            match repository.apply(
                account,
                &Batch {
                    id: NAMESPACE | snapshot.generation,
                    expected_generation: snapshot.generation,
                    ops: vec![Op::Update(record)],
                },
                now,
            ) {
                Ok(_) | Err(nfs_storage::Error::Conflict) => {
                    snapshot = repository.snapshot(account)?
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(nfs_storage::Error::Conflict.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_storage::{MemoryRepository, SqliteRepository, tables::Table};
    use nfs_world_core::items::{Definition, DefinitionFlags, OwnershipLevel};
    use std::collections::BTreeMap;
    fn catalog() -> InventoryCatalog {
        InventoryCatalog::new(
            [(
                [1; 16],
                Definition {
                    class: DefinitionClass::CategoryUnlockControllerItemData,
                    buy_price: 5,
                    sell_price: 2,
                    quantity: 1,
                    ownership_level: OwnershipLevel::Owned,
                    flags: DefinitionFlags::default(),
                    sub_items: vec![],
                    additional_items: vec![],
                    default_derived: Some(Derived::CategoryUnlockController {
                        unlock_mask: 0x8000_0001,
                    }),
                },
            )],
            vec![[1; 16]],
        )
        .unwrap()
    }
    fn account(n: u8) -> AccountId {
        AccountId::from_owned_config([n; 16]).unwrap()
    }
    fn score(repo: &dyn InventoryRepository, who: AccountId, value: i32) -> Snapshot {
        let before = inventory::load(repo, who, &catalog(), 1, Timestamp(1)).unwrap();
        repo.apply(
            who,
            &Batch {
                id: before.generation + 20,
                expected_generation: before.generation,
                ops: vec![Op::SetTable(
                    key("RepValuesTable"),
                    Table {
                        rows: BTreeMap::from([(
                            0,
                            BTreeMap::from([
                                (key("RepScore"), Value::Int(value)),
                                (key("RepLevel"), Value::Int(999)),
                            ]),
                        )]),
                    },
                )],
            },
            Timestamp(2),
        )
        .unwrap();
        repo.snapshot(who).unwrap()
    }
    fn mask(snapshot: &Snapshot) -> u32 {
        let c = inventory::collection(snapshot, catalog().bindings()).unwrap();
        let Derived::CategoryUnlockController { unlock_mask } = c.items[&1].derived else {
            panic!()
        };
        unlock_mask
    }
    #[test]
    fn threshold_boundary_adds_only_eligible_bits_preserves_unknown_bits_and_other_accounts() {
        let repo = MemoryRepository::default();
        let policy = Policy::new(&catalog(), [1; 16], vec![(2, 10), (4, 25)]).unwrap();
        let other = score(&repo, account(2), 99);
        let before = score(&repo, account(1), 9);
        assert_eq!(
            policy
                .ensure(&repo, account(1), &catalog(), before.clone(), Timestamp(3))
                .unwrap(),
            before
        );
        let before = score(&repo, account(1), 10);
        let after = policy
            .ensure(&repo, account(1), &catalog(), before.clone(), Timestamp(3))
            .unwrap();
        assert_eq!(mask(&after), 0x8000_0003);
        let mut expected = before;
        expected.items = after.items.clone();
        expected.generation += 1;
        expected.updated_at = Timestamp(3);
        assert_eq!(after, expected);
        assert_eq!(repo.snapshot(account(2)).unwrap(), other);
        assert_eq!(
            policy
                .ensure(&repo, account(1), &catalog(), after.clone(), Timestamp(4))
                .unwrap(),
            after
        );
        // Replaying an earlier snapshot converges to the existing committed grant.
        let reset = score(&repo, account(1), 0);
        assert_eq!(
            policy
                .ensure(&repo, account(1), &catalog(), reset.clone(), Timestamp(5))
                .unwrap(),
            reset
        );
        assert_eq!(mask(&reset), 0x8000_0003);
    }
    #[test]
    fn stale_grants_converge_and_missing_rep_never_mutates_inventory() {
        let repo = MemoryRepository::default();
        let policy = Policy::new(&catalog(), [1; 16], vec![(2, 10)]).unwrap();
        let empty = inventory::load(&repo, account(1), &catalog(), 1, Timestamp(1)).unwrap();
        assert!(
            policy
                .ensure(&repo, account(1), &catalog(), empty.clone(), Timestamp(2))
                .is_err()
        );
        assert_eq!(repo.snapshot(account(1)).unwrap(), empty);
        let before = score(&repo, account(1), 10);
        let after = policy
            .ensure(&repo, account(1), &catalog(), before.clone(), Timestamp(3))
            .unwrap();
        assert_eq!(
            policy
                .ensure(&repo, account(1), &catalog(), before, Timestamp(4))
                .unwrap(),
            after
        );
        for rules in [vec![], vec![(3, 1)], vec![(2, -1)], vec![(2, 0), (2, 1)]] {
            assert!(Policy::new(&catalog(), [1; 16], rules).is_err());
        }
        assert!(Policy::new(&catalog(), [9; 16], vec![(2, 0)]).is_err());
    }
    #[test]
    fn grants_survive_database_reopen_without_duplicate_generation() {
        let path = std::env::temp_dir().join(format!(
            "afterhours-unlocks-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let policy = Policy::new(&catalog(), [1; 16], vec![(2, 10), (4, 25)]).unwrap();
        let saved = {
            let repo = SqliteRepository::open_owned_directory(&path).unwrap();
            let before = score(&repo, account(1), 25);
            policy
                .ensure(&repo, account(1), &catalog(), before, Timestamp(3))
                .unwrap()
        };
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        assert_eq!(mask(&saved), 0x8000_0007);
        assert_eq!(repo.snapshot(account(1)).unwrap(), saved);
        assert_eq!(
            policy
                .ensure(&repo, account(1), &catalog(), saved.clone(), Timestamp(4))
                .unwrap(),
            saved
        );
        drop(repo);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn load_reply_contains_the_committed_mask_and_generation() {
        let repo = MemoryRepository::default();
        let policy = Policy::new(&catalog(), [1; 16], vec![(2, 10)]).unwrap();
        let before = score(&repo, account(1), 10);
        let request = items::Request {
            sequence: 9,
            operation: items::LOAD_INVENTORY,
        }
        .encode();
        let reply = inventory::answer_load_with_policy(
            &request,
            &repo,
            account(1),
            &catalog(),
            1,
            Timestamp(3),
            None,
            Some(&policy),
        )
        .unwrap()
        .unwrap();
        let saved = repo.snapshot(account(1)).unwrap();
        assert_eq!(saved.generation, before.generation + 1);
        assert_eq!(reply.generation, saved.generation);
        let envelope = items::Envelope::decode(&reply.bytes).unwrap();
        assert_eq!(
            envelope.collection(catalog().bindings()).unwrap(),
            inventory::collection(&saved, catalog().bindings()).unwrap()
        );
    }
}
