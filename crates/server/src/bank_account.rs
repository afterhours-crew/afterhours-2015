// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local operator cash inspection/adjustment. No game, network or admin access.
use nfs_services::bank::{self, Balance};
use nfs_storage::{AccountId, InventoryRepository, SqliteRepository, Timestamp};
use std::{ffi::OsString, path::PathBuf, process::ExitCode};

const HELP: &str = "Usage: nfs-server bank --state-directory <account-directory> --local-account <32 lowercase hex digits> [--set <u32 amount> --expected-generation <u64>]\nInspect cash/generation as JSON. Stop the game/server before setting cash. This is an explicit developer grant/reset, not earned gameplay money. Existing accounts only; no game content or administrator access is needed.";
struct Args {
    directory: PathBuf,
    account: AccountId,
    set: Option<(u32, u64)>,
}
fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let (mut directory, mut account, mut amount, mut generation) = (None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("missing bank option value")?;
        match flag.to_str() {
            Some("--state-directory") if directory.is_none() => {
                directory = Some(PathBuf::from(value))
            }
            Some("--local-account") if account.is_none() => {
                let text = value.to_str().ok_or("bad account id")?;
                if text.len() != 32
                    || !text
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err("bad account id".into());
                }
                let mut bytes = [0; 16];
                for (i, byte) in bytes.iter_mut().enumerate() {
                    *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
                        .map_err(|_| "bad account id")?;
                }
                account = Some(AccountId::from_owned_config(bytes).map_err(|_| "bad account id")?);
            }
            Some("--set") if amount.is_none() => {
                amount = Some(
                    value
                        .to_str()
                        .ok_or("bad amount")?
                        .parse::<u32>()
                        .map_err(|_| "bad amount")?,
                )
            }
            Some("--expected-generation") if generation.is_none() => {
                generation = Some(
                    value
                        .to_str()
                        .ok_or("bad generation")?
                        .parse::<u64>()
                        .map_err(|_| "bad generation")?,
                )
            }
            _ => return Err("unknown or repeated bank option".into()),
        }
    }
    let set = match (amount, generation) {
        (None, None) => None,
        (Some(a), Some(g)) => Some((a, g)),
        _ => return Err("--set and --expected-generation must be supplied together".into()),
    };
    Ok(Args {
        directory: directory.ok_or("--state-directory is required")?,
        account: account.ok_or("--local-account is required")?,
        set,
    })
}
fn run(args: &Args, now: Timestamp) -> Result<serde_json::Value, nfs_storage::Error> {
    if !args.directory.is_dir() {
        return Err(nfs_storage::Error::Config);
    }
    let repo = SqliteRepository::open_owned_directory(&args.directory)?;
    let before = repo.snapshot(args.account)?;
    let old = Balance::from_snapshot(&before)?;
    let saved = if let Some((amount, generation)) = args.set {
        bank::set_for_operator(&repo, args.account, generation, Balance(amount), now)?
    } else {
        before.clone()
    };
    Ok(
        serde_json::json!({"account":args.account.hex(),"generation":saved.generation,"balance":Balance::from_snapshot(&saved)?.0,
        "adjusted":args.set.is_some(),"previous_balance":old.0,"previous_generation":before.generation}),
    )
}
pub fn main(args: impl Iterator<Item = OsString>) -> ExitCode {
    let mut args = args.peekable();
    if args.peek().is_some_and(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let args = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n{HELP}");
            return ExitCode::from(2);
        }
    };
    let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(t) => Timestamp(t.as_secs()),
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    match run(&args, now) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("bank operation failed: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bank_options_are_explicit_and_inspection_is_read_only() {
        let base = [
            "--state-directory",
            "existing",
            "--local-account",
            "01010101010101010101010101010101",
        ];
        assert!(
            parse(base.into_iter().map(Into::into))
                .unwrap()
                .set
                .is_none()
        );
        for extra in [
            vec!["--set", "1"],
            vec!["--expected-generation", "0"],
            vec!["--set", "-1", "--expected-generation", "0"],
            vec!["--set", "4294967296", "--expected-generation", "0"],
            vec!["--set", "1", "--set", "2", "--expected-generation", "0"],
        ] {
            assert!(parse(base.into_iter().chain(extra).map(Into::into)).is_err());
        }
        assert_eq!(
            parse(
                base.into_iter()
                    .chain(["--set", "2500", "--expected-generation", "1"])
                    .map(Into::into)
            )
            .unwrap()
            .set,
            Some((2500, 1))
        );
    }
    #[test]
    fn cli_inspects_and_sets_an_existing_database_without_creating_accounts() {
        let path = std::env::temp_dir().join(format!(
            "afterhours-bank-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let account = AccountId::from_owned_config([1; 16]).unwrap();
        let mut args = Args {
            directory: path.clone(),
            account,
            set: None,
        };
        assert!(run(&args, Timestamp(1)).is_err());
        assert!(!path.exists());
        let repo = SqliteRepository::open_owned_directory(&path).unwrap();
        assert!(run(&args, Timestamp(1)).is_err());
        assert!(!repo.path(account).exists());
        let before = repo.open(account).unwrap();
        assert_eq!(run(&args, Timestamp(1)).unwrap()["balance"], 0);
        assert_eq!(repo.snapshot(account).unwrap(), before);
        args.set = Some((2500, 0));
        assert_eq!(run(&args, Timestamp(2)).unwrap()["balance"], 2500);
        args.set = Some((9000, 0));
        assert_eq!(run(&args, Timestamp(3)), Err(nfs_storage::Error::Conflict));
        assert_eq!(
            Balance::from_snapshot(&repo.snapshot(account).unwrap()),
            Ok(Balance(2500))
        );
        drop(repo);
        std::fs::remove_dir_all(path).unwrap();
    }
}
