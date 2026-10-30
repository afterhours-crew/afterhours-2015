// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use clap::Parser;

fn spec(dir: &Path, name: &str, script: &str) -> SpawnSpec {
    let path = dir.join(if cfg!(windows) {
        format!("{name}.cmd")
    } else {
        format!("{name}.sh")
    });
    let script = if cfg!(windows) {
        format!("@echo off\n{script}")
    } else {
        script.to_owned()
    };
    std::fs::write(&path, script).unwrap();
    let (executable, mut arguments) = if cfg!(windows) {
        ("cmd", vec!["/C".into()])
    } else {
        ("sh", vec![])
    };
    arguments.push(path.to_str().unwrap().into());
    SpawnSpec {
        executable: executable.into(),
        arguments,
        working_directory: dir.into(),
        stdout_log: dir.join(format!("{name}.json")),
        stdout_keeps: Keep::Start,
        stderr_log: dir.join(format!("{name}.stderr.log")),
        environment: vec![],
        capture_first_line: false,
        map_feed: false,
    }
}
#[test]
fn helper_accepts_only_complete_successful_json_and_has_a_bounded_deadline() {
    let dir = crate::testing::scratch("story-tool");
    let good = if cfg!(windows) {
        "echo {\"version\":1}"
    } else {
        "echo '{\"version\":1}'"
    };
    assert_eq!(
        execute(&spec(&dir, "ok", good), Duration::from_secs(5)).unwrap()["version"],
        1
    );
    let failed = if cfg!(windows) {
        "@echo off\necho {\"version\":1}\nexit /b 7"
    } else {
        "echo '{\"version\":1}'\nexit 7"
    };
    assert!(execute(&spec(&dir, "failed", failed), Duration::from_secs(5)).is_err());
    assert!(
        execute(
            &spec(&dir, "invalid", "echo invalid-json"),
            Duration::from_secs(5)
        )
        .is_err()
    );
    let hanging = if cfg!(windows) {
        "@echo off\n:again\ngoto again"
    } else {
        "while :; do :; done"
    };
    let started = Instant::now();
    assert!(execute(&spec(&dir, "deadline", hanging), Duration::from_millis(50)).is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn apply_requires_the_exact_preview_and_preview_cannot_also_apply() {
    use crate::cli::{Cli, Command};
    let parse = |options: Vec<&str>| {
        Cli::try_parse_from(["afterhours", "story"].into_iter().chain(options))
    };
    assert!(parse(vec!["--preview", "9007199254740993"]).is_ok());
    for options in [
        vec!["--apply", "7"],
        vec!["--apply", "7", "--expected-generation", "1"],
        vec![
            "--preview",
            "7",
            "--apply",
            "21",
            "--expected-generation",
            "1",
            "--plan-id",
            "hash",
        ],
        vec!["--expected-generation", "1"],
        vec!["--plan-id", "hash"],
    ] {
        assert!(parse(options).is_err());
    }
    let Command::Story(args) = parse(vec![
        "--apply",
        "21",
        "--expected-generation",
        "9007199254740993",
        "--plan-id",
        "hash",
    ])
    .unwrap()
    .command
    else {
        panic!()
    };
    assert_eq!(
        args.expected_generation.as_deref(),
        Some("9007199254740993")
    );
}
