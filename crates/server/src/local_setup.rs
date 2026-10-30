// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Generate portable local service policy and garage lifecycle from an installation.
//! Account creation is a separate transaction and existing accounts are preserved.
mod garage;
mod policy;
mod prepared;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};
const HELP: &str = "Usage: nfs-server prepare-local --game-dir <installation> --root <data-directory>\nGenerates local AllDrive policy and installation-derived content. No game or listeners are started; existing accounts are untouched.";

fn parse(mut args: impl Iterator<Item = OsString>) -> Result<(PathBuf, PathBuf), String> {
    let (mut root, mut game) = (None, None);
    while let Some(flag) = args.next() {
        let path = PathBuf::from(args.next().ok_or("missing preparation value")?);
        match flag.to_str() {
            Some("--root") if root.is_none() => root = Some(path),
            Some("--game-dir") if game.is_none() => game = Some(path),
            _ => return Err("unknown or repeated local preparation option".into()),
        }
    }
    Ok((
        root.ok_or("--root is required")?,
        game.ok_or("--game-dir is required")?,
    ))
}
fn write_new(path: &Path, value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn linked(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn validate_paths(root: &Path, game: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if root.parent().is_none() {
        return Err("choose a dedicated data directory".into());
    }
    // Reject links before creating anything, including a link above a missing
    // data directory. Resolve existing prefixes to catch aliases of the game.
    for path in root.ancestors() {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if linked(&meta) || !meta.is_dir() => {
                return Err("linked or non-directory data path".into());
            }
            Ok(_) => {
                let canonical = path.canonicalize()?;
                let game = game.canonicalize()?;
                #[cfg(windows)]
                let inside = canonical.to_string_lossy().to_lowercase()
                    == game.to_string_lossy().to_lowercase()
                    || canonical
                        .to_string_lossy()
                        .to_lowercase()
                        .starts_with(&(game.to_string_lossy().to_lowercase() + "\\"));
                #[cfg(not(windows))]
                let inside = canonical.starts_with(game);
                if inside {
                    return Err("data directory must be outside the installation".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let mut pending = if root.exists() {
        vec![root.to_path_buf()]
    } else {
        vec![]
    };
    let mut count = 0;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            count += 1;
            if count > 100_000 {
                return Err("data directory exceeds inspection bound".into());
            }
            let meta = std::fs::symlink_metadata(entry.path())?;
            if linked(&meta) {
                return Err("data directory contains a link".into());
            }
            if meta.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}
/// Build configuration in a new, immutable generation directory. Failed runs
/// never replace a previous generation. Files contain no saved account state.
pub fn prepare(root: &Path, game: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let root = std::path::absolute(root)?;
    let game = std::path::absolute(game)?;
    validate_paths(&root, &game)?;
    let generated = crate::install_content::resolve_with_progress(
        &root,
        &game,
        None,
        &nfs_content::Options::default(),
        &mut |progress| eprintln!("[content] {progress}"),
    )?;
    let documents = prepared::documents(&root, &generated, || build_documents(&game, &generated))?;
    publish_documents(&root, &generated, &documents)
}

fn build_documents(
    game: &Path,
    generated: &crate::install_content::Generated,
) -> Result<std::collections::BTreeMap<String, Value>, Box<dyn std::error::Error>> {
    eprintln!("[setup] Reading garage, challenge and world settings...");
    let mut index = nfs_content::assets::AssetIndex::open(game, &nfs_content::Options::default())?;
    let thresholds: Value =
        serde_json::from_slice(&std::fs::read(&generated.reputation_thresholds)?)?;
    eprintln!("[setup] Preparing garage entry, exit and upgrades...");
    let garage = garage::build(&mut index, &thresholds["thresholds"])?;
    eprintln!("[setup] Preparing local server rules and preview challenges...");
    let mut documents = policy::documents();
    documents.insert(
        "challenge-content",
        policy::challenges(&index.challenge_types()?)?,
    );
    documents.insert("garage-logic", garage);
    let mut world: Value = serde_json::from_slice(&std::fs::read(&generated.world_content)?)?;
    eprintln!("[setup] Preparing your free-roam starting location...");
    let services = index.local_world_services("levels/genesis01/gameplay")?;
    let site = |s: nfs_world::world_activation::Site| json!({"scene_key":s.scene_key,"serializer":s.serializer});
    world["roles"]["free_roam"] = json!({"volumes":[],"presence":site(services.presence),"streamed":site(services.streamed),"location":site(services.location)});
    documents.insert("world-content", world);
    Ok(documents
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect())
}

fn publish_documents(
    root: &Path,
    generated: &crate::install_content::Generated,
    documents: &std::collections::BTreeMap<String, Value>,
) -> Result<Value, Box<dyn std::error::Error>> {
    // A digest identifies this exact generated policy, including derived layouts.

    let digest: String = openssl::sha::sha256(&serde_json::to_vec(&documents)?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let directory = root.join("artifacts/local-config").join(digest);
    if directory.exists() {
        eprintln!("[setup] Checking and reusing your local server settings...");
        verify_documents(&directory, documents)?;
    } else {
        eprintln!("[setup] Saving your local server settings...");
        std::fs::create_dir_all(directory.parent().ok_or("configuration parent")?)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temporary =
            directory.with_extension(format!("partial-{}-{nonce}-{serial}", std::process::id()));
        std::fs::create_dir(&temporary)?;
        let temporary = PendingDirectory(temporary);
        for (name, value) in documents {
            write_new(&temporary.0.join(format!("{name}.json")), value)?;
        }
        if let Err(error) = std::fs::rename(&temporary.0, &directory) {
            if !directory.exists() {
                return Err(error.into());
            }
            // Another process may have published the same immutable generation
            // while this one wrote its private staging directory.
            verify_documents(&directory, documents)?;
        }
    }
    let files: serde_json::Map<String, Value> = documents
        .keys()
        .map(|name| {
            (
                (*name).to_owned(),
                json!(directory.join(format!("{name}.json"))),
            )
        })
        .collect();
    eprintln!("[setup] Local server settings ready.");
    Ok(
        json!({"prepared":true,"policy_revision":1,"install_content":generated.summary(),"files":files}),
    )
}

struct PendingDirectory(PathBuf);
impl Drop for PendingDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn verify_documents(
    directory: &Path,
    documents: &std::collections::BTreeMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = std::fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || linked(&metadata) {
        return Err("invalid local configuration directory".into());
    }
    for (name, value) in documents {
        let path = directory.join(format!("{name}.json"));
        let expected = serde_json::to_vec(value)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file()
            || linked(&metadata)
            || metadata.len() != expected.len() as u64
            || std::fs::read(path)? != expected
        {
            return Err("existing local configuration was modified".into());
        }
    }
    Ok(())
}
pub fn main(args: impl Iterator<Item = OsString>) -> std::process::ExitCode {
    let mut args = args.peekable();
    if args.peek().is_some_and(|s| s == "--help" || s == "-h") {
        println!("{HELP}");
        return std::process::ExitCode::SUCCESS;
    }
    let result = parse(args)
        .map_err(|e| e.into())
        .and_then(|(root, game)| prepare(&root, &game));
    match result {
        Ok(report) => {
            println!("{report}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}\n{HELP}");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_rejects_installation_writes_and_linked_data_before_creation() {
        let root = std::env::temp_dir().join(format!(
            "afterhours-path-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let game = root.join("Game");
        let data = root.join("data");
        std::fs::create_dir_all(&game).unwrap();
        assert!(validate_paths(&game.join("missing/child"), &game).is_err());
        assert!(!game.join("missing").exists());
        assert!(validate_paths(&data, &game).is_ok());
        #[cfg(windows)]
        assert!(validate_paths(&root.join("GAME/new"), &game).is_err());
        std::fs::create_dir(&data).unwrap();
        std::fs::write(data.join("file"), b"keep").unwrap();
        assert!(validate_paths(&data.join("file/child"), &game).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&game, data.join("linked")).unwrap();
            assert!(validate_paths(&data, &game).is_err());
        }
        assert_eq!(std::fs::read(data.join("file")).unwrap(), b"keep");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn preparation_refuses_missing_repeated_and_unrelated_options() {
        for args in [
            vec![],
            vec!["--root", "data"],
            vec!["--root", "data", "--game-dir", "game", "--root", "other"],
            vec![
                "--root",
                "data",
                "--game-dir",
                "game",
                "--profile",
                "private",
            ],
        ] {
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
        assert!(
            parse(
                ["--root", "data", "--game-dir", "game"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_ok()
        );
    }
    #[test]
    fn configuration_creation_never_overwrites_existing_bytes() {
        let path = std::env::temp_dir().join(format!(
            "afterhours-policy-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        write_new(&path, &json!({"original":1})).unwrap();
        assert!(write_new(&path, &json!({"replacement":2})).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
            json!({"original":1})
        );
        std::fs::remove_file(path).unwrap();
    }
}
