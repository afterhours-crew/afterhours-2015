// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The verified copy of our own server under the compatibility name.
//!
//! The copy lives at `<data>/bin/<server-sha256>/<compatibility name>`. The
//! digest in the path pins the copy to one exact build; a different build
//! gets a different directory and an existing directory is never rewritten.
//! A copy that no longer matches its own digest is refused rather than
//! replaced, so a changed executable is something the user sees, not
//! something the launcher papers over.

use crate::{
    digest,
    exit::{Failure, Result},
    paths,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// Inputs the copy depends on, all established during preflight.
#[derive(Debug, Clone)]
pub struct Source<'a> {
    pub data_directory: &'a Path,
    pub game_directory: Option<&'a Path>,
    pub server_path: &'a Path,
    pub server_sha256: &'a str,
    pub compatibility_name: &'a str,
}

/// Ensure the compatibility copy exists and matches, returning its path.
pub fn install_compatibility_server(source: &Source<'_>) -> Result<PathBuf> {
    if !digest::is_sha256_hex(source.server_sha256) {
        return Err(Failure::refused("Invalid server digest."));
    }
    let data = paths::checked_data_path(source.data_directory, source.game_directory)?;
    let directory = data.join("bin").join(source.server_sha256);
    paths::checked_data_path(&directory, source.game_directory)?;
    std::fs::create_dir_all(&directory)?;
    paths::checked_data_path(&directory, source.game_directory)?;
    let target = directory.join(source.compatibility_name);
    match std::fs::symlink_metadata(&target) {
        Ok(meta) => {
            if paths::is_link(&meta) {
                return Err(Failure::refused(
                    "Refusing linked compatibility executable.",
                ));
            }
            if digest::sha256_file(&target)? != source.server_sha256 {
                return Err(Failure::refused(
                    "Existing compatibility executable has changed; choose a new data directory or inspect it.",
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if digest::sha256_file(source.server_path)? != source.server_sha256 {
                return Err(Failure::refused("Server changed after preflight."));
            }
            copy_verified(
                source.server_path,
                &directory,
                &target,
                source.server_sha256,
            )?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(target)
}

fn copy_verified(from: &Path, directory: &Path, target: &Path, expected: &str) -> Result<()> {
    let temporary = directory.join(format!(
        "{}-{}.partial",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let result = (|| -> Result<()> {
        let mut source = std::fs::File::open(from)?;
        let mut destination = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        std::io::copy(&mut source, &mut destination)?;
        destination.flush()?;
        destination.sync_all()?;
        drop(destination);
        if digest::sha256_file(&temporary)? != expected {
            return Err(Failure::refused(
                "Compatibility server copy failed its digest check.",
            ));
        }
        if target.exists() {
            return Err(Failure::refused(
                "Compatibility executable appeared during the copy; retry.",
            ));
        }
        std::fs::rename(&temporary, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_is_verified_repeatable_and_never_overwrites_a_changed_file() {
        let base = crate::testing::scratch("install");
        let game = base.join("game");
        let data = base.join("data space å");
        std::fs::create_dir_all(&game).unwrap();
        let server = base.join("server.exe");
        std::fs::write(&server, [1, 2, 3, 4]).unwrap();
        let sha = digest::sha256_file(&server).unwrap();
        let source = Source {
            data_directory: &data,
            game_directory: Some(&game),
            server_path: &server,
            server_sha256: &sha,
            compatibility_name: "EADesktopDebug.exe",
        };
        let copy = install_compatibility_server(&source).unwrap();
        assert_eq!(
            copy,
            paths::normalize(&data)
                .unwrap()
                .join("bin")
                .join(&sha)
                .join("EADesktopDebug.exe")
        );
        assert_eq!(std::fs::read(&copy).unwrap(), [1, 2, 3, 4]);
        assert_eq!(
            install_compatibility_server(&source).unwrap(),
            copy,
            "repeat preparation"
        );
        assert!(
            std::fs::read_dir(copy.parent().unwrap()).unwrap().count() == 1,
            "no partial file remains"
        );
        std::fs::write(&copy, [9]).unwrap();
        assert!(install_compatibility_server(&source).is_err());
        assert_eq!(
            std::fs::read(&copy).unwrap()[0],
            9,
            "never overwrite a changed existing file"
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn bad_digests_and_changed_sources_are_refused_before_copying() {
        let base = crate::testing::scratch("install");
        let data = base.join("data");
        let server = base.join("server.exe");
        std::fs::write(&server, [1]).unwrap();
        let mut source = Source {
            data_directory: &data,
            game_directory: None,
            server_path: &server,
            server_sha256: "nothex",
            compatibility_name: "EADesktopDebug.exe",
        };
        assert!(install_compatibility_server(&source).is_err());
        let other = digest::sha256_bytes(b"different build");
        source.server_sha256 = &other;
        assert!(
            install_compatibility_server(&source).is_err(),
            "server changed after preflight"
        );
        assert!(
            !data
                .join("bin")
                .join(&other)
                .join("EADesktopDebug.exe")
                .exists()
        );
        std::fs::remove_dir_all(base).unwrap();
    }
}
