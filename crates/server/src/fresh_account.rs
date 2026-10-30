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
/// Saved progression of the new account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Progression {
    /// Asset-default tables: the wired initial objectives, i.e. the start of
    /// the official prologue. The starter car is granted by story selection.
    Fresh,
    /// The intro branch complete and the main objectives it activates active:
    /// the official fresh career after its prologue (E742), where the garage
    /// is entered on the PageDown prompt rather than on arrival.
    PostIntro,
}
struct Args {
    name: String,
    directory: PathBuf,
    policy: PathBuf,
    content: Content,
    progression: Progression,
    /// Explicit progression content; required with files and post-intro.
    progression_content: Option<PathBuf>,
    tester_resources: bool,
}
fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let (mut name, mut directory, mut policy, mut items, mut tables) =
        (None, None, None, None, None);
    let (mut game_dir, mut cache) = (None, None);
    let (mut progression, mut progression_content) = (None, None);
    let mut tester_resources = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("missing option value")?;
        match flag.to_str() {
            Some("--starting-resources") if tester_resources.is_none() => {
                tester_resources = Some(match value.to_str() {
                    Some("standard") => false,
                    Some("tester") => true,
                    _ => return Err("--starting-resources takes standard or tester".into()),
                });
            }
            Some("--name") if name.is_none() => {
                name = Some(value.into_string().map_err(|_| "name must be UTF-8")?)
            }
            Some("--state-directory") if directory.is_none() => {
                directory = Some(PathBuf::from(value))
            }
            Some("--account-policy") if policy.is_none() => policy = Some(PathBuf::from(value)),
            Some("--item-content") if items.is_none() => items = Some(PathBuf::from(value)),
            Some("--persistent-content") if tables.is_none() => tables = Some(PathBuf::from(value)),
            Some("--progression-content") if progression_content.is_none() => {
                progression_content = Some(PathBuf::from(value))
            }
            Some("--progression") if progression.is_none() => {
                progression = Some(match value.to_str() {
                    Some("fresh") => Progression::Fresh,
                    Some("post-intro") => Progression::PostIntro,
                    _ => return Err("--progression takes fresh or post-intro".into()),
                })
            }
            Some("--game-dir") if game_dir.is_none() => game_dir = Some(PathBuf::from(value)),
            Some("--content-cache") if cache.is_none() => cache = Some(PathBuf::from(value)),
            _ => return Err("unknown or repeated account creation option".into()),
        }
    }
    let content = match (items, tables, game_dir) {
        (Some(items), Some(tables), None) if cache.is_none() => Content::Files { items, tables },
        (None, None, Some(game_dir)) if progression_content.is_none() => {
            Content::Game { game_dir, cache }
        }
        _ => {
            return Err(
                "pass --item-content and --persistent-content (plus --progression-content for --progression post-intro), or --game-dir (optionally with --content-cache)"
                    .into(),
            );
        }
    };
    let progression = progression.unwrap_or(Progression::Fresh);
    let tester_resources = tester_resources.unwrap_or(false);
    if tester_resources && !matches!(content, Content::Game { .. }) {
        return Err(
            "tester starting resources require --game-dir for installed REP thresholds".into(),
        );
    }
    if matches!(content, Content::Files { .. })
        && (progression == Progression::PostIntro) != progression_content.is_some()
    {
        return Err(
            "--progression post-intro needs --progression-content, which nothing else uses".into(),
        );
    }
    Ok(Args {
        name: name.ok_or("--name is required")?,
        directory: directory.ok_or("--state-directory is required")?,
        policy: policy.ok_or("--account-policy is required")?,
        content,
        progression,
        progression_content,
        tester_resources,
    })
}
fn run(
    args: Args,
    options: &nfs_content::Options,
) -> Result<nfs_storage::AccountId, Box<dyn std::error::Error>> {
    let policy = Policy::load(&args.policy)?;
    let (items, tables, progression_content, thresholds) = match &args.content {
        Content::Files { items, tables } => (
            items.clone(),
            tables.clone(),
            args.progression_content.clone(),
            None,
        ),
        Content::Game { game_dir, cache } => {
            let generated = crate::install_content::resolve(
                std::path::Path::new(""),
                game_dir,
                cache.as_deref(),
                options,
            )?;
            (
                generated.item_content,
                generated.persistent_content,
                Some(generated.progression_content),
                Some(generated.reputation_thresholds),
            )
        }
    };
    let items = ItemContent::load(&items)?;
    let items = items
        .inventory()
        .ok_or("item content must contain an initialization policy")?;
    let fresh_items = if args.progression == Progression::Fresh {
        Some(prologue_inventory(items)?)
    } else {
        None
    };
    let items = fresh_items.as_ref().unwrap_or(items);
    let tables = persistent::Catalog::load(&tables)?;
    let objectives = match args.progression {
        Progression::Fresh => Vec::new(),
        Progression::PostIntro => {
            let content = nfs_content::progression::Content::load(
                progression_content
                    .as_deref()
                    .ok_or("post-intro progression needs progression content")?,
            )
            .map_err(|e| format!("progression content: {e:?}"))?;
            content.catalog.post_intro(content.settings)
        }
    };
    let now = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let mut prepared = Prepared::new(
        &args.name,
        fresh::<SEED_BYTES>(&mut OsSeeds)?,
        nfs_storage::Timestamp(now),
        &policy,
        items,
        &tables,
        &objectives,
    )?;
    let starting_level = if args.tester_resources {
        let thresholds = nfs_services::reputation::Thresholds::load(
            thresholds
                .as_deref()
                .ok_or("missing installed REP thresholds")?,
        )?;
        let level = tester_level(&thresholds)?;
        prepared = prepared.with_starting_resources(
            &tables,
            &thresholds,
            level,
            nfs_services::bank::Balance(TESTER_CASH),
        )?;
        Some(level)
    } else {
        None
    };
    prepared.publish(&args.directory)?;
    let progression = match args.progression {
        Progression::Fresh => "fresh",
        Progression::PostIntro => "post-intro",
    };
    println!(
        "{}",
        serde_json::json!({"created":true,"local_account":prepared.identity().storage_account().hex(),"schema_version":nfs_storage::sqlite::VERSION,"progression":progression,"objective_rows":objectives.len(),"starting_level":starting_level,"starting_cash":starting_level.map(|_| TESTER_CASH)})
    );
    Ok(prepared.identity().storage_account())
}
const TESTER_CASH: u32 = 10_000_000;

fn prologue_inventory(
    items: &nfs_world::items::InventoryCatalog,
) -> Result<nfs_world::items::InventoryCatalog, Box<dyn std::error::Error>> {
    Ok(nfs_services::fresh_account::prologue_inventory(items)?)
}

fn tester_level(thresholds: &nfs_services::reputation::Thresholds) -> Result<u32, &'static str> {
    thresholds
        .maximum_level()
        .checked_sub(1)
        .filter(|level| *level > 0)
        .ok_or("installed REP levels do not contain a level below the cap")
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
        Ok(_) => std::process::ExitCode::SUCCESS,
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
    fn tester_resources_are_explicit_and_use_one_level_below_the_installed_cap() {
        use nfs_services::reputation::Thresholds;
        assert_eq!(
            tester_level(&Thresholds::new(vec![0, 100, 300]).unwrap()),
            Ok(2)
        );
        assert!(tester_level(&Thresholds::new(vec![0]).unwrap()).is_err());
        let base: Vec<OsString> = [
            "--name",
            "Driver",
            "--state-directory",
            "new",
            "--account-policy",
            "policy.json",
            "--game-dir",
            "installation",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        assert!(!parse(base.clone().into_iter()).unwrap().tester_resources);
        let mut tester = base.clone();
        tester.extend(["--starting-resources".into(), "tester".into()]);
        assert!(parse(tester.clone().into_iter()).unwrap().tester_resources);
        tester.extend(["--starting-resources".into(), "standard".into()]);
        assert!(parse(tester.into_iter()).is_err());
        let mut unknown = base;
        unknown.extend(["--starting-resources".into(), "unknown".into()]);
        assert!(parse(unknown.into_iter()).is_err());
        let files = [
            "--name",
            "Driver",
            "--state-directory",
            "new",
            "--account-policy",
            "policy.json",
            "--item-content",
            "items.json",
            "--persistent-content",
            "tables.json",
            "--starting-resources",
            "tester",
        ];
        assert!(parse(files.into_iter().map(Into::into)).is_err());
    }
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
        // Progression defaults to fresh; post-intro needs progression content
        // with explicit files, which nothing else may pass, and derives it
        // from an installation.
        let parsed = parse(args.clone().into_iter()).unwrap();
        assert_eq!(parsed.progression, Progression::Fresh);
        assert!(parsed.progression_content.is_none());
        let mut post = args.clone();
        post.extend(["--progression".into(), "post-intro".into()]);
        assert!(parse(post.clone().into_iter()).is_err());
        post.extend(["--progression-content".into(), "progression.json".into()]);
        let parsed = parse(post.into_iter()).unwrap();
        assert_eq!(parsed.progression, Progression::PostIntro);
        assert_eq!(
            parsed.progression_content.as_deref(),
            Some(std::path::Path::new("progression.json"))
        );
        let mut unused = args.clone();
        unused.extend(["--progression-content".into(), "progression.json".into()]);
        assert!(parse(unused.into_iter()).is_err());
        let mut unknown = args.clone();
        unknown.extend(["--progression".into(), "finished".into()]);
        assert!(parse(unknown.into_iter()).is_err());
        let mut game_post = game.clone();
        game_post.extend(["--progression".into(), "post-intro".into()]);
        assert_eq!(
            parse(game_post.into_iter()).unwrap().progression,
            Progression::PostIntro
        );
        let mut game_explicit = game.clone();
        game_explicit.extend(["--progression-content".into(), "progression.json".into()]);
        assert!(parse(game_explicit.into_iter()).is_err());
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
            policy: policy.clone(),
            content: Content::Game {
                game_dir: dir.join("game"),
                cache: Some(dir.join("cache")),
            },
            progression: Progression::Fresh,
            progression_content: None,
            tester_resources: false,
        };
        let fresh = run(args, &options).unwrap();
        assert!(dir.join("account").is_dir());
        let fresh_repo =
            nfs_storage::SqliteRepository::open_owned_directory(&dir.join("account")).unwrap();
        let fresh_snapshot =
            nfs_storage::InventoryRepository::snapshot(&fresh_repo, fresh).unwrap();
        assert_eq!(
            fresh_snapshot.garage,
            Some(nfs_storage::GarageSlots::default())
        );
        assert_eq!(std::fs::read_dir(dir.join("cache")).unwrap().count(), 1);
        // A post-intro account from the same installation stores the intro
        // objectives completed, read back through the generated catalogs.
        let args = Args {
            name: "Synthetic Driver".into(),
            directory: dir.join("post-intro"),
            policy,
            content: Content::Game {
                game_dir: dir.join("game"),
                cache: Some(dir.join("cache")),
            },
            progression: Progression::PostIntro,
            progression_content: None,
            tester_resources: false,
        };
        let account = run(args, &options).unwrap();
        let generated = crate::install_content::resolve(
            std::path::Path::new(""),
            &dir.join("game"),
            Some(&dir.join("cache")),
            &options,
        )
        .unwrap();
        let progression =
            crate::progression::Content::load(&generated.progression_content).unwrap();
        let tables = persistent::Catalog::load(&generated.persistent_content).unwrap();
        let repository =
            nfs_storage::SqliteRepository::open_owned_directory(&dir.join("post-intro")).unwrap();
        let snapshot = nfs_storage::InventoryRepository::snapshot(&repository, account).unwrap();
        let item_content = ItemContent::load(&generated.item_content).unwrap();
        let catalog = item_content.inventory().unwrap();
        assert!(
            fresh_snapshot.items.values().all(|item| catalog
                .bindings()
                .class(&item.definition.0)
                .unwrap()
                != nfs_world::items::DefinitionClass::RaceVehicleItemData)
        );
        assert!(
            snapshot.items.values().any(|item| catalog
                .bindings()
                .class(&item.definition.0)
                .unwrap()
                == nfs_world::items::DefinitionClass::RaceVehicleItemData)
        );
        let restored = progression
            .catalog
            .restore(&tables.view(&snapshot).unwrap(), progression.settings)
            .unwrap();
        let intro = progression
            .catalog
            .objective_ids(crate::progression::Branch::Intro);
        assert!(!intro.is_empty());
        for id in &intro {
            assert_eq!(
                restored
                    .branch(crate::progression::Branch::Intro)
                    .objectives()[id],
                crate::progression::Objective {
                    active: false,
                    completed: true
                }
            );
        }
        assert!(
            restored
                .branch(crate::progression::Branch::Main)
                .objectives()
                .values()
                .all(|o| !o.completed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
