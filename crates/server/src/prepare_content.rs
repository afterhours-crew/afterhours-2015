// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Prepare installation-derived files without requiring account or service policy.
use nfs_content::{Kind, Options};
use std::{ffi::OsString, path::PathBuf};

const HELP: &str = "Usage: nfs-server prepare-content --game-dir <installation> [--root <data-directory>] [--content-cache <directory>]\n\nChecks the supported executable and prepares verified cached content without\nopening listeners. Relative paths follow --root (default current directory).\nThe default cache is artifacts/content. Prints the cache identity and file paths\nas JSON. Vehicle, garage-logic, world-exit, sequence and progression content and service policy remain separate inputs.";

struct Args {
    root: PathBuf,
    game: PathBuf,
    cache: Option<PathBuf>,
}

fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let (mut root, mut game, mut cache) = (None, None, None);
    while let Some(flag) = args.next() {
        let value = PathBuf::from(args.next().ok_or("missing preparation option value")?);
        match flag.to_str() {
            Some("--root") if root.is_none() => root = Some(value),
            Some("--game-dir") if game.is_none() => game = Some(value),
            Some("--content-cache") if cache.is_none() => cache = Some(value),
            _ => return Err("unknown or repeated preparation option".into()),
        }
    }
    Ok(Args {
        root: root.unwrap_or_else(|| ".".into()),
        game: game.ok_or("--game-dir is required")?,
        cache,
    })
}

fn run(args: &Args, options: &Options) -> Result<serde_json::Value, nfs_content::Error> {
    let generated =
        crate::install_content::resolve(&args.root, &args.game, args.cache.as_deref(), options)?;
    let files: serde_json::Map<String, serde_json::Value> = Kind::ALL
        .into_iter()
        .map(|kind| {
            let path = generated.prepared.path(kind);
            let path = path
                .to_str()
                .ok_or_else(|| nfs_content::Error::Content("prepared path is not UTF-8".into()))?;
            Ok((kind.key().to_owned(), serde_json::json!(path)))
        })
        .collect::<Result<_, nfs_content::Error>>()?;
    Ok(serde_json::json!({"prepared":true, "install_content":generated.summary(), "files":files}))
}

pub fn main(args: impl Iterator<Item = OsString>) -> std::process::ExitCode {
    let mut args = args.peekable();
    if args.peek().is_some_and(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return std::process::ExitCode::SUCCESS;
    }
    let args = match parse(args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n{HELP}");
            return std::process::ExitCode::from(2);
        }
    };
    match run(&args, &Options::default()) {
        Ok(report) => {
            println!("{report}");
            std::process::ExitCode::SUCCESS
        }
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
    fn preparation_requires_only_an_installation_and_rejects_ambiguous_options() {
        let parse = |s: &[&str]| super::parse(s.iter().map(OsString::from));
        let args = parse(&["--game-dir", "game"]).unwrap();
        assert_eq!(args.root, PathBuf::from("."));
        assert_eq!(args.game, PathBuf::from("game"));
        assert!(args.cache.is_none());
        for bad in [
            vec![],
            vec!["--game-dir"],
            vec!["--root", "data"],
            vec!["--game-dir", "a", "--game-dir", "b"],
            vec!["--game-dir", "a", "--root", "x", "--root", "y"],
            vec![
                "--game-dir",
                "a",
                "--content-cache",
                "x",
                "--content-cache",
                "y",
            ],
            vec!["--game-dir", "a", "--local-account", "ignored"],
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn preparation_reports_every_file_and_reuses_the_cache_without_account_state() {
        let root = std::env::temp_dir().join(format!(
            "nfs-prepare-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (options, _) =
            nfs_content::synthetic::write(&root.join("game"), &nfs_content::synthetic::assets())
                .unwrap();
        let args = Args {
            root: root.clone(),
            game: "game".into(),
            cache: Some("chosen-cache".into()),
        };
        let first = run(&args, &options).unwrap();
        assert_eq!(first["prepared"], true);
        assert_eq!(first["install_content"]["built"], true);
        assert_eq!(first["files"].as_object().unwrap().len(), Kind::ALL.len());
        for kind in Kind::ALL {
            let path = PathBuf::from(first["files"][kind.key()].as_str().unwrap());
            assert!(path.is_absolute() && path.starts_with(root.join("chosen-cache")));
            assert!(path.is_file());
        }
        let again = run(&args, &options).unwrap();
        assert_eq!(again["install_content"]["built"], false);
        assert_eq!(again["files"], first["files"]);
        assert!(run(&args, &Options::default()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
