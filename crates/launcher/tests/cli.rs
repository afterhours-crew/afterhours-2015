// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The built `nfs-afterhours` binary, driven as a user would.
//!
//! These tests cover the command line itself: help, usage errors, exit
//! codes, `status`, `stop` and `recover` against a data directory, and on
//! Unix a complete `serve` session supervised over a stand-in server
//! script. No game installation is needed; the one check that requires the
//! real game digest is exercised by its refusal.

#[cfg(unix)]
use std::path::Path;
use std::{
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "afterhours-cli-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // Windows canonical paths carry a `\\?\` prefix that other programs
    // cannot open; the binary under test is handed plain paths like a user would.
    let canonical = dir.canonicalize().unwrap();
    let text = canonical.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(stripped) => PathBuf::from(stripped),
        None => canonical,
    }
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nfs-afterhours"))
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn help_lists_every_public_subcommand_and_the_exit_table() {
    let out = run(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = text(&out.stdout);
    for sub in [
        "check", "setup", "serve", "start", "status", "stop", "recover",
    ] {
        assert!(
            help.contains(&format!("\n  {sub}")),
            "{sub} missing from:\n{help}"
        );
    }
    assert!(
        !help.contains("\n  supervise"),
        "internal command stays hidden"
    );
    assert!(help.contains("Exit codes:"));
    let sub = run(&["start", "--help"]);
    assert_eq!(sub.status.code(), Some(0));
    assert!(text(&sub.stdout).contains("--game-dir"));
    assert!(text(&sub.stdout).contains("Windows only"));
    let version = run(&["--version"]);
    assert!(text(&version.stdout).starts_with("nfs-afterhours "));
}

#[test]
fn usage_errors_exit_with_code_two_on_stderr() {
    let out = run(&["start"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--game-dir"));
    let out = run(&["frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn status_and_stop_on_an_unused_data_directory() {
    let data = scratch("unused");
    let out = run(&["--data-dir", data.to_str().unwrap(), "status"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).contains("No Afterhours session"));
    let out = run(&["--data-dir", data.to_str().unwrap(), "--json", "status"]);
    assert_eq!(text(&out.stdout).trim(), r#"{"state":"none"}"#);
    let out = run(&["--data-dir", data.to_str().unwrap(), "stop"]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn check_refuses_a_missing_installation_without_writing() {
    let data = scratch("check");
    let server = data.join("fake-server");
    std::fs::write(&server, b"x").unwrap();
    let game = data.join("no-game");
    let out = run(&[
        "--data-dir",
        data.join("store").to_str().unwrap(),
        "check",
        "--game-dir",
        game.to_str().unwrap(),
        "--server",
        server.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("not found"));
    std::fs::create_dir(&game).unwrap();
    std::fs::write(game.join("NFS16.exe"), b"wrong build").unwrap();
    let out = run(&[
        "--data-dir",
        data.join("store").to_str().unwrap(),
        "check",
        "--game-dir",
        game.to_str().unwrap(),
        "--server",
        server.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("Unsupported installed file"));
    assert!(!data.join("store").exists(), "check never writes");
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn recover_restores_a_pointer_only_for_a_real_database() {
    let data = scratch("recover");
    let id = "c".repeat(32);
    let out = run(&[
        "--data-dir",
        data.to_str().unwrap(),
        "recover",
        "--account-id",
        "bad",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let out = run(&[
        "--data-dir",
        data.to_str().unwrap(),
        "recover",
        "--account-id",
        &id,
    ]);
    assert_eq!(out.status.code(), Some(4), "never ran a session here");
    std::fs::write(data.join("launcher.lock"), b"").unwrap();
    let account = data.join("artifacts").join("account");
    std::fs::create_dir_all(&account).unwrap();
    std::fs::write(account.join(format!("{id}.sqlite")), b"sqlite").unwrap();
    let out = run(&[
        "--data-dir",
        data.to_str().unwrap(),
        "--json",
        "recover",
        "--account-id",
        &id,
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains(&format!("\"local_account\":\"{id}\"")));
    assert!(data.join("account.json").exists());
    std::fs::remove_dir_all(data).unwrap();
}

/// A stand-in `nfs-server`: answers `prepare-local` and `create-account`
/// with the JSON shapes the real server prints, and otherwise prints a
/// readiness record and waits for its `--stop-file`.
#[cfg(unix)]
fn fake_server(dir: &Path, data: &Path) -> PathBuf {
    let policy = dir.join("account-policy.json");
    std::fs::write(&policy, b"{}").unwrap();
    let script = dir.join("nfs-server");
    let body = format!(
        r#"#!/bin/sh
case "$1" in
  prepare-local)
    printf '{{"prepared":true,"files":{{"account-policy":"{policy}","world-content":"{dir}/world.json"}}}}\n'
    exit 0 ;;
  create-account)
    mkdir -p "{data}/artifacts/account"
    printf '{{"created":true,"local_account":"0123456789abcdef0123456789abcdef"}}\n'
    exit 0 ;;
esac
stop=""
while [ $# -gt 0 ]; do
  if [ "$1" = "--stop-file" ]; then stop="$2"; fi
  shift
done
echo "fake server starting" >&2
printf '{{"ready":true,"pid":%s,"redirector":"127.0.0.1:34567","lsx":null}}\n' "$$"
while [ ! -e "$stop" ]; do sleep 0.1; done
echo "fake server stopping" >&2
exit 0
"#,
        policy = policy.display(),
        dir = dir.display(),
        data = data.display(),
    );
    std::fs::write(&script, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[cfg(unix)]
fn sha256(path: &Path) -> String {
    use sha2::Digest;
    let bytes = std::fs::read(path).unwrap();
    sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(unix)]
#[test]
fn a_serve_session_prepares_runs_reports_and_stops() {
    let base = scratch("serve");
    let data = base.join("data");
    let game = base.join("game");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&game).unwrap();
    let server = fake_server(&base, &data);
    let id = "d".repeat(32);
    let run_dir = data.join("artifacts").join("launcher").join(&id);
    std::fs::create_dir_all(&run_dir).unwrap();
    // The front-end would write this after preflight; the fake installation
    // cannot pass the game digest, so the test writes the request directly.
    let request = serde_json::json!({
        "version": 1, "id": id, "action": "serve",
        "game_directory": game, "data_directory": data,
        "server_path": server, "server_sha256": sha256(&server),
        "player_name": "Tester", "record_protocol": false
    });
    std::fs::write(run_dir.join("request.json"), request.to_string()).unwrap();
    let data_arg = data.to_str().unwrap().to_owned();
    let mut supervisor = bin()
        .args([
            "supervise",
            "--request",
            run_dir.join("request.json").to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Wait for the ready state as a separate process would, through `status`.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let ready = loop {
        let out = run(&["--data-dir", &data_arg, "--json", "status"]);
        let stdout = text(&out.stdout);
        if stdout.contains("\"state\":\"ready\"") {
            break stdout;
        }
        if stdout.contains("\"state\":\"failed\"") || std::time::Instant::now() > deadline {
            let _ = supervisor.kill();
            let out = supervisor.wait_with_output().unwrap();
            panic!(
                "session did not become ready: {stdout}\nstdout: {}\nstderr: {}",
                text(&out.stdout),
                text(&out.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(ready.contains("\"server_pid\":"), "{ready}");
    assert!(
        data.join("account.json").exists(),
        "account pointer written"
    );
    assert!(data.join("current-session.json").exists());
    let stop = run(&["--data-dir", &data_arg, "stop"]);
    assert_eq!(stop.status.code(), Some(0), "{}", text(&stop.stderr));
    assert!(text(&stop.stdout).contains("Requested"));
    let out = supervisor.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let console = text(&out.stdout);
    for state in ["preparing", "ready", "stopping", "finished"] {
        assert!(
            console.contains(&format!("] {state} :")),
            "{state} missing from console:\n{console}"
        );
    }
    assert!(
        console.contains("fake server starting"),
        "server diagnostics are echoed"
    );
    let status = run(&["--data-dir", &data_arg, "status"]);
    assert!(
        text(&status.stdout).starts_with("finished :"),
        "{}",
        text(&status.stdout)
    );
    let again = run(&["--data-dir", &data_arg, "stop"]);
    assert_eq!(again.status.code(), Some(0));
    assert!(text(&again.stdout).contains("account data is preserved"));
    assert!(run_dir.join("server.stdout.jsonl").exists());
    assert!(run_dir.join("prepare.stdout.json").exists());
    std::fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn a_session_whose_supervisor_dies_is_reported_interrupted() {
    let base = scratch("interrupted");
    let data = base.join("data");
    let game = base.join("game");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&game).unwrap();
    let server = fake_server(&base, &data);
    let id = "e".repeat(32);
    let run_dir = data.join("artifacts").join("launcher").join(&id);
    std::fs::create_dir_all(&run_dir).unwrap();
    let request = serde_json::json!({
        "version": 1, "id": id, "action": "serve",
        "game_directory": game, "data_directory": data,
        "server_path": server, "server_sha256": sha256(&server),
        "player_name": "Tester", "record_protocol": false
    });
    std::fs::write(run_dir.join("request.json"), request.to_string()).unwrap();
    let data_arg = data.to_str().unwrap().to_owned();
    let mut supervisor = bin()
        .args([
            "supervise",
            "--request",
            run_dir.join("request.json").to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !text(&run(&["--data-dir", &data_arg, "--json", "status"]).stdout)
        .contains("\"state\":\"ready\"")
    {
        assert!(std::time::Instant::now() < deadline, "never ready");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    supervisor.kill().unwrap();
    supervisor.wait().unwrap();
    let out = run(&["--data-dir", &data_arg, "status"]);
    assert!(
        text(&out.stdout).starts_with("interrupted :"),
        "{}",
        text(&out.stdout)
    );
    // The orphaned fake server is still running; stop it by its stop file,
    // which is exactly what the status message tells the user to look at.
    std::fs::write(run_dir.join("stop-server"), b"stop").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::fs::remove_dir_all(base).unwrap();
}
