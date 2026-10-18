// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Explicit offline test-profile REP adjustment. No network or admin access.
use nfs_services::{
    persistent::Catalog,
    reputation::{self, Field, Scores, Thresholds},
};
use nfs_storage::{AccountId, InventoryRepository, SqliteRepository, Timestamp};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, process::ExitCode};

const HELP: &str = "Usage: nfs-server reputation --state-directory <account-directory> --local-account <32 lowercase hex digits> --persistent-content <tables.json> --reputation-thresholds <thresholds.json> [--set-level <level|max> | --set-score <total>] [--expected-generation <generation>]\nInspect REP and generation as JSON. Stop the game/server and back up the test account before setting REP. --set-level sets the total score to the installed level threshold; --set-score sets an exact total and derives the level. Neither is earned gameplay progression. Style scores, money and inventory are preserved. Existing accounts only; no administrator access is needed.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// An authored level, `None` for the highest installed one.
    Level(Option<u32>),
    /// An exact total score.
    Score(i32),
}
struct Args {
    directory: PathBuf,
    account: AccountId,
    tables: PathBuf,
    thresholds: PathBuf,
    set: Option<(Target, u64)>,
}
fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let mut fields = BTreeMap::new();
    while let Some(flag) = args.next() {
        let name = flag.to_str().ok_or("bad option")?;
        if ![
            "--state-directory",
            "--local-account",
            "--persistent-content",
            "--reputation-thresholds",
            "--set-level",
            "--set-score",
            "--expected-generation",
        ]
        .contains(&name)
        {
            return Err("unknown reputation option".into());
        }
        let value = args.next().ok_or("missing option value")?;
        if fields.insert(name.to_owned(), value).is_some() {
            return Err("repeated reputation option".into());
        }
    }
    let mut take = |name: &str| {
        fields
            .remove(name)
            .ok_or_else(|| format!("{name} is required"))
    };
    let directory = PathBuf::from(take("--state-directory")?);
    let tables = PathBuf::from(take("--persistent-content")?);
    let thresholds = PathBuf::from(take("--reputation-thresholds")?);
    let account = take("--local-account")?;
    let account = account.to_str().ok_or("bad account id")?;
    if account.len() != 32
        || !account
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("bad account id".into());
    }
    let mut bytes = [0; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&account[i * 2..i * 2 + 2], 16).map_err(|_| "bad account id")?;
    }
    let account = AccountId::from_owned_config(bytes).map_err(|_| "bad account id")?;
    let set = match (
        fields.remove("--set-level"),
        fields.remove("--set-score"),
        fields.remove("--expected-generation"),
    ) {
        (None, None, None) => None,
        (Some(level), None, Some(generation)) => {
            let level = if level == "max" {
                None
            } else {
                let level = level
                    .to_str()
                    .ok_or("bad level")?
                    .parse::<u32>()
                    .map_err(|_| "bad level")?;
                if level == 0 {
                    return Err("level must be at least 1".into());
                }
                Some(level)
            };
            Some((Target::Level(level), parse_generation(&generation)?))
        }
        (None, Some(score), Some(generation)) => {
            let score = score
                .to_str()
                .ok_or("bad score")?
                .parse::<i32>()
                .map_err(|_| "bad score")?;
            if score < 0 {
                return Err("score must not be negative".into());
            }
            Some((Target::Score(score), parse_generation(&generation)?))
        }
        _ => {
            return Err(
                "exactly one of --set-level or --set-score with --expected-generation is required for a change"
                    .into(),
            );
        }
    };
    Ok(Args {
        directory,
        account,
        tables,
        thresholds,
        set,
    })
}
fn parse_generation(value: &OsString) -> Result<u64, String> {
    value
        .to_str()
        .ok_or("bad generation")?
        .parse::<u64>()
        .map_err(|_| "bad generation".into())
}
fn run(args: &Args, now: Timestamp) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    if !args.directory.is_dir() {
        return Err("account directory does not exist".into());
    }
    let catalog = Catalog::load(&args.tables)?;
    let thresholds = Thresholds::load(&args.thresholds)?;
    let repository = SqliteRepository::open_owned_directory(&args.directory)?;
    let before = repository.snapshot(args.account)?;
    let previous = thresholds.evaluate(Scores::load(&catalog.view(&before)?)?);
    let saved = match args.set {
        Some((Target::Level(level), generation)) => reputation::set_for_operator(
            &repository,
            args.account,
            &catalog,
            &thresholds,
            generation,
            level.unwrap_or(thresholds.maximum_level()),
            now,
        )?,
        Some((Target::Score(score), generation)) => reputation::set_score_for_operator(
            &repository,
            args.account,
            &catalog,
            &thresholds,
            generation,
            score,
            now,
        )?,
        None => before.clone(),
    };
    let current = thresholds.evaluate(Scores::load(&catalog.view(&saved)?)?);
    Ok(
        serde_json::json!({"account":args.account.hex(),"generation":saved.generation,
        "level":current.get(Field::Level),"score":current.get(Field::Total),"maximum_level":thresholds.maximum_level(),
        "previous_generation":before.generation,"previous_level":previous.get(Field::Level),"previous_score":previous.get(Field::Total),"adjusted":args.set.is_some()}),
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
    let result = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(Box::<dyn std::error::Error>::from)
        .and_then(|t| run(&args, Timestamp(t.as_secs())));
    match result {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("reputation operation failed: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mutation_requires_explicit_level_and_generation() {
        let base = [
            "--state-directory",
            "existing",
            "--local-account",
            "01010101010101010101010101010101",
            "--persistent-content",
            "tables.json",
            "--reputation-thresholds",
            "thresholds.json",
        ];
        assert!(
            parse(base.into_iter().map(Into::into))
                .unwrap()
                .set
                .is_none()
        );
        for extra in [
            vec!["--set-level", "max"],
            vec!["--expected-generation", "1"],
            vec!["--set-level", "0", "--expected-generation", "1"],
            vec!["--set-level", "-1", "--expected-generation", "1"],
            vec![
                "--set-level",
                "2",
                "--set-level",
                "3",
                "--expected-generation",
                "1",
            ],
            vec!["--set-score", "5"],
            vec!["--set-score", "-5", "--expected-generation", "1"],
            vec!["--set-score", "x", "--expected-generation", "1"],
            vec![
                "--set-level",
                "2",
                "--set-score",
                "5",
                "--expected-generation",
                "1",
            ],
        ] {
            assert!(parse(base.into_iter().chain(extra).map(Into::into)).is_err());
        }
        assert_eq!(
            parse(
                base.into_iter()
                    .chain(["--set-level", "max", "--expected-generation", "7"])
                    .map(Into::into)
            )
            .unwrap()
            .set,
            Some((Target::Level(None), 7))
        );
        assert_eq!(
            parse(
                base.into_iter()
                    .chain(["--set-score", "13040000", "--expected-generation", "7"])
                    .map(Into::into)
            )
            .unwrap()
            .set,
            Some((Target::Score(13_040_000), 7))
        );
    }
}
