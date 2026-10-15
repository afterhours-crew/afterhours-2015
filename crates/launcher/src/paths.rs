// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Rules for where the launcher may write.
//!
//! The data directory is the only place the launcher creates files. These
//! checks refuse locations where a write could land somewhere unexpected:
//! a drive or filesystem root, a network share, the game installation, or
//! anywhere reached through a junction or symbolic link. Links are refused
//! rather than followed because following one could lead into an
//! installation, a shared directory or another user's saved state.
//!
//! The server performs its own, similar validation in `nfs-server`'s
//! `local_setup` before it writes. The duplication is deliberate: each
//! program validates its own inputs at its own trust boundary.

use crate::exit::{Failure, Result};
use std::path::{Component, Path, PathBuf};

/// Upper bound on entries inspected while walking an existing data tree.
pub const TREE_INSPECTION_BOUND: usize = 100_000;

/// Absolute, lexically normalized path: `.` and `..` resolved, no trailing
/// separator. Nothing on disk is consulted, so this works for paths that do
/// not exist yet.
pub fn normalize(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Never pop a prefix or the root; `C:\..` stays `C:\`.
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

/// Whether the path is a UNC share. Always false where prefixes do not exist.
fn is_network_share(path: &Path) -> bool {
    match path.components().next() {
        Some(Component::Prefix(prefix)) => matches!(
            prefix.kind(),
            std::path::Prefix::UNC(..) | std::path::Prefix::VerbatimUNC(..)
        ),
        _ => false,
    }
}

/// Whether metadata describes a junction, mount point or symbolic link.
pub fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT covers junctions and symbolic links.
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn same_or_inside(candidate: &Path, container: &Path) -> bool {
    #[cfg(windows)]
    {
        // Windows paths compare case-insensitively.
        let c = candidate.to_string_lossy().to_lowercase();
        let g = container.to_string_lossy().to_lowercase();
        c == g || c.starts_with(&format!("{g}\\")) || c.starts_with(&format!("{g}/"))
    }
    #[cfg(not(windows))]
    {
        candidate == container || candidate.starts_with(container)
    }
}

/// Validate a writable path and return it normalized.
///
/// `game` is the installation the path must stay out of; `None` when the
/// command has no installation in hand, such as `status`.
pub fn checked_data_path(data: &Path, game: Option<&Path>) -> Result<PathBuf> {
    let data = normalize(data)?;
    if data.parent().is_none() || is_network_share(&data) {
        return Err(Failure::refused(
            "Choose a dedicated local data directory, not a drive root or network share.",
        ));
    }
    if let Some(game) = game {
        let game = normalize(game)?;
        if same_or_inside(&data, &game) {
            return Err(Failure::refused(
                "Choose a data directory outside the game installation.",
            ));
        }
    }
    // Every existing ancestor must be an ordinary directory.
    for ancestor in data.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if is_link(&meta) || !meta.is_dir() => {
                return Err(Failure::refused(
                    "Data path must contain only ordinary directories.",
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(data)
}

/// [`checked_data_path`] plus a bounded walk of the existing tree, refusing
/// any link anywhere under it.
pub fn checked_data_tree(data: &Path, game: Option<&Path>) -> Result<PathBuf> {
    let data = checked_data_path(data, game)?;
    if !data.exists() {
        return Ok(data);
    }
    let mut pending = vec![data.clone()];
    let mut count = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            count += 1;
            if count > TREE_INSPECTION_BOUND {
                return Err(Failure::refused(format!(
                    "Data directory exceeds the {TREE_INSPECTION_BOUND}-entry inspection bound."
                )));
            }
            let meta = std::fs::symlink_metadata(entry.path())?;
            if is_link(&meta) {
                return Err(Failure::refused(
                    "Data directory contains a junction or symbolic link; use ordinary files and directories.",
                ));
            }
            if meta.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(data)
}

/// Per-session directory under the data directory.
pub fn session_dir(data: &Path, id: &str) -> PathBuf {
    data.join("artifacts").join("launcher").join(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_resolves_dots_without_touching_disk() {
        let base = crate::testing::scratch("normalize");
        let nested = base.join("a").join("..").join("b").join(".").join("c");
        assert_eq!(normalize(&nested).unwrap(), base.join("b").join("c"));
        let root = normalize(Path::new("/")).unwrap();
        assert!(root.parent().is_none());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn roots_installation_and_files_in_the_path_are_refused() {
        let base = crate::testing::scratch("refuse");
        let game = base.join("Game");
        std::fs::create_dir(&game).unwrap();
        let root: PathBuf = normalize(&base)
            .unwrap()
            .ancestors()
            .last()
            .unwrap()
            .to_path_buf();
        assert!(checked_data_path(&root, Some(&game)).is_err());
        assert!(checked_data_path(&game, Some(&game)).is_err());
        assert!(checked_data_path(&game.join("saves"), Some(&game)).is_err());
        assert!(checked_data_path(&base.join("data"), Some(&game)).is_ok());
        // A missing nested path is fine; the ancestors that exist are checked.
        assert!(checked_data_path(&base.join("data/deeper/still"), Some(&game)).is_ok());
        assert!(!base.join("data").exists());
        std::fs::write(base.join("ordinary-file"), b"sentinel").unwrap();
        assert!(checked_data_path(&base.join("ordinary-file/child"), Some(&game)).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn spaces_and_non_ascii_are_accepted_and_returned_normalized() {
        let base = crate::testing::scratch("unicode");
        let data = base.join("data space å");
        let checked = checked_data_path(&data, None).unwrap();
        assert_eq!(checked, normalize(&data).unwrap());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn links_in_the_path_or_under_the_tree_are_refused() {
        let base = crate::testing::scratch("links");
        let game = base.join("Game");
        let data = base.join("data");
        std::fs::create_dir(&game).unwrap();
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(data.join("ok")).unwrap();
        assert!(checked_data_tree(&data, Some(&game)).is_ok());
        std::os::unix::fs::symlink(&game, data.join("ok").join("linked")).unwrap();
        assert!(
            checked_data_path(&data, Some(&game)).is_ok(),
            "the path itself is clean"
        );
        assert!(
            checked_data_tree(&data, Some(&game)).is_err(),
            "the tree is not"
        );
        std::os::unix::fs::symlink(&base, base.join("alias")).unwrap();
        assert!(checked_data_path(&base.join("alias").join("data"), Some(&game)).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_case_insensitively_and_shares_are_refused() {
        let base = crate::testing::scratch("windows");
        let game = base.join("Game");
        std::fs::create_dir(&game).unwrap();
        assert!(checked_data_path(&base.join("GAME").join("x"), Some(&game)).is_err());
        assert!(checked_data_path(Path::new(r"\\server\share\data"), Some(&game)).is_err());
        assert!(checked_data_path(Path::new(r"C:\"), Some(&game)).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }
}
