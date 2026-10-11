// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Synchronous one-shot account creation; no listeners, game or external auth.
use crate::seeds::{OsSeeds, fresh};
use nfs_services::{
    fresh_account::{Policy, Prepared, SEED_BYTES},
    item_content::ItemContent,
    persistent,
};
use std::{
    ffi::OsString,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

struct Args {
    name: String,
    directory: PathBuf,
    policy: PathBuf,
    items: PathBuf,
    tables: PathBuf,
}
fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let (mut name, mut directory, mut policy, mut items, mut tables) =
        (None, None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("missing option value")?;
        match flag.to_str() {
            Some("--name") if name.is_none() => {
                name = Some(value.into_string().map_err(|_| "name must be UTF-8")?)
            }
            Some("--state-directory") if directory.is_none() => {
                directory = Some(PathBuf::from(value))
            }
            Some("--account-policy") if policy.is_none() => policy = Some(PathBuf::from(value)),
            Some("--item-content") if items.is_none() => items = Some(PathBuf::from(value)),
            Some("--persistent-content") if tables.is_none() => tables = Some(PathBuf::from(value)),
            _ => return Err("unknown or repeated account creation option".into()),
        }
    }
    Ok(Args {
        name: name.ok_or("--name is required")?,
        directory: directory.ok_or("--state-directory is required")?,
        policy: policy.ok_or("--account-policy is required")?,
        items: items.ok_or("--item-content is required")?,
        tables: tables.ok_or("--persistent-content is required")?,
    })
}
fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let policy = Policy::load(&args.policy)?;
    let items = ItemContent::load(&args.items)?;
    let items = items
        .inventory()
        .ok_or("item content must contain an initialization policy")?;
    let tables = persistent::Catalog::load(&args.tables)?;
    let now = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let prepared = Prepared::new(
        &args.name,
        fresh::<SEED_BYTES>(&mut OsSeeds)?,
        nfs_storage::Timestamp(now),
        &policy,
        items,
        &tables,
    )?;
    prepared.publish(&args.directory)?;
    println!(
        "{}",
        serde_json::json!({"created":true,"local_account":prepared.identity().storage_account().hex(),"schema_version":nfs_storage::sqlite::VERSION})
    );
    Ok(())
}
pub fn main(args: impl Iterator<Item = OsString>) -> std::process::ExitCode {
    let args = match parse(args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_content_is_explicit_and_unknown_or_repeated_options_fail() {
        let args: Vec<OsString> = [
            "--name",
            "Synthetic Driver",
            "--state-directory",
            "new-state",
            "--account-policy",
            "policy.json",
            "--item-content",
            "items.json",
            "--persistent-content",
            "tables.json",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        assert_eq!(
            parse(args.clone().into_iter()).unwrap().name,
            "Synthetic Driver"
        );
        assert!(parse(args[..8].iter().cloned()).is_err());
        for extra in [["--name", "replacement"], ["--profile", "existing.sqlite"]] {
            let mut duplicate = args.clone();
            duplicate.extend(extra.into_iter().map(OsString::from));
            assert!(parse(duplicate.into_iter()).is_err());
        }
    }
}
