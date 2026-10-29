// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy)]
enum Fault {
    None,
    WrongPid,
    Exited,
    Logs,
    ChangedExecutable,
}
#[derive(Default)]
struct Observed {
    spawned: Vec<SpawnSpec>,
    killed: Vec<u32>,
}
struct Scripted {
    observed: Rc<RefCell<Observed>>,
    fault: Fault,
}
struct Child {
    observed: Rc<RefCell<Observed>>,
    serving: bool,
    fault: Fault,
}
impl ChildProcess for Child {
    fn id(&self) -> u32 {
        if self.serving { 22 } else { 11 }
    }
    fn exited(&mut self) -> bool {
        !self.serving || matches!(self.fault, Fault::Exited)
    }
    fn exit_code(&mut self) -> Option<i32> {
        self.exited().then_some(0)
    }
    fn first_line(&mut self) -> Option<Result<String>> {
        let pid = if matches!(self.fault, Fault::WrongPid) {
            11
        } else {
            self.id()
        };
        Some(Ok(serde_json::json!({
            "ready": true, "pid": pid, "redirector": "127.0.0.1:24567", "lsx": null
        })
        .to_string()))
    }
    fn drain_diagnostics(&mut self) -> String {
        String::new()
    }
    fn logs_finished(&self) -> bool {
        true
    }
    fn output_truncated(&self) -> bool {
        false
    }
    fn check_logs(&self) -> Result<()> {
        if matches!(self.fault, Fault::Logs) {
            Err(Failure::session("constructed log failure"))
        } else {
            Ok(())
        }
    }
    fn request_close(&mut self) -> bool {
        false
    }
    fn kill(&mut self) {
        self.observed.borrow_mut().killed.push(self.id());
    }
}
impl Platform for Scripted {
    fn refuse_privileged(&self) -> Result<()> {
        Ok(())
    }
    fn registered_client_path(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn running_processes(&self, _: &[&str]) -> Result<Vec<String>> {
        Ok(vec![])
    }
    fn process_alive(&self, _: u32) -> bool {
        false
    }
    fn attach_session_guard(&self) -> Result<Box<dyn platform::SessionGuard>> {
        Err(Failure::internal("no native process in this test"))
    }
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn ChildProcess>> {
        self.observed.borrow_mut().spawned.push(spec.clone());
        if !spec.capture_first_line {
            if spec.arguments.iter().any(|a| a == "create-account") {
                let at = spec
                    .arguments
                    .iter()
                    .position(|a| a == "--state-directory")
                    .unwrap();
                std::fs::create_dir_all(&spec.arguments[at + 1])?;
                std::fs::write(
                    &spec.stdout_log,
                    br#"{"local_account":"0123456789abcdef0123456789abcdef"}"#,
                )?;
            } else {
                std::fs::write(&spec.stdout_log, b"{\"prepared\":true}")?;
            }
        } else if matches!(self.fault, Fault::ChangedExecutable) {
            std::fs::write(&spec.executable, b"replacement bytes")?;
        }
        Ok(Box::new(Child {
            observed: self.observed.clone(),
            serving: spec.capture_first_line,
            fault: self.fault,
        }))
    }
}

fn session(fault: Fault) -> (LiveSession, Rc<RefCell<Observed>>) {
    let data = crate::testing::scratch("serving-process");
    let run = data.join("session");
    std::fs::create_dir(&run).unwrap();
    let executable = data.join("Compatibility Server.exe");
    std::fs::write(&executable, b"constructed server").unwrap();
    let request = Request {
        version: records::VERSION,
        id: "0123456789abcdef0123456789abcdef".into(),
        action: records::Action::Serve,
        game_directory: None,
        data_directory: data.clone(),
        server_path: executable,
        server_sha256: crate::digest::sha256_bytes(b"constructed server"),
        bundled_server: false,
        player_name: "Test".into(),
        record_protocol: true,
        events: false,
        api: false,
        game_support: false,
        listen: None,
        api_port: None,
        remote: None,
        settings: Default::default(),
    };
    let observed = Rc::new(RefCell::new(Observed::default()));
    let session = LiveSession {
        platform: Box::new(Scripted {
            observed: observed.clone(),
            fault,
        }),
        request,
        data,
        run,
        started: Instant::now(),
        server: None,
        game: None,
        prepared: None,
        account: None,
        server_executable: None,
        ready: None,
        events: Emitter::disabled(),
        map_feed: Default::default(),
        games: 0,
        #[cfg(windows)]
        job: None,
        #[cfg(windows)]
        lease: None,
        relay_sha256: None,
    };
    (session, observed)
}

fn start(session: &mut LiveSession, args: Vec<String>, role: &str) -> Result<()> {
    session.run_server(
        session.request.server_path.clone(),
        args,
        &session.request.server_sha256.clone(),
        role,
    )
}

#[test]
fn receipt_names_the_ready_serving_child_and_preserves_argument_boundaries() {
    let (mut session, observed) = session(Fault::None);
    session
        .run_tool("prepare-local", vec!["prepare-local".into()])
        .unwrap();
    let path = session.run.join("server-process.json");
    assert!(
        !path.exists(),
        "a preparation process is not the serving process"
    );
    let args = vec![
        "--server".into(),
        "--root".into(),
        "C:/Data with spaces".into(),
        "--output".into(),
        "a quoted \"name\"".into(),
        "--record-protocol".into(),
    ];
    start(&mut session, args.clone(), "local-server").unwrap();
    let receipt: serde_json::Value = records::read_json(&path).unwrap();
    assert_eq!(receipt["format"], "afterhours-serving-process-v1");
    assert_eq!(receipt["session_id"], session.request.id);
    assert_eq!(receipt["role"], "local-server");
    assert_eq!(receipt["pid"], 22);
    assert_eq!(receipt["readiness"]["pid"], 22);
    assert_eq!(receipt["arguments"], serde_json::json!(args));
    assert_eq!(
        receipt["executable"],
        serde_json::json!(session.request.server_path)
    );
    assert_eq!(receipt["executable_sha256"], session.request.server_sha256);
    assert_eq!(
        receipt["working_directory"],
        serde_json::json!(session.data)
    );
    assert_eq!(session.server.as_ref().unwrap().id(), 22);
    assert_eq!(observed.borrow().spawned.len(), 2);
    assert!(observed.borrow().killed.is_empty());
    std::fs::remove_dir_all(session.data).unwrap();
}

#[test]
fn failed_readiness_or_receipt_never_adopts_the_child() {
    for fault in [
        Fault::WrongPid,
        Fault::Exited,
        Fault::Logs,
        Fault::ChangedExecutable,
    ] {
        let (mut session, observed) = session(fault);
        assert!(start(&mut session, vec![], "local-server").is_err());
        assert!(!session.run.join("server-process.json").exists());
        assert!(session.server.is_none() && session.ready.is_none());
        assert_eq!(observed.borrow().killed, vec![22]);
        std::fs::remove_dir_all(session.data).unwrap();
    }
    // A previous receipt is evidence, not a file to overwrite. The new child
    // must be cleaned up when persisting its identity fails.
    let (mut session, observed) = session(Fault::None);
    let path = session.run.join("server-process.json");
    std::fs::write(&path, b"preserve").unwrap();
    assert!(start(&mut session, vec![], "local-server").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"preserve");
    assert_eq!(observed.borrow().killed, vec![22]);
    assert!(session.server.is_none());
    std::fs::remove_dir_all(session.data).unwrap();
}

#[test]
fn receipt_is_bounded_and_remote_relay_is_explicit() {
    let (mut session, observed) = session(Fault::None);
    assert!(
        start(
            &mut session,
            vec!["x".repeat(records::MAX_RECORD_BYTES as usize)],
            "local-server"
        )
        .is_err()
    );
    assert_eq!(observed.borrow().killed, vec![22]);
    assert!(!session.run.join("server-process.json").exists());
    std::fs::remove_dir_all(session.data).unwrap();
    let (mut session, _) = self::session(Fault::None);
    start(&mut session, vec!["relay".into()], "relay").unwrap();
    let receipt: serde_json::Value =
        records::read_json(&session.run.join("server-process.json")).unwrap();
    assert_eq!(receipt["role"], "relay");
    std::fs::remove_dir_all(session.data).unwrap();
}

#[test]
fn first_setup_starts_the_prologue_and_repeated_setup_preserves_the_account() {
    let (mut session, observed) = session(Fault::None);
    session.request.game_directory = Some(session.data.join("Game"));
    let prepared = Prepared {
        files: BTreeMap::from([("account-policy".into(), session.data.join("policy.json"))]),
    };
    let created = session.ensure_account(&prepared).unwrap();
    assert_eq!(created.progression, "fresh");
    let args = &observed.borrow().spawned[0].arguments;
    for (flag, value) in [
        ("--progression", "fresh"),
        ("--starting-resources", "standard"),
    ] {
        let at = args.iter().position(|a| a == flag).unwrap();
        assert_eq!(args[at + 1], value);
    }
    let account_path = session.data.join("account.json");
    let original = std::fs::read(&account_path).unwrap();
    let marker = session.data.join("artifacts/account/preserved.sqlite");
    std::fs::write(&marker, b"existing save bytes").unwrap();
    assert_eq!(
        session.ensure_account(&Prepared::default()).unwrap(),
        created
    );
    assert_eq!(std::fs::read(&account_path).unwrap(), original);
    assert_eq!(std::fs::read(&marker).unwrap(), b"existing save bytes");
    assert_eq!(observed.borrow().spawned.len(), 1);
    std::fs::remove_dir_all(session.data).unwrap();
}

#[test]
fn existing_post_intro_accounts_are_not_reinitialized_by_setup() {
    let (mut session, observed) = session(Fault::None);
    let account = AccountRecord::post_intro("a".repeat(32));
    let path = session.data.join("account.json");
    records::write_new_json(&path, &account).unwrap();
    std::fs::create_dir_all(session.data.join("artifacts/account")).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        session.ensure_account(&Prepared::default()).unwrap(),
        account
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(observed.borrow().spawned.is_empty());
    std::fs::remove_dir_all(session.data).unwrap();
}
