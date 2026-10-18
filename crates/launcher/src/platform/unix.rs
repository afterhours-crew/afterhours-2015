// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Linux and macOS: enough to run `serve`. There is no game here, so no
//! registration, no conflicting game processes and no session job.

use super::{ChildProcess, Platform, SessionGuard, SpawnSpec, child::StdChild};
use crate::exit::{Failure, Result};

/// The Unix platform.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnixPlatform;

struct NoGuard;

impl SessionGuard for NoGuard {
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Platform for UnixPlatform {
    fn refuse_privileged(&self) -> Result<()> {
        if nfs_afterhours_sys::is_privileged()? {
            return Err(Failure::refused(
                "Run nfs-afterhours as an ordinary user, not as root.",
            ));
        }
        Ok(())
    }
    fn registered_client_path(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn running_processes(&self, _names: &[&str]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn process_alive(&self, pid: u32) -> bool {
        nfs_afterhours_sys::process_exists(pid)
    }
    fn attach_session_guard(&self) -> Result<Box<dyn SessionGuard>> {
        Ok(Box::new(NoGuard))
    }
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn ChildProcess>> {
        Ok(Box::new(StdChild::spawn(spec)?))
    }
}
