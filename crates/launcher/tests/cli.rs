// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The built `nfs-afterhours` binary, driven as a user would.
//!
//! These tests cover the command line itself: help, usage errors, exit
//! codes, `status`, `stop`, `profile` and `recover` against a data directory, and on
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
fn terminal_colors_respect_redirection_no_color_and_leave_json_plain() {
    let root = scratch("colors");
    for (force, no_color, expect_color) in [
        (false, false, false),
        (true, false, true),
        (true, true, false),
    ] {
        let mut command = bin();
        command
            .env_remove("CLICOLOR_FORCE")
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR");
        if force {
            command.env("CLICOLOR_FORCE", "1");
        }
        if no_color {
            command.env("NO_COLOR", "1");
        }
        let output = command
            .args(["check", "--game-dir"])
            .arg(root.join("missing-game"))
            .arg("--server")
            .arg(env!("CARGO_BIN_EXE_nfs-afterhours"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = text(&output.stderr);
        assert!(error.contains("Checking your game"));
        assert!(error.contains("ERROR"));
        assert_eq!(error.contains('\u{1b}'), expect_color, "{error:?}");
    }
    let output = bin()
        .env("CLICOLOR_FORCE", "1")
        .env_remove("NO_COLOR")
        .args(["status", "--json", "--data-dir"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(!text(&output.stdout).contains('\u{1b}'));
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn help_lists_every_public_subcommand_and_the_exit_table() {
    let out = run(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = text(&out.stdout);
    for sub in [
        "check",
        "setup",
        "serve",
        "start",
        "status",
        "stop",
        "play",
        "profile",
        "car-images",
        "map-images",
        "sessions",
        "settings",
        "version",
        "schema",
        "recover",
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
    assert_eq!(text(&out.stdout).trim(), r#"{"version":1,"session":null}"#);
    let out = run(&["--data-dir", data.to_str().unwrap(), "stop"]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    let out = run(&["--data-dir", data.to_str().unwrap(), "play", "--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    assert!(out.stdout.is_empty(), "a refused play prints no record");
    assert!(text(&out.stderr).contains("nfs-afterhours serve"));
    assert!(
        std::fs::read_dir(&data).unwrap().next().is_none(),
        "play wrote nothing"
    );
    let out = run(&["--data-dir", data.to_str().unwrap(), "api", "--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    assert!(out.stdout.is_empty(), "no session, no API record");
    assert!(text(&out.stderr).contains("--api"));
    assert!(
        std::fs::read_dir(&data).unwrap().next().is_none(),
        "api wrote nothing"
    );
    std::fs::remove_dir_all(data).unwrap();
}

fn json(out: &Output) -> serde_json::Value {
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn version_reports_records_and_capabilities_and_schema_prints_each_one() {
    let report = json(&run(&["version", "--json"]));
    assert_eq!(report["version"], 1);
    assert_eq!(report["program"], "nfs-afterhours");
    assert_eq!(report["program_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        report["capabilities"],
        serde_json::json!([
            "profile",
            "sessions",
            "events",
            "play",
            "car-images",
            "map-images",
            "world-settings",
            "map-feed",
            "api"
        ])
    );
    assert_eq!(report["os"], std::env::consts::OS);
    let records = report["records"].as_object().unwrap();
    let schemas = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema");
    let mut files: Vec<String> = std::fs::read_dir(&schemas)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    let mut expected: Vec<String> = records
        .keys()
        .map(|name| format!("{name}.schema.json"))
        .collect();
    expected.sort();
    assert_eq!(files, expected, "one committed schema per record");
    for (name, version) in records {
        let out = run(&["schema", name]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let committed = std::fs::read_to_string(schemas.join(format!("{name}.schema.json")))
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(text(&out.stdout), committed, "{name}");
        let schema: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(&schema["properties"]["version"]["const"], version, "{name}");
    }
    let text_version = run(&["version"]);
    assert!(text(&text_version.stdout).contains(
        "capabilities: profile, sessions, events, play, car-images, map-images, world-settings, map-feed"
    ));
    assert_eq!(run(&["schema", "nothing"]).status.code(), Some(2));
}

#[test]
fn sessions_summarize_constructed_folders_without_writing() {
    let data = scratch("sessions");
    let dir = data.to_str().unwrap();
    let list = json(&run(&["sessions", "--json", "--data-dir", dir]));
    assert_eq!(list["sessions"], serde_json::json!([]));
    assert_eq!(list["version"], 1);
    let id = "ab".repeat(16);
    let session = data.join("artifacts").join("launcher").join(&id);
    std::fs::create_dir_all(&session).unwrap();
    let request = serde_json::json!({
        "version": 1, "id": id, "action": "serve", "game_directory": "game",
        "data_directory": data, "server_path": "server",
        "server_sha256": "e".repeat(64), "player_name": "Driver",
        "record_protocol": false
    });
    std::fs::write(session.join("request.json"), request.to_string()).unwrap();
    std::fs::write(
        session.join("status.json"),
        format!(
            r#"{{"version":1,"id":"{id}","utc":"2026-10-18T09:00:00Z","state":"finished","message":"Server closed.","worker_pid":1,"game_pid":null,"server_pid":2}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        session.join("server.stderr.log"),
        "2026-10-18T08:19:41Z  WARN nfs_server::diagnostics: unsupported operation received service=\"lsx\" route=Some(\"QueryFriends\") seen=1\n2026-10-18T08:19:42Z  INFO nfs_server: stopping\n",
    )
    .unwrap();
    let entries = |path: &std::path::Path| std::fs::read_dir(path).unwrap().count();
    let before = (entries(&data), entries(&session));
    let list = json(&run(&["sessions", "--json", "--data-dir", dir]));
    assert_eq!(
        before,
        (entries(&data), entries(&session)),
        "nothing written"
    );
    let summary = &list["sessions"][0];
    assert_eq!(summary["id"], id.as_str());
    assert_eq!(summary["action"], "serve");
    assert_eq!(summary["state"], "finished");
    assert_eq!(summary["ended_utc"], "2026-10-18T09:00:00Z");
    assert_eq!(summary["server_log"]["levels"]["warn"], 1);
    assert_eq!(
        summary["server_log"]["unsupported"],
        serde_json::json!([{"service": "lsx", "route": "QueryFriends", "warnings": 1}])
    );
    assert_eq!(summary["runs_directory_exists"], false);
    let one = json(&run(&[
        "sessions",
        "--json",
        "--id",
        &id,
        "--data-dir",
        dir,
    ]));
    assert_eq!(one["sessions"].as_array().unwrap().len(), 1);
    let shown = run(&["sessions", "--data-dir", dir]);
    assert!(text(&shown.stdout).contains("unsupported lsx QueryFriends x1"));
    let absent = run(&["sessions", "--id", &"cd".repeat(16), "--data-dir", dir]);
    assert_eq!(absent.status.code(), Some(4));
    let invalid = run(&["sessions", "--id", "ABC", "--data-dir", dir]);
    assert_eq!(invalid.status.code(), Some(1));
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn an_event_stream_refused_before_a_session_is_one_final_line() {
    // `--api` alone reports the same way: a front end learns why it was
    // refused before any session (or API) existed.
    for mode in ["--events", "--api"] {
        let data = scratch("events-refused");
        let server = data.join(format!("fake-server{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&server, b"x").unwrap();
        let out = run(&[
            "--data-dir",
            data.join("store").to_str().unwrap(),
            "serve",
            mode,
            "--game-dir",
            data.join("no-game").to_str().unwrap(),
            "--server",
            server.to_str().unwrap(),
        ]);
        assert_eq!(out.status.code(), Some(1), "{mode}: {}", text(&out.stderr));
        assert!(
            out.stderr.is_empty(),
            "{mode}: console text is silenced: {}",
            text(&out.stderr)
        );
        let stdout = text(&out.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 1, "{mode}: {stdout}");
        let event: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(event["version"], 1);
        assert_eq!(event["event"], "final");
        assert_eq!(event["exit_code"], 1);
        assert!(event["status"].is_null());
        assert!(!event["message"].as_str().unwrap().is_empty());
        assert!(!data.join("store").exists(), "{mode}: nothing was written");
        std::fs::remove_dir_all(data).unwrap();
    }
}

#[test]
fn check_refuses_a_missing_installation_without_writing() {
    let data = scratch("check");
    let server = data.join(format!("fake-server{}", std::env::consts::EXE_SUFFIX));
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
    let stderr = text(&out.stderr);
    if stderr.contains("non-administrator") || stderr.contains("not as root") {
        // Hosted CI runners are elevated. Preflight refuses before it looks
        // at the installation, which is the right behavior; the installation
        // checks below need an ordinary user and run on developer machines.
        std::fs::remove_dir_all(data).unwrap();
        return;
    }
    assert!(stderr.contains("not found"), "{stderr}");
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
    assert!(
        text(&out.stderr).contains("Unsupported installed file"),
        "{}",
        text(&out.stderr)
    );
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

#[test]
fn the_relay_stands_in_for_a_server_until_its_stop_file_appears() {
    use std::io::{BufRead, Read, Write};
    // Two stand-in services that echo what they receive.
    let echo = || {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut reader = stream.try_clone().unwrap();
                    let mut writer = stream;
                    let _ = std::io::copy(&mut reader, &mut writer);
                });
            }
        });
        address
    };
    let (redirector, services) = (echo(), echo());
    let dir = scratch("relay");
    std::fs::create_dir_all(&dir).unwrap();
    let stop = dir.join("stop");
    let mut relay = bin()
        .args([
            "relay",
            "--redirector",
            &redirector,
            "--launcher-services",
            &services,
            "--lsx-port",
            "0",
            "--stop-file",
            stop.to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(relay.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    // The readiness record a supervisor expects from a server.
    let ready: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(ready["ready"], true);
    assert_eq!(ready["pid"], relay.id());
    assert_eq!(ready["relays_to"]["redirector"], redirector.as_str());
    for key in ["redirector", "lsx"] {
        let address = ready[key].as_str().unwrap();
        assert!(address.starts_with("127.0.0.1:"), "{key}: {address}");
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        stream.write_all(key.as_bytes()).unwrap();
        let mut answer = vec![0u8; key.len()];
        stream.read_exact(&mut answer).unwrap();
        assert_eq!(answer, key.as_bytes());
    }
    std::fs::write(&stop, b"stop").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = relay.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the relay did not stop"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(status.success());
    std::fs::remove_dir_all(dir).unwrap();
}

/// A synthetic save of account `byte` repeated, for a driver called `name`,
/// selected by `account.json`; no prepared content. Its path.
fn save_driver(data: &std::path::Path, byte: u8, name: &str) -> PathBuf {
    use nfs_storage::{
        AccountId, Batch, GarageSlots, Op, Timestamp,
        account::{Document, Kind, Repository},
    };
    let id = format!("{byte:02x}").repeat(16);
    let account = AccountId::from_owned_config([byte; 16]).unwrap();
    let identity = serde_json::json!({"version": 1, "storage_account": id,
        "persona": 41, "account": 42, "name": name});
    let documents: Vec<_> = Kind::ALL
        .into_iter()
        .map(|kind| {
            let bytes = if kind == Kind::Identity {
                identity.to_string().into_bytes()
            } else {
                b"{}".to_vec()
            };
            (kind, Document { revision: 1, bytes })
        })
        .collect();
    let batch = Batch {
        id: 1,
        expected_generation: 0,
        ops: vec![Op::SetGarage(GarageSlots::default())],
    };
    let accounts = data.join("artifacts").join("account");
    std::fs::create_dir_all(&accounts).unwrap();
    Repository::open_owned_directory(&accounts)
        .unwrap()
        .initialize_fresh(account, &documents, &batch, Timestamp(1))
        .unwrap();
    std::fs::write(
        data.join("account.json"),
        format!(r#"{{"version":1,"local_account":"{id}","progression":"post-intro"}}"#),
    )
    .unwrap();
    accounts.join(format!("{id}.sqlite"))
}

/// A stand-in `nfs-server`: answers `prepare-local` and `create-account`
/// with the JSON shapes the real server prints, and otherwise prints a
/// readiness record and waits for its `--stop-file`.
#[test]
fn profile_reads_a_saved_driver_and_reports_missing_accounts() {
    let data = scratch("profile");
    let dir = data.to_str().unwrap();
    let out = run(&["profile", "--json", "--data-dir", dir]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    let out = run(&["profile", "--list", "--json", "--data-dir", dir]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let listing: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listing["accounts"], serde_json::json!([]));
    let save = save_driver(&data, 0x5a, "Cli Tester");
    let before = std::fs::read(&save).unwrap();
    // An empty game directory: no test reads a registered installation.
    let no_game = scratch("profile-no-game");
    let game = no_game.to_str().unwrap();
    let out = run(&["profile", "--json", "--data-dir", dir, "--game-dir", game]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let profile: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(profile["identity"]["display_name"], "Cli Tester");
    assert_eq!(profile["identity"]["persona_id"], "41");
    assert_eq!(profile["bank"]["balance"], 0);
    assert_eq!(profile["save"]["generation"], 1);
    assert_eq!(profile["active"], true);
    assert!(profile["reputation"].is_null(), "no content, no REP schema");
    assert!(!profile["warnings"].as_array().unwrap().is_empty());
    assert!(
        profile["localization"].is_null(),
        "no installation, no names"
    );
    let out = run(&["profile", "--data-dir", dir, "--game-dir", game]);
    let shown = text(&out.stdout);
    assert!(
        shown.contains("Cli Tester (selected)") && shown.contains("Bank $0"),
        "{shown}"
    );
    let out = run(&["profile", "--list", "--json", "--data-dir", dir]);
    let listing: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listing["accounts"][0]["display_name"], "Cli Tester");
    let out = run(&[
        "profile",
        "--data-dir",
        dir,
        "--account-id",
        &"7b".repeat(16),
        "--game-dir",
        game,
    ]);
    assert_eq!(out.status.code(), Some(4));
    let out = run(&["profile", "--data-dir", dir, "--account-id", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        std::fs::read(&save).unwrap(),
        before,
        "profile never writes"
    );
    // Another process holding an uncommitted write, as a running server
    // would, neither blocks the profile nor shows through.
    let writer = rusqlite::Connection::open(&save).unwrap();
    writer
        .execute_batch("BEGIN IMMEDIATE; UPDATE meta SET generation=generation+5;")
        .unwrap();
    let generation = |out: Output| {
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["save"]["generation"]
            .clone()
    };
    assert_eq!(
        generation(run(&[
            "profile",
            "--json",
            "--data-dir",
            dir,
            "--game-dir",
            game
        ])),
        1
    );
    writer.execute_batch("COMMIT;").unwrap();
    assert_eq!(
        generation(run(&[
            "profile",
            "--json",
            "--data-dir",
            dir,
            "--game-dir",
            game
        ])),
        6
    );
    drop(writer);
    std::fs::remove_dir_all(data).unwrap();
    std::fs::remove_dir_all(no_game).unwrap();
}

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
feed=""
while [ $# -gt 0 ]; do
  if [ "$1" = "--stop-file" ]; then stop="$2"; fi
  if [ "$1" = "--map-feed" ]; then feed=1; fi
  shift
done
echo "fake server starting" >&2
printf '{{"ready":true,"pid":%s,"redirector":"127.0.0.1:34567","lsx":null}}\n' "$$"
frame=0
while [ ! -e "$stop" ]; do
  if [ -n "$feed" ]; then
    frame=$((frame + 1))
    printf '{{"map_feed":1,"frame":%s,"worlds":[{{"connection":1,"persona_id":"1000000007","world_ms":%s,"drivers":[{{"participant":1,"x":%s.5,"y":0.0,"z":-20.25,"heading":90.0,"speed":3.5}}],"traffic":[],"rivals":[],"traffic_ring":null,"omitted":0}}]}}\n' "$frame" "$((frame * 100))" "$frame"
  fi
  sleep 0.1
done
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
    assert!(ready.contains("\"action\":\"serve\""), "{ready}");
    assert!(ready.contains("\"game_running\":false"), "{ready}");
    // Only Windows hosts the game; the serving session is left as it was.
    let play = run(&["--data-dir", &data_arg, "play"]);
    assert_eq!(play.status.code(), Some(1), "{}", text(&play.stderr));
    assert!(text(&play.stderr).contains("Windows only"));
    assert!(!run_dir.join("play").exists());
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
    assert!(
        out.stdout.is_empty(),
        "supervisor diagnostics stay off stdout"
    );
    let console = text(&out.stderr);
    let mut previous = 0;
    for heading in [
        "Preparing your game data",
        "Server is running",
        "Closing your session",
        "All done",
    ] {
        let position = console
            .find(heading)
            .unwrap_or_else(|| panic!("{heading} missing from console:\n{console}"));
        assert!(position >= previous, "session headings are out of order");
        previous = position;
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
    assert!(text(&again.stdout).contains("Your local save is kept for next time."));
    assert!(run_dir.join("server.stdout.jsonl").exists());
    assert!(run_dir.join("prepare.stdout.json").exists());
    std::fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn a_serve_session_streams_its_states_and_diagnostics_as_json_lines() {
    let base = scratch("events");
    let data = base.join("data");
    let game = base.join("game");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&game).unwrap();
    let server = fake_server(&base, &data);
    let id = "f".repeat(32);
    let run_dir = data.join("artifacts").join("launcher").join(&id);
    std::fs::create_dir_all(&run_dir).unwrap();
    let request = serde_json::json!({
        "version": 1, "id": id, "action": "serve",
        "game_directory": game, "data_directory": data,
        "server_path": server, "server_sha256": sha256(&server),
        "player_name": "Tester", "record_protocol": false, "events": true
    });
    std::fs::write(run_dir.join("request.json"), request.to_string()).unwrap();
    let data_arg = data.to_str().unwrap().to_owned();
    let supervisor = bin()
        .args([
            "supervise",
            "--request",
            run_dir.join("request.json").to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !text(&run(&["--data-dir", &data_arg, "--json", "status"]).stdout)
        .contains("\"state\":\"ready\"")
    {
        assert!(std::time::Instant::now() < deadline, "never ready");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Long enough for several of the server's map frames and polls.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let stop = run(&["--data-dir", &data_arg, "stop"]);
    assert_eq!(stop.status.code(), Some(0), "{}", text(&stop.stderr));
    let out = supervisor.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        out.stderr.is_empty(),
        "console text is silenced: {}",
        text(&out.stderr)
    );
    let stdout_log = std::fs::read_to_string(run_dir.join("server.stdout.jsonl")).unwrap();
    assert_eq!(
        stdout_log.lines().count(),
        1,
        "only the readiness line is logged: {stdout_log}"
    );
    let events: Vec<serde_json::Value> = text(&out.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("every line is one JSON object"))
        .collect();
    assert!(
        events
            .iter()
            .all(|e| e["version"] == 1 && e["utc"].is_string())
    );
    let statuses: Vec<&serde_json::Value> =
        events.iter().filter(|e| e["event"] == "status").collect();
    // The supervisor records and emits a first `preparing` ("Starting the
    // session.") before its state machine's own, so the stream may carry
    // `preparing` twice; the states still pass in order, each once.
    assert_eq!(statuses[0]["status"]["state"], "preparing");
    let mut states: Vec<&str> = Vec::new();
    for status in &statuses {
        let state = status["status"]["state"].as_str().unwrap();
        if states.last() != Some(&state) {
            states.push(state);
        }
    }
    assert_eq!(states, ["preparing", "ready", "stopping", "finished"]);
    assert!(
        events
            .iter()
            .filter(|e| e["event"] == "status")
            .all(|e| e["status"]["action"] == "serve" && e["status"]["game_running"] == false),
        "every status names its action and that no game runs"
    );
    let diagnostics: Vec<(&str, &str)> = events
        .iter()
        .filter(|e| e["event"] == "diagnostic")
        .map(|e| (e["source"].as_str().unwrap(), e["text"].as_str().unwrap()))
        .collect();
    assert!(diagnostics.contains(&("server", "fake server starting")));
    assert!(diagnostics.contains(&("server", "fake server stopping")));
    // The server got --map-feed; its frames arrive while it is ready, only
    // ever forward, and say how many were skipped in between.
    let ready = events
        .iter()
        .position(|e| e["event"] == "status" && e["status"]["state"] == "ready")
        .unwrap();
    let feed: Vec<(usize, &serde_json::Value)> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e["event"] == "map-feed")
        .collect();
    assert!(feed.len() >= 2, "map frames while serving: {events:?}");
    let mut previous = 0;
    for (at, event) in &feed {
        assert!(*at > ready);
        let frame = event["frame"].as_u64().unwrap();
        assert_eq!(frame, previous + event["skipped"].as_u64().unwrap() + 1);
        previous = frame;
        let driver = &event["worlds"][0]["drivers"][0];
        assert_eq!(driver["x"], frame as f64 + 0.5, "the frame's own data");
        assert_eq!(
            (driver["z"].as_f64(), driver["heading"].as_f64()),
            (Some(-20.25), Some(90.0))
        );
        assert_eq!(event["worlds"][0]["persona_id"], "1000000007");
    }
    let last = events.last().unwrap();
    assert_eq!(
        last["event"], "status",
        "the front end, not the supervisor, writes final"
    );
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("status.json")).unwrap()).unwrap();
    assert_eq!(
        last["status"], stored,
        "events carry the status.json record"
    );
    std::fs::remove_dir_all(base).unwrap();
}

/// One request to a session API; its status code and body.
#[cfg(unix)]
fn api_request(address: &str, token: &str, method: &str, path: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap();
    let code = head.split(' ').nth(1).unwrap().parse().unwrap();
    (code, body.to_owned())
}

/// `PUT` a JSON body to a session API; the status code and body.
#[cfg(unix)]
fn api_put(address: &str, token: &str, path: &str, body: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "PUT {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap();
    let code = head.split(' ').nth(1).unwrap().parse().unwrap();
    (code, body.to_owned())
}

#[cfg(unix)]
#[test]
fn a_serve_session_serves_its_api_until_it_stops() {
    use std::io::{BufRead, Write};
    let base = scratch("api");
    let data = base.join("data");
    let game = base.join("game");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&game).unwrap();
    let server = fake_server(&base, &data);
    let id = "a".repeat(32);
    let run_dir = data.join("artifacts").join("launcher").join(&id);
    std::fs::create_dir_all(&run_dir).unwrap();
    let request = serde_json::json!({
        "version": 1, "id": id, "action": "serve",
        "game_directory": game, "data_directory": data,
        "server_path": server, "server_sha256": sha256(&server),
        "player_name": "Tester", "record_protocol": false, "api": true
    });
    std::fs::write(run_dir.join("request.json"), request.to_string()).unwrap();
    let data_arg = data.to_str().unwrap().to_owned();
    let supervisor = bin()
        .args([
            "supervise",
            "--request",
            run_dir.join("request.json").to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // A front end finds the API through `api --json`, then follows it there.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let record = loop {
        let out = run(&["--data-dir", &data_arg, "api", "--json"]);
        if out.status.code() == Some(0) {
            break json(&out);
        }
        assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
        assert!(
            std::time::Instant::now() < deadline,
            "the API never came up"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(record["id"], id.as_str());
    let address = record["url"]
        .as_str()
        .unwrap()
        .strip_prefix("http://127.0.0.1:")
        .map(|port| format!("127.0.0.1:{port}"))
        .unwrap();
    let token = record["token"].as_str().unwrap().to_owned();
    assert_eq!(api_request(&address, "wrong", "GET", "/v1/status").0, 401);
    loop {
        let (code, body) = api_request(&address, &token, "GET", "/v1/status");
        assert_eq!(code, 200, "{body}");
        if body.contains("\"state\":\"ready\"") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "never ready: {body}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (code, body) = api_request(&address, &token, "GET", "/v1/info");
    assert_eq!(code, 200);
    assert!(body.contains("\"api\""), "{body}");
    // The settings, read and changed there as with `settings`, while the
    // session runs.
    let (code, body) = api_request(&address, &token, "GET", "/v1/settings");
    assert_eq!(code, 200, "{body}");
    let read: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        read,
        json(&run(&["--data-dir", &data_arg, "settings", "--json"]))
    );
    let change = r#"{"traffic_percent":150,"rival_limit":2}"#;
    let (code, body) = api_put(&address, &token, "/v1/settings", change);
    assert_eq!(code, 200, "{body}");
    let saved: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        (
            &saved["traffic_percent"],
            &saved["rival_limit"],
            &saved["saved"]
        ),
        (&150.into(), &2.into(), &true.into())
    );
    let after = json(&run(&["--data-dir", &data_arg, "settings", "--json"]));
    assert_eq!(
        (&after["traffic_percent"], &after["rival_limit"]),
        (&150.into(), &2.into())
    );
    assert_eq!(
        api_put(
            &address,
            &token,
            "/v1/settings",
            r#"{"traffic_percent":301}"#
        )
        .0,
        400
    );
    // The selected save's profile, as `profile --json` reads it.
    save_driver(&data, 0x5b, "Api Tester");
    let (code, body) = api_request(&address, &token, "GET", "/v1/profile");
    assert_eq!(code, 200, "{body}");
    let profile: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(profile["identity"]["display_name"], "Api Tester");
    assert_eq!(
        profile,
        json(&run(&["--data-dir", &data_arg, "profile", "--json"]))
    );
    // The event stream, read to its end on its own thread.
    let reader = {
        let (address, token) = (address.clone(), token.clone());
        std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(&address).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(30)))
                .unwrap();
            write!(
                stream,
                "GET /v1/events HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\n\r\n"
            )
            .unwrap();
            let mut events = Vec::new();
            let mut kind = String::new();
            for line in std::io::BufReader::new(stream).lines() {
                let line = line.unwrap();
                if let Some(value) = line.strip_prefix("event: ") {
                    kind = value.to_owned();
                } else if let Some(value) = line.strip_prefix("data: ") {
                    let event: serde_json::Value = serde_json::from_str(value).unwrap();
                    assert_eq!(event["event"], kind.as_str());
                    events.push(event);
                }
            }
            events
        })
    };
    // Long enough for several of the server's map frames and polls.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let (code, body) = api_request(&address, &token, "POST", "/v1/stop");
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"outcome\":\"requested\""), "{body}");
    let out = supervisor.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        out.stdout.is_empty(),
        "without --events the supervisor writes nothing to stdout"
    );
    assert!(
        out.stderr.is_empty(),
        "console text is silenced: {}",
        text(&out.stderr)
    );
    let events = reader.join().unwrap();
    assert_eq!(events[0]["event"], "status");
    assert_eq!(events[0]["status"]["state"], "ready");
    let states: Vec<&str> = events
        .iter()
        .filter(|e| e["event"] == "status")
        .map(|e| e["status"]["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["ready", "stopping", "finished"]);
    let frames: Vec<u64> = events
        .iter()
        .filter(|e| e["event"] == "map-feed")
        .map(|e| e["frame"].as_u64().unwrap())
        .collect();
    assert!(frames.len() >= 2, "map frames for the API: {events:?}");
    assert!(frames.windows(2).all(|w| w[0] < w[1]));
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "diagnostic" && e["text"] == "fake server stopping")
    );
    assert!(!run_dir.join("api.json").exists(), "the token is forgotten");
    let gone = run(&["--data-dir", &data_arg, "api", "--json"]);
    assert_eq!(gone.status.code(), Some(4));
    assert!(std::net::TcpStream::connect(&address).is_err());
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
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("status.json")).unwrap()).unwrap();
    assert!(text(&out.stdout).contains(&format!("server {}", stored["server_pid"])));
    assert!(text(&out.stdout).contains("Verify identity"));
    // The orphaned fake server is still running; stop it by its stop file,
    // which is exactly what the status message tells the user to look at.
    std::fs::write(run_dir.join("stop-server"), b"stop").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn car_images_needs_prepared_content_first() {
    let data = scratch("car-images");
    let out = run(&["car-images", "--json", "--data-dir", data.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("run setup first"));
    assert!(!data.join("artifacts").exists(), "nothing was written");
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn map_images_needs_prepared_content_first() {
    let data = scratch("map-images");
    let out = run(&["map-images", "--json", "--data-dir", data.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("run setup first"));
    assert!(!data.join("artifacts").exists(), "nothing was written");
    std::fs::remove_dir_all(data).unwrap();
}
