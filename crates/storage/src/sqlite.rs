// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! SQLite adapter: one versioned database file per account under an owned
//! directory. It uses strict
//! tables, application id and `user_version` checks, ownership check on every
//! connection, `IMMEDIATE` transactions, commit before any acknowledgement,
//! WAL after schema acceptance, per-connection size and journal limits.
//!
//! Each call opens its own connection; there is no shared connection or
//! global lock across accounts. Durability past commit relies on SQLite's
//! synchronous mode and the file system honouring it; process-crash and
//! power-loss behaviour is not tested here.
use crate::{
    model::{
        AccountId, Applied, Batch, DefinitionGuid, Error, GarageSlots, ItemId, ItemRecord,
        MAX_BATCH_HISTORY, MAX_DERIVED, MAX_ITEMS, MAX_NESTED, Op, Snapshot, Timestamp,
    },
    port::{
        Decision, InventoryRepository, check_stored, decide, snapshot, transition,
        transition_garage,
    },
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

/// `NFSD`: one file per account, domains
/// added by schema version.
const APPLICATION_ID: i64 = 0x4e46_5344;
/// Version 4 consolidates service state into the inventory account database.
pub const VERSION: i64 = 4;
const TABLES: i64 = 9;
/// 4096-byte pages: 64 MiB, above the worst-case item set.
const MAX_PAGES: i64 = 16384;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const JOURNAL_SIZE_LIMIT: i64 = 1024 * 1024;
const BUSY_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_VALUE_BYTES: i32 = 512 * 1024 + 1024;
const MAX_SQL_BYTES: i32 = 16 * 1024;
const MAX_LIST_BYTES: usize = MAX_NESTED * 8;

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::Busy
            }
            _ => Self::Storage,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SqliteRepository {
    root: PathBuf,
}
impl SqliteRepository {
    /// `root` is an owned local directory (created when absent). Symlinks are
    /// rejected; the canonical path is used from then on.
    pub fn open_owned_directory(root: &Path) -> Result<Self, Error> {
        std::fs::create_dir_all(root).map_err(|_| Error::Storage)?;
        let meta = std::fs::symlink_metadata(root).map_err(|_| Error::Storage)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(Error::Config);
        }
        let root = root.canonicalize().map_err(|_| Error::Storage)?;
        Ok(Self { root })
    }
    /// The account's database file. Exposed for tests and backups.
    pub fn path(&self, account: AccountId) -> PathBuf {
        self.root.join(format!("{}.sqlite", account.hex()))
    }
    pub(crate) fn connect(&self, account: AccountId, create: bool) -> Result<Connection, Error> {
        let path = self.path(account);
        match std::fs::symlink_metadata(&path) {
            Ok(m) => {
                if !m.is_file() || m.file_type().is_symlink() || m.len() > MAX_FILE_BYTES {
                    return Err(Error::Config);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::Absent),
            Err(_) => return Err(Error::Storage),
        }
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if create {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.set_limit(
            rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
            MAX_VALUE_BYTES,
        )?;
        conn.set_limit(
            rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH,
            MAX_SQL_BYTES,
        )?;
        conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 16)?;
        // EXTRA protects the initial rollback-journal import; it equals FULL
        // once the file is in WAL mode.
        conn.execute_batch(
            "PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA;",
        )?;
        let synchronous: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
        if synchronous != 3 {
            return Err(Error::Storage);
        }
        conn.pragma_update(None, "max_page_count", MAX_PAGES)?;
        conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT)?;
        Ok(conn)
    }
    pub(crate) fn check(conn: &Connection, account: AccountId) -> Result<(), Error> {
        Self::check_version(conn, account, VERSION)
    }
    fn check_version(conn: &Connection, account: AccountId, expected: i64) -> Result<(), Error> {
        let app: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if app != APPLICATION_ID || version != expected {
            return Err(Error::Version);
        }
        let tables: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        let expected_tables = match expected {
            1 | 2 => 3,
            3 => 4,
            4 => TABLES,
            _ => return Err(Error::Version),
        };
        if tables != expected_tables {
            return Err(Error::Version);
        }
        let rows: i64 = conn.query_row("SELECT count(*) FROM meta", [], |r| r.get(0))?;
        if rows != 1 {
            return Err(Error::Config);
        }
        let stored: Vec<u8> =
            conn.query_row("SELECT account FROM meta WHERE singleton=1", [], |r| {
                r.get(0)
            })?;
        if stored != account.bytes() {
            return Err(Error::Identity);
        }
        Ok(())
    }
    fn create_v1(tx: &Transaction<'_>, account: AccountId) -> Result<(), Error> {
        let app: i64 = tx.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let objects: i64 = tx.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if app != 0 || objects != 0 {
            return Err(Error::Version);
        }
        tx.execute_batch(&format!(
            "CREATE TABLE meta(\
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),\
                account BLOB NOT NULL CHECK(length(account)=16),\
                generation INTEGER NOT NULL CHECK(generation>=0),\
                updated_at INTEGER NOT NULL CHECK(updated_at>=0)) STRICT;\
             CREATE TABLE item(\
                id BLOB PRIMARY KEY CHECK(length(id)=8),\
                definition BLOB NOT NULL CHECK(length(definition)=16),\
                owner BLOB NOT NULL CHECK(length(owner)=8),\
                state INTEGER NOT NULL CHECK(state BETWEEN 0 AND 255),\
                buy_price INTEGER NOT NULL CHECK(buy_price BETWEEN 0 AND 4294967295),\
                sell_price INTEGER NOT NULL CHECK(sell_price BETWEEN 0 AND 4294967295),\
                default_items BLOB NOT NULL CHECK(length(default_items)<={MAX_LIST_BYTES} AND length(default_items)%8=0),\
                sub_items BLOB NOT NULL CHECK(length(sub_items)<={MAX_LIST_BYTES} AND length(sub_items)%8=0),\
                derived BLOB NOT NULL CHECK(length(derived)<={MAX_DERIVED})) STRICT;\
             CREATE TABLE batch(\
                id BLOB PRIMARY KEY CHECK(length(id)=8),\
                generation INTEGER NOT NULL CHECK(generation>0),\
                applied_at INTEGER NOT NULL CHECK(applied_at>=0)) STRICT;"
        ))?;
        tx.execute(
            "INSERT INTO meta VALUES(1,?1,0,0)",
            params![account.bytes().as_slice()],
        )?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", 1)?;
        Ok(())
    }
    fn migrate_v2(tx: &Transaction<'_>, account: AccountId) -> Result<(), Error> {
        Self::check_version(tx, account, 1)?;
        Self::read_meta(tx)?;
        Self::read_items(tx)?;
        tx.execute_batch("ALTER TABLE meta ADD COLUMN garage BLOB CHECK(garage IS NULL OR length(garage)=40); PRAGMA user_version=2;")?;
        Ok(())
    }
    fn read_garage(conn: &Connection) -> Result<Option<GarageSlots>, Error> {
        let bytes: Option<Vec<u8>> =
            conn.query_row("SELECT garage FROM meta WHERE singleton=1", [], |r| {
                r.get(0)
            })?;
        bytes
            .map(|bytes| {
                let bytes: [u8; 40] = bytes.try_into().map_err(|_| Error::Config)?;
                let slots = GarageSlots(std::array::from_fn(|i| {
                    ItemId::from_bytes(bytes[i * 8..i * 8 + 8].try_into().expect("fixed slice"))
                }));
                slots.validate().map_err(|_| Error::Config)?;
                Ok(slots)
            })
            .transpose()
    }
    fn migrate_v3(tx: &Transaction<'_>, account: AccountId) -> Result<(), Error> {
        Self::check_version(tx, account, 2)?;
        Self::read_meta(tx)?;
        let items = Self::read_items(tx)?;
        if let Some(garage) = Self::read_garage(tx)? {
            garage.validate_items(&items).map_err(|_| Error::Config)?;
        }
        tx.execute_batch(&format!(
            "CREATE TABLE persistent(\
             id INTEGER PRIMARY KEY CHECK(id BETWEEN 0 AND 4294967295),\
             rows BLOB NOT NULL CHECK(length(rows)<={})) STRICT; PRAGMA user_version=3;",
            crate::tables::MAX_TABLE_BYTES
        ))?;
        Ok(())
    }
    fn read_tables(conn: &Connection) -> Result<BTreeMap<u32, crate::tables::Table>, Error> {
        let mut statement = conn.prepare("SELECT id,rows FROM persistent ORDER BY id LIMIT ?1")?;
        let mut tables = BTreeMap::new();
        for row in statement.query_map([(crate::tables::MAX_TABLES + 1) as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })? {
            let (id, bytes) = row?;
            if tables.len() == crate::tables::MAX_TABLES {
                return Err(Error::Config);
            }
            let id = u32::try_from(id).map_err(|_| Error::Config)?;
            if tables
                .insert(id, crate::tables::Table::decode(&bytes)?)
                .is_some()
            {
                return Err(Error::Config);
            }
        }
        Ok(tables)
    }
    fn read_meta(conn: &Connection) -> Result<(u64, Timestamp), Error> {
        let (generation, updated_at): (i64, i64) = conn.query_row(
            "SELECT generation,updated_at FROM meta WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let generation = u64::try_from(generation).map_err(|_| Error::Config)?;
        let updated_at = u64::try_from(updated_at).map_err(|_| Error::Config)?;
        Ok((generation, Timestamp(updated_at)))
    }
    fn read_items(conn: &Connection) -> Result<BTreeMap<ItemId, ItemRecord>, Error> {
        let mut statement = conn.prepare(
            "SELECT id,definition,owner,state,buy_price,sell_price,default_items,sub_items,derived \
             FROM item ORDER BY id LIMIT ?1",
        )?;
        let limit = i64::try_from(MAX_ITEMS + 1).map_err(|_| Error::Config)?;
        let mut items = BTreeMap::new();
        for row in statement.query_map([limit], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Vec<u8>>(6)?,
                r.get::<_, Vec<u8>>(7)?,
                r.get::<_, Vec<u8>>(8)?,
            ))
        })? {
            let (id, definition, owner, state, buy, sell, defaults, subs, derived) = row?;
            let id = id
                .try_into()
                .ok()
                .and_then(ItemId::from_bytes)
                .ok_or(Error::Config)?;
            let definition: [u8; 16] = definition.try_into().map_err(|_| Error::Config)?;
            let owner: [u8; 8] = owner.try_into().map_err(|_| Error::Config)?;
            let record = ItemRecord {
                id,
                definition: DefinitionGuid(definition),
                owner: ItemId::from_bytes(owner),
                state: u8::try_from(state).map_err(|_| Error::Config)?,
                buy_price: u32::try_from(buy).map_err(|_| Error::Config)?,
                sell_price: u32::try_from(sell).map_err(|_| Error::Config)?,
                default_items: decode_list(&defaults)?,
                sub_items: decode_list(&subs)?,
                derived,
            };
            if items.insert(id, record).is_some() {
                return Err(Error::Config);
            }
        }
        check_stored(&items)?;
        Ok(items)
    }
    fn write_op(tx: &Transaction<'_>, op: &Op) -> Result<(), Error> {
        match op {
            Op::SetGarage(_) => {} // Written with metadata after final-state validation.
            Op::SetTable(id, table) => {
                tx.execute(
                    "INSERT OR REPLACE INTO persistent VALUES(?1,?2)",
                    params![i64::from(*id), table.encode()?],
                )?;
            }
            Op::Insert(r) | Op::Update(r) => {
                let changed = tx.execute(
                    "INSERT OR REPLACE INTO item VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        r.id.to_bytes().as_slice(),
                        r.definition.0.as_slice(),
                        r.owner.map_or([0; 8], ItemId::to_bytes).as_slice(),
                        i64::from(r.state),
                        i64::from(r.buy_price),
                        i64::from(r.sell_price),
                        encode_list(&r.default_items),
                        encode_list(&r.sub_items),
                        r.derived,
                    ],
                )?;
                if changed != 1 {
                    return Err(Error::Storage);
                }
            }
            Op::Remove(id) => {
                let changed = tx.execute(
                    "DELETE FROM item WHERE id=?1",
                    params![id.to_bytes().as_slice()],
                )?;
                if changed != 1 {
                    return Err(Error::Storage);
                }
            }
        }
        Ok(())
    }
    pub(crate) fn snapshot_in(conn: &Connection, account: AccountId) -> Result<Snapshot, Error> {
        Self::check(conn, account)?;
        let (generation, updated_at) = Self::read_meta(conn)?;
        let items = Self::read_items(conn)?;
        let garage = Self::read_garage(conn)?;
        if let Some(slots) = garage {
            slots.validate_items(&items).map_err(|_| Error::Config)?;
        }
        let mut result = snapshot(generation, updated_at, items);
        result.garage = garage;
        result.tables = Self::read_tables(conn)?;
        Ok(result)
    }
}
fn encode_list(ids: &[ItemId]) -> Vec<u8> {
    ids.iter().flat_map(|id| id.to_bytes()).collect()
}
fn decode_list(bytes: &[u8]) -> Result<Vec<ItemId>, Error> {
    if !bytes.len().is_multiple_of(8) || bytes.len() > MAX_LIST_BYTES {
        return Err(Error::Config);
    }
    bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| ItemId::from_bytes(*c).ok_or(Error::Config))
        .collect()
}

impl InventoryRepository for SqliteRepository {
    fn open(&self, account: AccountId) -> Result<Snapshot, Error> {
        let mut conn = self.connect(account, true)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            Self::create_v1(&tx, account)?;
            Self::migrate_v2(&tx, account)?;
            Self::migrate_v3(&tx, account)?;
        } else if version == 1 {
            Self::migrate_v2(&tx, account)?;
            Self::migrate_v3(&tx, account)?;
        } else if version == 2 {
            Self::migrate_v3(&tx, account)?;
        } else if version == 3 {
            Self::check_version(&tx, account, 3)?;
        } else if version > VERSION {
            return Err(Error::Version);
        } else {
            Self::check(&tx, account)?;
        }
        if version < 4 {
            Self::check_version(&tx, account, 3)?;
            Self::read_meta(&tx)?;
            let items = Self::read_items(&tx)?;
            if let Some(garage) = Self::read_garage(&tx)? {
                garage.validate_items(&items).map_err(|_| Error::Config)?;
            }
            Self::read_tables(&tx)?;
            crate::account::create_tables(&tx)?;
            tx.pragma_update(None, "user_version", VERSION)?;
        }
        tx.commit()?;
        // WAL only after schema and ownership acceptance; it persists in the file.
        let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if mode != "wal" {
            return Err(Error::Storage);
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let snapshot = Self::snapshot_in(&tx, account)?;
        tx.commit()?;
        Ok(snapshot)
    }
    fn snapshot(&self, account: AccountId) -> Result<Snapshot, Error> {
        let mut conn = self.connect(account, false)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let snapshot = Self::snapshot_in(&tx, account)?;
        tx.commit()?;
        Ok(snapshot)
    }
    fn apply(&self, account: AccountId, batch: &Batch, now: Timestamp) -> Result<Applied, Error> {
        let mut conn = self.connect(account, false)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = Self::apply_in(&tx, account, batch, now)?;
        tx.commit()?;
        Ok(result)
    }
}

impl SqliteRepository {
    pub(crate) fn apply_in(
        tx: &Transaction<'_>,
        account: AccountId,
        batch: &Batch,
        now: Timestamp,
    ) -> Result<Applied, Error> {
        Self::check(tx, account)?;
        let (generation, _) = Self::read_meta(tx)?;
        let recorded: Option<i64> = tx
            .query_row(
                "SELECT generation FROM batch WHERE id=?1",
                params![batch.id.to_be_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        let recorded = recorded
            .map(|g| u64::try_from(g).map_err(|_| Error::Config))
            .transpose()?;
        let next_generation = match decide(generation, recorded, batch, now)? {
            Decision::Replay(applied) => return Ok(applied),
            Decision::Apply { next_generation } => next_generation,
        };
        let items = Self::read_items(tx)?;
        let next = transition(&items, batch)?;
        let garage = transition_garage(Self::read_garage(tx)?, &next, batch)?;
        crate::tables::transition(&Self::read_tables(tx)?, &batch.ops)?;
        for op in &batch.ops {
            Self::write_op(tx, op)?;
        }
        let count: i64 = tx.query_row("SELECT count(*) FROM item", [], |r| r.get(0))?;
        if usize::try_from(count).map_err(|_| Error::Storage)? != next.len() {
            return Err(Error::Storage);
        }
        let next_generation_i = i64::try_from(next_generation).map_err(|_| Error::Bounds)?;
        let now_i = i64::try_from(now.0).map_err(|_| Error::Bounds)?;
        tx.execute(
            "UPDATE meta SET generation=?1,updated_at=?2,garage=?3 WHERE singleton=1",
            params![
                next_generation_i,
                now_i,
                garage.map(|slots| slots
                    .0
                    .into_iter()
                    .flat_map(|id| id.map_or([0; 8], ItemId::to_bytes))
                    .collect::<Vec<_>>())
            ],
        )?;
        tx.execute(
            "INSERT INTO batch VALUES(?1,?2,?3)",
            params![batch.id.to_be_bytes().as_slice(), next_generation_i, now_i],
        )?;
        // Keep the newest MAX_BATCH_HISTORY ids; a missing offset row makes
        // the subquery NULL and deletes nothing.
        tx.execute(
            "DELETE FROM batch WHERE generation<=(SELECT generation FROM batch ORDER BY generation DESC LIMIT 1 OFFSET ?1)",
            params![i64::try_from(MAX_BATCH_HISTORY).map_err(|_| Error::Config)?],
        )?;
        // The enclosing operation must commit before acknowledging anything.
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
    fn list_round_trip_and_bounds() {
        let ids: Vec<ItemId> = (1..=3).filter_map(ItemId::new).collect();
        let bytes = encode_list(&ids);
        assert_eq!(bytes.len(), 24);
        assert_eq!(decode_list(&bytes).unwrap(), ids);
        assert_eq!(decode_list(&bytes[..7]), Err(Error::Config));
        assert_eq!(decode_list(&[0u8; 8]), Err(Error::Config));
        assert_eq!(
            decode_list(&vec![1u8; MAX_LIST_BYTES + 8]),
            Err(Error::Config)
        );
    }
    #[test]
    fn account_filename_is_hex() {
        let account = AccountId::from_owned_config([0xab; 16]).unwrap();
        let repo = SqliteRepository {
            root: PathBuf::from("root"),
        };
        assert_eq!(
            repo.path(account).file_name().unwrap().to_str().unwrap(),
            format!("{}.sqlite", "ab".repeat(16))
        );
    }
}
