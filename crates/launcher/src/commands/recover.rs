// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `recover`: restore the small account pointer for an existing database.

use super::Context;
use crate::{
    exit::{Failure, Result},
    paths, records,
    session::Lock,
};

/// Largest account database the pointer may be restored for.
const MAX_DATABASE_BYTES: u64 = 64 * 1024 * 1024;

/// Restore `account.json` for `artifacts/account/<id>.sqlite`. Never
/// creates, imports or resets a database.
pub fn recover(context: &Context, account_id: &str) -> Result<()> {
    if !records::is_id(account_id) {
        return Err(Failure::refused(
            "Recovery requires --account-id: the 32-character ID in the successful create-account output or saved SQLite filename.",
        ));
    }
    let data = paths::checked_data_tree(&context.data_dir, None)?;
    if !data.join(crate::session::LOCK_FILE).exists() {
        return Err(Failure::no_session(
            "This data directory has never run a session; there is nothing to recover.",
        ));
    }
    let _lock = Lock::acquire(&data)?;
    let database = data
        .join("artifacts")
        .join("account")
        .join(format!("{account_id}.sqlite"));
    let meta = std::fs::symlink_metadata(&database)
        .map_err(|_| Failure::refused("Account database is absent or exceeds its bound."))?;
    if paths::is_link(&meta)
        || !meta.is_file()
        || meta.len() == 0
        || meta.len() > MAX_DATABASE_BYTES
    {
        return Err(Failure::refused(
            "Account database is absent or exceeds its bound.",
        ));
    }
    let target = data.join("account.json");
    let record = records::AccountRecord::post_intro(account_id.to_owned());
    if target.exists() {
        let existing: records::AccountRecord = records::read_json(&target)?;
        if existing.version != records::VERSION || existing.local_account != account_id {
            return Err(Failure::refused(
                "An existing account record will not be replaced.",
            ));
        }
    } else {
        records::write_new_json(&target, &record)?;
    }
    context.emit(
        &serde_json::json!({"recovered": true, "account": record}),
        || {
            "Account pointer recovered. The server validates database ownership and schema on next start; no account data was modified.".to_owned()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(data: &std::path::Path) -> Context {
        Context {
            data_dir: data.to_path_buf(),
            json: true,
        }
    }

    #[test]
    fn recovery_needs_a_valid_id_a_prior_session_and_a_bounded_database() {
        let data = crate::testing::scratch("recover");
        let id = "a".repeat(32);
        assert!(recover(&context(&data), "nope").is_err());
        assert_eq!(
            recover(&context(&data), &id).unwrap_err().exit,
            crate::exit::Exit::NoSession
        );
        std::fs::write(data.join(crate::session::LOCK_FILE), b"").unwrap();
        assert!(
            recover(&context(&data), &id)
                .unwrap_err()
                .message
                .contains("absent")
        );
        let account = data.join("artifacts").join("account");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join(format!("{id}.sqlite")), b"").unwrap();
        assert!(recover(&context(&data), &id).is_err(), "empty database");
        std::fs::write(account.join(format!("{id}.sqlite")), b"sqlite").unwrap();
        recover(&context(&data), &id).unwrap();
        let record: records::AccountRecord =
            records::read_json(&data.join("account.json")).unwrap();
        assert_eq!(record.local_account, id);
        recover(&context(&data), &id).unwrap();
        let other = "b".repeat(32);
        std::fs::write(account.join(format!("{other}.sqlite")), b"sqlite").unwrap();
        assert!(
            recover(&context(&data), &other)
                .unwrap_err()
                .message
                .contains("will not be replaced")
        );
        std::fs::remove_dir_all(data).unwrap();
    }
}
