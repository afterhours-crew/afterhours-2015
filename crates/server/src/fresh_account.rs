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

/// Item and table content: explicit files or an installation to build them from.
enum Content {
    Files {
        items: PathBuf,
        tables: PathBuf,
    },
    Game {
        game_dir: PathBuf,
        cache: Option<PathBuf>,
    },
}
struct Args {
    name: String,
    directory: PathBuf,
    policy: PathBuf,
    content: Content,
}
fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let (mut name, mut directory, mut policy, mut items, mut tables) =
        (None, None, None, None, None);
    let (mut game_dir, mut cache) = (None, None);
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
            Some("--game-dir") if game_dir.is_none() => game_dir = Some(PathBuf::from(value)),
            Some("--content-cache") if cache.is_none() => cache = Some(PathBuf::from(value)),
            _ => return Err("unknown or repeated account creation option".into()),
        }
    }
    let content = match (items, tables, game_dir) {
        (Some(items), Some(tables), None) if cache.is_none() => Content::Files { items, tables },
        (None, None, Some(game_dir)) => Content::Game { game_dir, cache },
        _ => {
            return Err(
                "pass --item-content and --persistent-content, or --game-dir (optionally with --content-cache)"
                    .into(),
            );
        }
    };
    Ok(Args {
        name: name.ok_or("--name is required")?,
        directory: directory.ok_or("--state-directory is required")?,
        policy: policy.ok_or("--account-policy is required")?,
        content,
    })
}
fn run(args: Args, options: &nfs_content::Options) -> Result<(), Box<dyn std::error::Error>> {
    let policy = Policy::load(&args.policy)?;
    let (items, tables) = match &args.content {
        Content::Files { items, tables } => (items.clone(), tables.clone()),
        Content::Game { game_dir, cache } => {
            let generated = crate::install_content::resolve(
                std::path::Path::new(""),
                game_dir,
                cache.as_deref(),
                options,
            )?;
            (generated.item_content, generated.persistent_content)
        }
    };
    let items = ItemContent::load(&items)?;
    let items = items
        .inventory()
        .ok_or("item content must contain an initialization policy")?;
    let tables = persistent::Catalog::load(&tables)?;
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
    match run(args, &nfs_content::Options::default()) {
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
        let mut game: Vec<OsString> = args[..6].to_vec();
        game.extend(["--game-dir".into(), "installation".into()]);
        assert!(matches!(
            parse(game.clone().into_iter()).unwrap().content,
            Content::Game { cache: None, .. }
        ));
        let mut both = game.clone();
        both.extend(["--item-content".into(), "items.json".into()]);
        assert!(parse(both.into_iter()).is_err());
        let mut orphan = args.clone();
        orphan.extend(["--content-cache".into(), "cache".into()]);
        assert!(parse(orphan.into_iter()).is_err());
        for extra in [["--name", "replacement"], ["--profile", "existing.sqlite"]] {
            let mut duplicate = args.clone();
            duplicate.extend(extra.into_iter().map(OsString::from));
            assert!(parse(duplicate.into_iter()).is_err());
        }
    }

    #[test]
    fn account_is_created_from_content_built_from_an_installation() {
        let dir = std::env::temp_dir().join(format!("nfs-server-account-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (options, _) =
            nfs_content::synthetic::write(&dir.join("game"), &nfs_content::synthetic::assets())
                .unwrap();
        let policy = dir.join("policy.json");
        std::fs::write(
            &policy,
            serde_json::json!({"format":"nfs-fresh-account-policy","version":1,
                "build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,"screenshot_count_max":20,
                "entitlements":{"scopes":[["synthetic"]],"grants":[]}})
            .to_string(),
        )
        .unwrap();
        let args = Args {
            name: "Synthetic Driver".into(),
            directory: dir.join("account"),
            policy,
            content: Content::Game {
                game_dir: dir.join("game"),
                cache: Some(dir.join("cache")),
            },
        };
        run(args, &options).unwrap();
        assert!(dir.join("account").is_dir());
        assert_eq!(std::fs::read_dir(dir.join("cache")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
