// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Launcher for the Afterhours 2015 local server and the installed game.
//!
//! The binary `nfs-afterhours` prepares content and an account, starts the
//! server, and on Windows also starts and routes the game. Everything the
//! launcher writes lives in one dedicated data directory, and every file it
//! writes is small, versioned JSON or a verified copy of our own server.
//!
//! The crate is split so that the parts which make decisions can be tested
//! on any platform without a game, a server or an operating-system process:
//!
//! - [`marker`], [`compat`], [`paths`], [`records`], [`digest`], [`install`]
//!   and [`session`] are portable and have no process or platform dependency.
//! - [`platform`] defines the operations the supervisor needs from the
//!   operating system. Implementations live behind `cfg` and are the only
//!   code that spawns processes or reads the registry.
//!
//! See `crates/launcher/README.md` for how the pieces fit together and
//! `docs/LOCAL-LAUNCHER.md` for how to run the game.

pub mod cli;
pub mod commands;
pub mod compat;
pub mod digest;
pub mod exit;
pub mod install;
pub mod marker;
pub mod paths;
pub mod platform;
pub mod records;
pub mod session;

/// Test support shared by the unit tests: unique scratch directories.
#[cfg(test)]
pub(crate) mod testing {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A fresh, empty directory under the system temporary directory, as a
    /// real path. The temporary directory may sit under a symlink (`/var`
    /// on macOS) and the path rules refuse links, so tests use the
    /// canonical form. The name carries the process id and a counter so
    /// tests running in parallel never share a directory.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "afterhours-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }
}
