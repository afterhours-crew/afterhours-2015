// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windows: the session job, registration lookup, process conflicts and the
//! helpers the routed game needs.

use super::{ChildProcess, Platform, SessionGuard, SpawnSpec, child::StdChild};
use crate::exit::{Failure, Result};
use nfs_afterhours_sys::windows as sys;
use std::time::{Duration, Instant};

pub mod lease;

/// The Windows platform.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPlatform;

/// The session job object; see [`super`] for why it exists.
pub struct JobGuard {
    job: sys::Job,
}

impl SessionGuard for JobGuard {
    /// Disarm only after Windows reports that every child has exited. A
    /// signalled process handle can precede removal from the job's list, so
    /// wait briefly for the accounting to catch up. On error the job stays
    /// armed and process exit performs crash cleanup.
    fn finish(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let ids = self.job.process_ids()?;
            if ids == [std::process::id()] {
                break;
            }
            if Instant::now() > deadline {
                return Err(Failure::session("Session still has live children"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        self.job.disarm()?;
        Ok(())
    }
}

impl JobGuard {
    /// Create a new session job and place this process in it. Windows nests
    /// it under any inherited job, preserving the ancestor's restrictions.
    /// Kill-on-close applies only to our job and its descendants; we never
    /// adopt or change an inherited job. Incompatible restrictions fail here
    /// before any session children are spawned.
    pub fn attach() -> Result<JobGuard> {
        let job = sys::Job::attach_current_process().map_err(|error| {
            Failure::refused(format!(
                "Windows could not create an isolated Afterhours session job: {error}. \
                 An inherited job may have incompatible restrictions. \
                 No game or server has been started."
            ))
        })?;
        Ok(JobGuard { job })
    }

    /// Whether a process is in this session's job.
    pub fn owns(&self, pid: u32) -> Result<bool> {
        let process = sys::Process::open(pid, false)?;
        Ok(self.job.contains(&process)?)
    }
}

impl Platform for WindowsPlatform {
    fn refuse_privileged(&self) -> Result<()> {
        if nfs_afterhours_sys::is_privileged()? {
            return Err(Failure::refused(
                "Run nfs-afterhours from an ordinary, non-administrator terminal.",
            ));
        }
        Ok(())
    }
    fn registered_client_path(&self) -> Result<Option<String>> {
        Ok(sys::local_machine_string_32(
            "SOFTWARE\\Origin",
            "ClientPath",
        )?)
    }
    fn running_processes(&self, names: &[&str]) -> Result<Vec<String>> {
        let mut found: Vec<String> = sys::processes_named(names)?
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        found.sort();
        found.dedup();
        Ok(found)
    }
    fn process_alive(&self, pid: u32) -> bool {
        sys::process_exists(pid)
    }
    fn attach_session_guard(&self) -> Result<Box<dyn SessionGuard>> {
        Ok(Box::new(JobGuard::attach()?))
    }
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn ChildProcess>> {
        Ok(Box::new(StdChild::spawn(spec)?))
    }
}

/// Whether a process in this session's job shows a visible window for the
/// installed activation helper. Observes only; never interacts with it.
pub fn activation_window_visible(guard: &JobGuard, helper: &std::path::Path) -> Result<bool> {
    let stem = helper
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Failure::internal("activation helper has no file name"))?;
    for (pid, _) in sys::processes_named(&[stem])? {
        // The helper may exit during observation; treat lookup failures as
        // "not visible" rather than errors.
        let Ok(process) = sys::Process::open(pid, false) else {
            continue;
        };
        let owned = guard.job.contains(&process).unwrap_or(false);
        let same_path = process
            .image_path()
            .and_then(|p| activation_image_matches(&p, helper).map_err(std::io::Error::other))
            .unwrap_or(false);
        if owned && same_path && sys::has_visible_window(pid).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn activation_image_matches(image: &std::path::Path, helper: &std::path::Path) -> Result<bool> {
    // ACTIVATION_HELPER uses a portable slash; Path::join preserves it inside
    // that component. Windows process queries return backslashes instead.
    // Normalize lexically without following links or weakening job ownership.
    let helper = crate::paths::normalize(helper)?;
    Ok(image
        .to_string_lossy()
        .eq_ignore_ascii_case(&helper.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn activation_helper_join_matches_windows_image_path() {
        let helper = Path::new(r"F:\Games\Example Game").join(super::super::ACTIVATION_HELPER);
        let image = Path::new(r"f:\games\example game\Core\ActivationUI.exe");
        assert!(activation_image_matches(image, &helper).unwrap());
        assert!(
            !activation_image_matches(Path::new(r"F:\Other\Core\ActivationUI.exe"), &helper)
                .unwrap()
        );
        assert!(
            !activation_image_matches(Path::new(r"F:\Games\Example Game\Core\Other.exe"), &helper)
                .unwrap()
        );
    }
}
