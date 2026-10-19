// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The real session: [`Steps`] implemented over the server, the game and
//! the files in the session directory.
//!
//! Each step is small and does one thing. Preparation runs the server's
//! own `prepare-local` and `create-account` subcommands and reads their
//! JSON output from the bounded log. Starting the server waits for its
//! first-line readiness record and checks it names the process we started.
//! On Windows, starting the game pins it with a routing lease, and polling
//! watches for the activation helper's window, the game's lifetime and the
//! lease becoming ready. Every wait is bounded and every file written here
//! lives under the session directory or the account directory.

use super::{GamePoll, Steps};
use crate::{
    clock,
    events::{DiagnosticSource, Emitter},
    exit::{Failure, Result},
    paths,
    platform::{self, ChildProcess, Platform, SpawnSpec},
    records::{self, AccountRecord, Action, Request, State, Status},
    session, terminal,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// How long preparation subcommands may run.
const PREPARATION_DEADLINE: Duration = Duration::from_secs(600);
/// How long the server may take to print its readiness record.
const SERVER_READY_DEADLINE: Duration = Duration::from_secs(120);
/// How long a child gets to close normally before it is killed.
const CLOSE_GRACE: Duration = Duration::from_secs(15);
/// How long a killed child gets to disappear.
const KILL_GRACE: Duration = Duration::from_secs(10);
/// How long the log pumps get after the last child exits.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// Output of `nfs-server prepare-local`: generated policy files by name.
#[derive(Debug, Clone, Default)]
pub struct Prepared {
    pub files: BTreeMap<String, PathBuf>,
}

/// The server's readiness record, reduced to what the session uses.
#[derive(Debug, Clone)]
struct ServerReady {
    /// Port the routing lease writes into the game; unused where there is no game.
    #[cfg_attr(not(windows), allow(dead_code))]
    redirector_port: u16,
}

/// A running session.
pub struct LiveSession {
    platform: Box<dyn Platform>,
    request: Request,
    data: PathBuf,
    run: PathBuf,
    started: Instant,
    server: Option<Box<dyn ChildProcess>>,
    game: Option<Box<dyn ChildProcess>>,
    prepared: Option<Prepared>,
    account: Option<AccountRecord>,
    server_executable: Option<PathBuf>,
    ready: Option<ServerReady>,
    /// The JSON-lines stream when the request asked for events.
    events: Emitter,
    #[cfg(windows)]
    job: Option<platform::windows::JobGuard>,
    #[cfg(windows)]
    lease: Option<platform::windows::lease::EndpointLease>,
}

fn wait_child_exit(child: &mut dyn ChildProcess, grace: Duration) -> Result<()> {
    let deadline = Instant::now() + grace;
    while !child.exited() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if child.exited() {
        return Ok(());
    }
    child.kill();
    let deadline = Instant::now() + KILL_GRACE;
    while !child.exited() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if child.exited() {
        Ok(())
    } else {
        Err(Failure::session(
            "Owned process did not exit; session job remains armed.",
        ))
    }
}

impl LiveSession {
    /// Build a session for a validated request whose data directory is
    /// locked by the caller. On Windows this attaches the session job.
    /// Status changes and child diagnostics go to `events` when it is
    /// enabled, and to the console otherwise.
    pub fn new(request: Request, data: PathBuf, run: PathBuf, events: Emitter) -> Result<Self> {
        #[cfg(windows)]
        let job = Some(platform::windows::JobGuard::attach()?);
        Ok(Self {
            platform: Box::new(platform::native()),
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
            events,
            #[cfg(windows)]
            job,
            #[cfg(windows)]
            lease: None,
        })
    }

    fn game_directory(&self) -> Result<&Path> {
        self.request
            .game_directory
            .as_deref()
            .ok_or_else(|| Failure::refused("This action needs --game-dir."))
    }

    /// Pass a child's diagnostic text to the event stream or the console.
    fn forward(&mut self, source: DiagnosticSource, text: &str) {
        if self.events.enabled() {
            self.events.diagnostic(source, text);
        } else {
            terminal::raw(text);
        }
    }

    fn echo_diagnostics(&mut self) {
        let server = self.server.as_mut().map(|s| s.drain_diagnostics());
        let game = self.game.as_mut().map(|g| g.drain_diagnostics());
        for (source, text) in [
            (DiagnosticSource::Server, server),
            (DiagnosticSource::Game, game),
        ] {
            if let Some(text) = text {
                self.forward(source, &text);
            }
        }
    }

    fn stop_file_present(&self) -> bool {
        self.run.join(session::STOP_FILE).exists()
    }

    /// Run one server subcommand to completion and parse its JSON output.
    fn run_tool(&mut self, name: &str, arguments: Vec<String>) -> Result<serde_json::Value> {
        let stdout_log = self.run.join(format!("{name}.stdout.json"));
        let spec = SpawnSpec {
            executable: self.request.server_path.clone(),
            arguments: self.request.server_arguments(arguments),
            working_directory: self.data.clone(),
            stdout_log: stdout_log.clone(),
            stderr_log: self.run.join(format!("{name}.stderr.log")),
            environment: Vec::new(),
            capture_first_line: false,
        };
        let mut child = self.platform.spawn(&spec)?;
        let source = if name == "create-account" {
            DiagnosticSource::CreateAccount
        } else {
            DiagnosticSource::Prepare
        };
        let mut diagnostics = terminal::Diagnostics::default();
        let mut pass = |events: &mut Emitter, text: &str| {
            if events.enabled() {
                events.diagnostic(source, text);
            } else if !text.is_empty() {
                diagnostics.push(text);
            }
        };
        let deadline = Instant::now() + PREPARATION_DEADLINE;
        while !child.exited() || !child.logs_finished() {
            pass(&mut self.events, &child.drain_diagnostics());
            child.check_logs()?;
            if Instant::now() > deadline || self.stop_file_present() {
                child.kill();
                self.events.flush(source);
                return Err(Failure::session(
                    "Preparation cancelled or exceeded ten minutes.",
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        child.check_logs()?;
        pass(&mut self.events, &child.drain_diagnostics());
        self.events.flush(source);
        diagnostics.finish();
        if child.exit_code() != Some(0) || child.output_truncated() {
            return Err(Failure::session(format!(
                "{name} failed; see {}.",
                self.run.join(format!("{name}.stderr.log")).display()
            )));
        }
        let bytes = std::fs::read(&stdout_log)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Failure::session(format!("{name} produced invalid output: {e}")))
    }

    fn ensure_account(&mut self, prepared: &Prepared) -> Result<AccountRecord> {
        let account_path = self.data.join("account.json");
        let state = self.data.join("artifacts").join("account");
        terminal::heading("Your driver");
        if account_path.exists() {
            terminal::step("Opening your saved driver and garage...");
            let account: AccountRecord = records::read_json(&account_path)?;
            account.validate()?;
            if !state.exists() {
                return Err(Failure::refused(
                    "Existing account record is invalid; no state was replaced.",
                ));
            }
            terminal::success("Your existing save is ready. Progress and balance are unchanged.");
            return Ok(account);
        }
        if state.exists() {
            return Err(Failure::refused(
                "An account directory exists without its launcher record. Preserve it for recovery; it will not be overwritten.",
            ));
        }
        let policy = prepared
            .files
            .get("account-policy")
            .ok_or_else(|| Failure::session("prepare-local did not produce an account policy."))?
            .clone();
        let game = self.game_directory()?.to_path_buf();
        terminal::step("Creating your local tester account and starter garage...");
        let created = self.run_tool(
            "create-account",
            vec![
                "create-account".into(),
                "--name".into(),
                self.request.player_name.clone(),
                "--state-directory".into(),
                state.to_string_lossy().into_owned(),
                "--account-policy".into(),
                policy.to_string_lossy().into_owned(),
                "--game-dir".into(),
                game.to_string_lossy().into_owned(),
                "--content-cache".into(),
                self.data
                    .join("artifacts")
                    .join("content")
                    .to_string_lossy()
                    .into_owned(),
                "--progression".into(),
                "post-intro".into(),
                "--starting-resources".into(),
                "tester".into(),
            ],
        )?;
        let id = created["local_account"]
            .as_str()
            .filter(|id| records::is_id(id))
            .ok_or_else(|| Failure::session("create-account did not report a valid account."))?;
        let account = AccountRecord::post_intro(id.to_owned());
        records::write_new_json(&account_path, &account)?;
        terminal::success("Your local save is ready. Future launches keep this driver and garage.");
        if let Some(level) = created["starting_level"].as_u64() {
            terminal::detail(
                "Starting REP",
                format_args!("level {level} (one below the cap)"),
            );
            terminal::detail("Starting cash", "$10,000,000");
        }
        Ok(account)
    }

    fn server_arguments(
        &self,
        prepared: &Prepared,
        account: &AccountRecord,
    ) -> Result<Vec<String>> {
        let output = self
            .data
            .join("artifacts")
            .join("runs")
            .join(&self.request.id);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let game = self.game_directory()?;
        let mut args: Vec<String> = vec![
            "--root".into(),
            self.data.to_string_lossy().into_owned(),
            "--output".into(),
            output.to_string_lossy().into_owned(),
            "--game-dir".into(),
            game.to_string_lossy().into_owned(),
            "--state-directory".into(),
            "artifacts/account".into(),
            "--local-account".into(),
            account.local_account.clone(),
            "--stop-file".into(),
            self.run
                .join(session::STOP_SERVER_FILE)
                .to_string_lossy()
                .into_owned(),
            "--owned-local-social".into(),
            "--owned-menu-awards".into(),
            "--owned-world-readiness".into(),
            "--owned-world-attributes".into(),
            "--owned-world-connection".into(),
            "--owned-only".into(),
        ];
        if self.request.action == Action::Start {
            // The game expects the launcher services on this port.
            args.push("--lsx-port".into());
            args.push(platform::LSX_PORT.to_string());
        }
        for (name, path) in &prepared.files {
            if name != "account-policy" {
                args.push(format!("--{name}"));
                args.push(path.to_string_lossy().into_owned());
            }
        }
        if self.request.record_protocol {
            args.push("--record-protocol".into());
        }
        Ok(args)
    }

    fn parse_ready(&self, line: &str, pid: u32) -> Result<ServerReady> {
        let ready: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| Failure::session(format!("Server readiness record is invalid: {e}")))?;
        if ready["ready"] != true || ready["pid"] != pid {
            return Err(Failure::session("Server readiness identity mismatch."));
        }
        if self.request.action == Action::Start
            && ready["lsx"] != format!("127.0.0.1:{}", platform::LSX_PORT)
        {
            return Err(Failure::session("Server readiness identity mismatch."));
        }
        let redirector: std::net::SocketAddr = ready["redirector"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Failure::session("Server redirector is not loopback."))?;
        if redirector.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
            return Err(Failure::session("Server redirector is not loopback."));
        }
        Ok(ServerReady {
            redirector_port: redirector.port(),
        })
    }
}

impl Steps for LiveSession {
    fn status(&mut self, state: State, message: &str) {
        self.echo_diagnostics();
        match state {
            State::Preparing => terminal::heading("Preparing your game data"),
            State::Starting => terminal::heading("Starting the game"),
            State::Activation => terminal::heading("EA activation needs your attention"),
            State::Ready => terminal::heading(if self.request.action == Action::Serve {
                "Server is running"
            } else {
                "Ready to play"
            }),
            State::Stopping => terminal::heading("Closing your session"),
            State::Finished => terminal::heading("All done"),
            State::Failed | State::Interrupted => terminal::heading("Session stopped"),
        }
        match state {
            State::Ready | State::Finished => terminal::success(message),
            State::Activation => terminal::action(message),
            State::Failed | State::Interrupted => terminal::error(message),
            _ => terminal::step(message),
        }
        let record = Status {
            version: records::VERSION,
            id: self.request.id.clone(),
            utc: clock::now_iso(),
            state,
            message: message.to_owned(),
            worker_pid: std::process::id(),
            game_pid: self.game.as_ref().map(|g| g.id()),
            server_pid: self.server.as_ref().map(|s| s.id()),
        };
        if let Err(e) = records::replace_json(&self.run.join("status.json"), &record) {
            terminal::error(format_args!("Could not save the session status: {e}"));
        }
        if matches!(state, State::Finished | State::Failed) {
            // Lines a child left unterminated precede the final state.
            self.events.flush_all();
        }
        self.events.status(&record);
    }

    fn stop_requested(&mut self) -> bool {
        nfs_afterhours_sys::STOP_REQUESTED.load(std::sync::atomic::Ordering::SeqCst)
            || self.stop_file_present()
    }

    fn elapsed(&mut self) -> Duration {
        self.started.elapsed()
    }

    fn wait(&mut self) {
        self.echo_diagnostics();
        std::thread::sleep(Duration::from_millis(250));
    }

    fn prepare(&mut self) -> Result<()> {
        let game = self.game_directory()?.to_path_buf();
        let report = self.run_tool(
            "prepare",
            vec![
                "prepare-local".into(),
                "--root".into(),
                self.data.to_string_lossy().into_owned(),
                "--game-dir".into(),
                game.to_string_lossy().into_owned(),
            ],
        )?;
        let files = report["files"]
            .as_object()
            .ok_or_else(|| Failure::session("prepare-local reported no files."))?
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|p| (k.clone(), PathBuf::from(p))))
            .collect();
        let prepared = Prepared { files };
        let account = self.ensure_account(&prepared)?;
        self.prepared = Some(prepared);
        self.account = Some(account);
        Ok(())
    }

    fn start_server(&mut self) -> Result<()> {
        terminal::heading("Starting your local server");
        terminal::step("Starting local services. This may take a moment...");
        let prepared = self.prepared.clone().unwrap_or_default();
        let account = self
            .account
            .clone()
            .ok_or_else(|| Failure::internal("account not prepared"))?;
        let executable = if self.request.action == Action::Start {
            let registration = self
                .platform
                .registered_client_path()?
                .ok_or_else(|| Failure::refused("Existing game launcher registration is missing or unsupported. Afterhours does not install or modify it."))?;
            let name = crate::compat::compatibility_name(&registration)?;
            crate::install::install_compatibility_server(&crate::install::Source {
                data_directory: &self.data,
                game_directory: self.request.game_directory.as_deref(),
                server_path: &self.request.server_path,
                server_sha256: &self.request.server_sha256,
                compatibility_name: name,
            })?
        } else {
            self.request.server_path.clone()
        };
        let arguments = self.server_arguments(&prepared, &account)?;
        let spec = SpawnSpec {
            executable: executable.clone(),
            arguments: self.request.server_arguments(arguments),
            working_directory: self.data.clone(),
            stdout_log: self.run.join("server.stdout.jsonl"),
            stderr_log: self.run.join("server.stderr.log"),
            environment: Vec::new(),
            capture_first_line: true,
        };
        let mut server = self.platform.spawn(&spec)?;
        let deadline = Instant::now() + SERVER_READY_DEADLINE;
        let line = loop {
            let text = server.drain_diagnostics();
            self.forward(DiagnosticSource::Server, &text);
            server.check_logs()?;
            if let Some(line) = server.first_line() {
                break line?;
            }
            if server.exited() || Instant::now() > deadline {
                server.kill();
                return Err(Failure::session("Server did not become ready."));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let ready = self.parse_ready(&line, server.id());
        let ready = match ready {
            Ok(ready) => ready,
            Err(e) => {
                server.kill();
                return Err(e);
            }
        };
        self.server_executable = Some(executable);
        self.ready = Some(ready);
        self.server = Some(server);
        terminal::success("Your local server is ready.");
        Ok(())
    }

    fn server_exited(&mut self) -> bool {
        self.server.as_mut().is_none_or(|s| s.exited())
    }

    #[cfg(windows)]
    fn start_game(&mut self) -> Result<()> {
        use crate::platform::windows::lease::{EndpointLease, GAME_BUILD_SHA256};
        let game_dir = self.game_directory()?.to_path_buf();
        let game_path = game_dir.join(platform::GAME_EXECUTABLE);
        if crate::digest::sha256_file(&game_path)? != GAME_BUILD_SHA256 {
            return Err(Failure::session("Game changed after preflight."));
        }
        let date = crate::marker::UtcDate::today();
        let marker = crate::marker::launch_marker(date);
        // This date-derived value is launch context, not an account credential.
        let spec = SpawnSpec {
            executable: game_path.clone(),
            arguments: Vec::new(),
            working_directory: game_dir,
            stdout_log: self.run.join("game.stdout.log"),
            stderr_log: self.run.join("game.stderr.log"),
            environment: vec![("EARtPLaunchCode".into(), marker.to_string())],
            capture_first_line: false,
        };
        let game = self.platform.spawn(&spec)?;
        let server = self
            .server
            .as_ref()
            .ok_or_else(|| Failure::internal("server not started"))?;
        let ready = self
            .ready
            .as_ref()
            .ok_or_else(|| Failure::internal("server not ready"))?;
        let executable = self
            .server_executable
            .clone()
            .ok_or_else(|| Failure::internal("server executable unknown"))?;
        let created = |pid: u32| -> Result<u64> {
            Ok(nfs_afterhours_sys::windows::Process::open(pid, false)?.creation_time()?)
        };
        let lease = EndpointLease::new(
            game.id(),
            created(game.id())?,
            &game_path,
            server.id(),
            created(server.id())?,
            &executable,
            &self.request.server_sha256,
            ready.redirector_port,
        );
        self.game = Some(game);
        self.lease = Some(lease?);
        Ok(())
    }

    #[cfg(not(windows))]
    fn start_game(&mut self) -> Result<()> {
        Err(Failure::refused(
            "Starting the game is supported on Windows only. Use `nfs-afterhours serve` here.",
        ))
    }

    #[cfg(windows)]
    fn poll_game(&mut self) -> Result<GamePoll> {
        use crate::platform::windows::{activation_window_visible, lease::NOT_UNPACKED_YET};
        let helper = self.game_directory()?.join(platform::ACTIVATION_HELPER);
        if let Some(job) = &self.job
            && activation_window_visible(job, &helper)?
        {
            return Ok(GamePoll::Activation);
        }
        if let Some(game) = self.game.as_mut()
            && game.exited()
        {
            return Ok(GamePoll::Exited(game.exit_code()));
        }
        let lease = self
            .lease
            .as_mut()
            .ok_or_else(|| Failure::internal("game not started"))?;
        match lease.prepare() {
            Ok(true) => Ok(GamePoll::RoutingReady),
            Ok(false) => Ok(GamePoll::NotReady),
            // Protected startup code is not unpacked at process creation. Only
            // this exact pre-write condition may be retried; identity and
            // shape failures fail closed.
            Err(e) if e.message == NOT_UNPACKED_YET => Ok(GamePoll::NotReady),
            Err(e) => Err(e),
        }
    }

    #[cfg(not(windows))]
    fn poll_game(&mut self) -> Result<GamePoll> {
        Err(Failure::internal("no game on this platform"))
    }

    #[cfg(windows)]
    fn apply_routing(&mut self) -> Result<()> {
        self.lease
            .as_mut()
            .ok_or_else(|| Failure::internal("lease not prepared"))?
            .apply()
    }

    #[cfg(not(windows))]
    fn apply_routing(&mut self) -> Result<()> {
        Err(Failure::internal("no game on this platform"))
    }

    fn game_exited(&mut self) -> Option<Option<i32>> {
        match self.game.as_mut() {
            None => Some(None),
            Some(game) => {
                if game.exited() {
                    Some(game.exit_code())
                } else {
                    None
                }
            }
        }
    }

    fn close_game(&mut self) -> Result<()> {
        if let Some(game) = self.game.as_mut()
            && !game.exited()
        {
            game.request_close();
            wait_child_exit(game.as_mut(), CLOSE_GRACE)?;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn finish_lease(&mut self) -> Result<()> {
        if let Some(mut lease) = self.lease.take() {
            lease.complete_after_exit()?;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn finish_lease(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop_server(&mut self) -> Result<()> {
        if let Some(server) = self.server.as_mut()
            && !server.exited()
        {
            std::fs::write(self.run.join(session::STOP_SERVER_FILE), b"stop")?;
            wait_child_exit(server.as_mut(), CLOSE_GRACE)?;
        }
        Ok(())
    }

    fn drain_logs(&mut self) -> Result<()> {
        let deadline = Instant::now() + DRAIN_GRACE;
        for source in [DiagnosticSource::Game, DiagnosticSource::Server] {
            let child = match source {
                DiagnosticSource::Game => self.game.as_mut(),
                _ => self.server.as_mut(),
            };
            let Some(child) = child else { continue };
            while !child.logs_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            child.check_logs()?;
            if !child.logs_finished() {
                return Err(Failure::session(
                    "Child diagnostics did not finish after process exit.",
                ));
            }
            let mut text = String::new();
            loop {
                let chunk = child.drain_diagnostics();
                if chunk.is_empty() {
                    break;
                }
                text.push_str(&chunk);
            }
            self.forward(source, &text);
            self.events.flush(source);
        }
        Ok(())
    }

    #[cfg(windows)]
    fn finish_guard(&mut self) -> Result<()> {
        use crate::platform::SessionGuard;
        if let Some(job) = self.job.as_mut() {
            job.finish()?;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn finish_guard(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Validate a request file and its location, returning the request, the
/// data directory and the session directory.
pub fn load_request(path: &Path) -> Result<(Request, PathBuf, PathBuf)> {
    let request: Request = records::read_json(path)?;
    request.validate()?;
    let data =
        paths::checked_data_path(&request.data_directory, request.game_directory.as_deref())?;
    let run = paths::session_dir(&data, &request.id);
    paths::checked_data_path(&run, request.game_directory.as_deref())?;
    if paths::normalize(path)? != run.join("request.json") {
        return Err(Failure::refused("Launch request directory mismatch."));
    }
    Ok((request, data, run))
}
