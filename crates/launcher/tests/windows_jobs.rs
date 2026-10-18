// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![cfg(windows)]

use nfs_afterhours::platform::{SessionGuard, windows::JobGuard};
use nfs_afterhours_sys::windows::{Job, Process, Wait, in_job};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ROLE: &str = "AFTERHOURS_JOB_TEST_ROLE";
const ROOT: &str = "AFTERHOURS_JOB_TEST_ROOT";
const MODE: &str = "AFTERHOURS_JOB_TEST_MODE";

struct Worker(Child);

impl Worker {
    fn spawn(role: &str, root: &Path, mode: &str, ids: &[u32]) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "job_process_helper", "--nocapture"])
            .env(ROLE, role)
            .env(ROOT, root)
            .env(MODE, mode);
        for (index, id) in ids.iter().enumerate() {
            command.env(format!("AFTERHOURS_JOB_TEST_PID_{index}"), id.to_string());
        }
        Self(command.spawn().unwrap())
    }

    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "worker did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        // Bound cleanup even when the assertion which brought us here failed.
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn wait_file(path: &Path, worker: &mut Worker) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        assert!(
            worker.0.try_wait().unwrap().is_none(),
            "worker exited early"
        );
        assert!(Instant::now() < deadline, "missing {}", path.display());
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn run_nested_session(mode: &str) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "afterhours-job-{}-{nonce}-{mode}",
        std::process::id()
    ));
    std::fs::create_dir(&root).unwrap();
    // Never attach the test harness itself to a kill-on-close job. Each job
    // owner is a disposable helper; failure cannot terminate unrelated tests.
    let status = Worker::spawn("outer", &root, mode, &[]).wait();
    let completed = root.join("outer-finished").is_file();
    std::fs::remove_dir_all(&root).unwrap();
    assert!(status.success(), "nested {mode} session failed: {status}");
    // Kill-on-close can terminate its owner with exit code zero while the
    // stack is unwinding. Require explicit completion after disarming too.
    assert!(
        completed,
        "nested {mode} session did not complete its assertions"
    );
}

#[test]
fn inherited_job_allows_normal_session_cleanup() {
    run_nested_session("normal");
}

#[test]
fn inherited_job_crash_cleans_only_session_descendants() {
    run_nested_session("crash");
}

// Invoked in separate processes by the tests above; a normal test run is a
// no-op. Only test-owned helpers are ever terminated.
#[test]
fn job_process_helper() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let mode = std::env::var(MODE).unwrap();
    match role.as_str() {
        "outer" => outer_job(&root, &mode),
        "session" => session_job(&root, &mode),
        "idle" => std::thread::sleep(Duration::from_secs(45)),
        _ => panic!("unknown helper role"),
    }
}

fn outer_job(root: &Path, mode: &str) {
    let job = Job::attach_current_process().unwrap();
    let mut sibling = Worker::spawn("idle", root, mode, &[]);
    let mut session = Worker::spawn("session", root, mode, &[std::process::id(), sibling.0.id()]);
    let pid: u32 = wait_file(&root.join("child"), &mut session)
        .parse()
        .unwrap();
    // Retain the handle before terminating the owner; do not infer cleanup
    // from PID absence, since Windows can recycle process identifiers.
    let child = Process::open(pid, false).unwrap();
    assert!(matches!(child.wait(0).unwrap(), Wait::Running));
    if mode == "crash" {
        session.0.kill().unwrap();
        assert!(!session.wait().success());
    } else {
        std::fs::write(root.join("finish"), b"finish").unwrap();
        assert!(session.wait().success());
        assert!(root.join("session-finished").is_file());
    }
    assert!(matches!(child.wait(5_000).unwrap(), Wait::Exited));
    assert!(
        sibling.0.try_wait().unwrap().is_none(),
        "unrelated sibling was killed"
    );
    sibling.0.kill().unwrap();
    sibling.wait();
    let deadline = Instant::now() + Duration::from_secs(5);
    while job.process_ids().unwrap() != [std::process::id()] {
        assert!(Instant::now() < deadline, "outer job retained children");
        std::thread::sleep(Duration::from_millis(10));
    }
    job.disarm().unwrap();
    std::fs::write(root.join("outer-finished"), b"finished").unwrap();
}

fn session_job(root: &Path, mode: &str) {
    assert!(in_job().unwrap(), "regression requires an inherited job");
    let mut guard = JobGuard::attach().unwrap();
    let mut child = Worker::spawn("idle", root, mode, &[]);
    assert!(guard.owns(std::process::id()).unwrap());
    assert!(guard.owns(child.0.id()).unwrap());
    for index in 0..2 {
        let pid = std::env::var(format!("AFTERHOURS_JOB_TEST_PID_{index}"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            !guard.owns(pid).unwrap(),
            "session owns an ancestor or sibling"
        );
    }
    // Publish only after the entire value is written.
    std::fs::write(root.join("child.tmp"), child.0.id().to_string()).unwrap();
    std::fs::rename(root.join("child.tmp"), root.join("child")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !root.join("finish").exists() {
        assert!(Instant::now() < deadline, "parent did not finish session");
        std::thread::sleep(Duration::from_millis(10));
    }
    child.0.kill().unwrap();
    child.wait();
    guard.finish().unwrap();
    std::fs::write(root.join("session-finished"), b"finished").unwrap();
}
