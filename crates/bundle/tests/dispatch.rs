// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::{
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn output(command: &mut Command) -> Output {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("bundle command exceeded its deadline: {command:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn the_same_executable_exposes_launcher_server_and_setup_without_a_sibling_server() {
    for args in [
        vec!["--help"],
        vec!["--afterhours-server", "--help"],
        vec!["--afterhours-server", "prepare-local", "--help"],
    ] {
        let output = output(Command::new(env!("CARGO_BIN_EXE_Afterhours")).args(&args));
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            text.contains("Usage:") || text.contains("usage:"),
            "{args:?}: {text}"
        );
    }
}

#[test]
fn explicit_invalid_commands_never_wait_for_terminal_input() {
    let output = output(Command::new(env!("CARGO_BIN_EXE_Afterhours")).arg("unknown-command"));
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Press Enter"));
}

#[test]
fn a_renamed_sdk_copy_still_dispatches_the_server_explicitly() {
    let scratch =
        std::env::temp_dir().join(format!("afterhours-bundle-copy-{}", std::process::id()));
    std::fs::create_dir(&scratch).unwrap();
    let copy = scratch.join(if cfg!(windows) {
        "EADesktop.exe"
    } else {
        "EADesktop"
    });
    std::fs::copy(env!("CARGO_BIN_EXE_Afterhours"), &copy).unwrap();
    let output =
        output(Command::new(&copy).args(["--afterhours-server", "prepare-local", "--help"]));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("prepare-local"));
    std::fs::remove_file(copy).unwrap();
    std::fs::remove_dir(scratch).unwrap();
}
