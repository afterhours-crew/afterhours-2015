// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! In-memory adapter: the reference semantics for tests and for the server's
//! in-memory mode. Nothing survives the process.
use crate::{
    model::{AccountId, Applied, Batch, Error, MAX_BATCH_HISTORY, Snapshot, Timestamp},
    port::{Decision, InventoryRepository, decide, transition, transition_garage},
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

/// Local policy bounds accounts retained by one in-memory repository.
pub const MAX_ACCOUNTS: usize = 128;

#[derive(Default)]
struct State {
    snapshot: Snapshot,
    /// (batch id, generation after it), oldest first.
    history: VecDeque<(u64, u64)>,
}

#[derive(Default)]
pub struct MemoryRepository {
    accounts: Mutex<BTreeMap<AccountId, Arc<Mutex<State>>>>,
}
impl MemoryRepository {
    pub fn new() -> Self {
        Self::default()
    }
    fn account(&self, account: AccountId, create: bool) -> Result<Arc<Mutex<State>>, Error> {
        let mut accounts = self.accounts.lock().map_err(|_| Error::Storage)?;
        if let Some(state) = accounts.get(&account) {
            return Ok(Arc::clone(state));
        }
        if !create {
            return Err(Error::Absent);
        }
        if accounts.len() >= MAX_ACCOUNTS {
            return Err(Error::Bounds);
        }
        let state = Arc::new(Mutex::new(State::default()));
        accounts.insert(account, Arc::clone(&state));
        Ok(state)
    }
}
impl InventoryRepository for MemoryRepository {
    fn open(&self, account: AccountId) -> Result<Snapshot, Error> {
        let state = self.account(account, true)?;
        Ok(state.lock().map_err(|_| Error::Storage)?.snapshot.clone())
    }
    fn snapshot(&self, account: AccountId) -> Result<Snapshot, Error> {
        let state = self.account(account, false)?;
        Ok(state.lock().map_err(|_| Error::Storage)?.snapshot.clone())
    }
    fn receipt(&self, account: AccountId, batch_id: u64) -> Result<Option<Applied>, Error> {
        if batch_id == 0 {
            return Err(Error::Invalid);
        }
        let state = self.account(account, false)?;
        let state = state.lock().map_err(|_| Error::Storage)?;
        Ok(state.history.iter().find_map(|&(id, generation)| {
            (id == batch_id).then_some(Applied {
                generation,
                replayed: true,
            })
        }))
    }
    fn apply(&self, account: AccountId, batch: &Batch, now: Timestamp) -> Result<Applied, Error> {
        let state = self.account(account, false)?;
        let mut state = state.lock().map_err(|_| Error::Storage)?;
        let recorded = state
            .history
            .iter()
            .find(|(id, _)| *id == batch.id)
            .map(|(_, generation)| *generation);
        let next_generation = match decide(state.snapshot.generation, recorded, batch, now)? {
            Decision::Replay(applied) => return Ok(applied),
            Decision::Apply { next_generation } => next_generation,
        };
        let items = transition(&state.snapshot.items, batch)?;
        let garage = transition_garage(state.snapshot.garage, &items, batch)?;
        let tables = crate::tables::transition(&state.snapshot.tables, &batch.ops)?;
        state.snapshot.items = items;
        state.snapshot.garage = garage;
        state.snapshot.tables = tables;
        state.snapshot.generation = next_generation;
        state.snapshot.updated_at = now;
        state.history.push_back((batch.id, next_generation));
        while state.history.len() > MAX_BATCH_HISTORY {
            state.history.pop_front();
        }
        Ok(Applied {
            generation: next_generation,
            replayed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn busy_account_does_not_hold_other_accounts_or_the_directory() {
        let repo = Arc::new(MemoryRepository::new());
        let a = AccountId::from_owned_config([1; 16]).unwrap();
        let b = AccountId::from_owned_config([2; 16]).unwrap();
        repo.open(a).unwrap();
        let state = repo.account(a, false).unwrap();
        let held = state.lock().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let other = Arc::clone(&repo);
        let worker = std::thread::spawn(move || {
            send.send(other.open(b)).unwrap();
        });
        let result = receive.recv_timeout(std::time::Duration::from_secs(2));
        drop(held);
        worker.join().unwrap();
        assert_eq!(result.unwrap(), Ok(Snapshot::default()));
    }

    #[test]
    fn account_bound_keeps_existing_accounts_available() {
        let repo = MemoryRepository::new();
        for i in 1..=MAX_ACCOUNTS {
            repo.open(AccountId::from_owned_config((i as u128).to_be_bytes()).unwrap())
                .unwrap();
        }
        assert_eq!(
            repo.open(AccountId::from_owned_config([0xff; 16]).unwrap()),
            Err(Error::Bounds)
        );
        assert_eq!(
            repo.open(AccountId::from_owned_config(1u128.to_be_bytes()).unwrap()),
            Ok(Snapshot::default())
        );
    }
}
