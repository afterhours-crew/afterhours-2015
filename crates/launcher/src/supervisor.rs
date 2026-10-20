// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The session state machine.
//!
//! A session moves through `preparing`, `starting`, `activation`, `ready`
//! and `stopping` to `finished` or `failed`. This module owns the order of
//! those steps, the deadlines, how a stop request or a child exit is
//! handled at each point, and the cleanup that always runs afterwards:
//! close the game, retire the routing lease, stop the server, drain the
//! logs, disarm the session guard. It performs none of those actions itself.
//! It asks a [`Steps`] implementation to, which is a real process tree in
//! [`live`] and a recording fake in the tests. That is what lets the
//! ordering, cancellation and cleanup rules be tested without a game.
//!
//! A `serve` session can also host the game: a play request (`nfs-afterhours
//! play`) runs the same game steps as `start`, inside the same session and
//! job. When that game exits, its routing lease is finished and its logs are
//! drained, and the session serves on until it is stopped.

pub mod live;

use crate::{
    exit::{Failure, Result},
    records::{Action, State},
};
use std::time::Duration;

/// How long the game may take to reach guarded routing. Reset while the
/// activation dialog is visible, because a human sign-in has no timer.
pub const ROUTING_DEADLINE: Duration = Duration::from_secs(120);

/// What polling the game found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamePoll {
    /// The installed activation helper shows a window; wait for the user.
    Activation,
    /// The game is running but its startup root is not ready yet.
    NotReady,
    /// The lease is prepared; routing can be applied.
    RoutingReady,
    /// The game exited before routing, with this exit code if known.
    Exited(Option<i32>),
}

/// The actions a session performs, in the order the machine calls them.
/// Cleanup steps must tolerate being called when the thing they clean up
/// never started.
pub trait Steps {
    /// Record a state change for `status` to read and the console to show.
    fn status(&mut self, state: State, message: &str);
    /// A stop file or console interrupt is pending.
    fn stop_requested(&mut self) -> bool;
    /// Time since the session started.
    fn elapsed(&mut self) -> Duration;
    /// Pause between polls.
    fn wait(&mut self);
    /// Generate content and policy, then ensure the account exists.
    fn prepare(&mut self) -> Result<()>;
    /// Start the server and wait for its readiness record.
    fn start_server(&mut self) -> Result<()>;
    /// Whether the server has exited.
    fn server_exited(&mut self) -> bool;
    /// Start the game and pin it for routing.
    fn start_game(&mut self) -> Result<()>;
    /// Check the activation helper, the game's lifetime and lease readiness.
    fn poll_game(&mut self) -> Result<GamePoll>;
    /// Write the loopback endpoint into the game.
    fn apply_routing(&mut self) -> Result<()>;
    /// `Some(code)` once the game has exited.
    fn game_exited(&mut self) -> Option<Option<i32>>;
    /// Ask the game to close, then force it after a bounded wait.
    fn close_game(&mut self) -> Result<()>;
    /// Confirm the game lifetime ended and release the lease.
    fn finish_lease(&mut self) -> Result<()>;
    /// A play request for a serving session is pending; consume it.
    fn play_requested(&mut self) -> bool;
    /// After the game exited in a serving session: finish its lease, drain
    /// and release its logs, so the session serves on or starts another.
    fn retire_game(&mut self) -> Result<()>;
    /// Ask the server to stop, then force it after a bounded wait.
    fn stop_server(&mut self) -> Result<()>;
    /// Wait for the log pumps and report any fault.
    fn drain_logs(&mut self) -> Result<()>;
    /// Disarm the session guard once every child has exited.
    fn finish_guard(&mut self) -> Result<()>;
}

/// Final result of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub state: State,
    pub message: String,
}

impl Outcome {
    pub fn succeeded(&self) -> bool {
        self.state == State::Finished
    }
}

fn game_exit_message(code: Option<i32>, suffix: &str) -> String {
    match code {
        Some(code) => format!("Game exited with code 0x{:08X}. {suffix}", code as u32),
        None => format!("Game exited without an exit code. {suffix}"),
    }
}

fn finished_message(action: Action) -> &'static str {
    match action {
        Action::Setup => "Setup complete. Your local driver starts after the story introduction.",
        Action::Serve => "Server closed. Your local save is kept for next time.",
        Action::Start => "Game and server closed. Your local save is kept for next time.",
    }
}

/// Run a session to completion and return its outcome. Cleanup always
/// runs, and a cleanup failure is reported together with any earlier one.
pub fn run(steps: &mut impl Steps, action: Action) -> Outcome {
    let failure = session(steps, action).err();
    let cleanup = cleanup(steps, action).err();
    let outcome = match (failure, cleanup) {
        (None, None) => Outcome {
            state: State::Finished,
            message: finished_message(action).to_owned(),
        },
        (Some(failure), None) => Outcome {
            state: State::Failed,
            message: failure.message,
        },
        (failure, Some(cleanup)) => {
            let cleanup = format!(
                "{} The session job remains armed for cleanup on supervisor exit.",
                cleanup.message
            );
            Outcome {
                state: State::Failed,
                message: match failure {
                    Some(failure) => format!("{} Cleanup: {cleanup}", failure.message),
                    None => cleanup,
                },
            }
        }
    };
    steps.status(outcome.state, &outcome.message);
    outcome
}

fn session(steps: &mut impl Steps, action: Action) -> Result<()> {
    steps.status(
        State::Preparing,
        "Reading your installed game data and checking the local cache.",
    );
    if steps.stop_requested() {
        return Err(Failure::session("Start cancelled before preparation."));
    }
    steps.prepare()?;
    if action == Action::Setup {
        return Ok(());
    }
    steps.start_server()?;
    if action == Action::Serve {
        return serve(steps);
    }
    open_game(steps)?;
    loop {
        if steps.server_exited() {
            return Err(Failure::session("Local server exited unexpectedly."));
        }
        if steps.stop_requested() {
            steps.status(State::Stopping, "Closing game before server.");
            steps.close_game()?;
            return Ok(());
        }
        if let Some(code) = steps.game_exited() {
            return if code == Some(0) {
                Ok(())
            } else {
                Err(Failure::session(game_exit_message(
                    code,
                    "See Windows Application events and this session directory.",
                )))
            };
        }
        steps.wait();
    }
}

/// Start the game, wait for routing and apply it: the part of `start` that
/// follows the server, shared with a play request.
fn open_game(steps: &mut impl Steps) -> Result<()> {
    steps.status(
        State::Starting,
        "Opening Need for Speed. Wait for 'Ready to play' here before pressing Space in the game.",
    );
    steps.start_game()?;
    wait_for_routing(steps)?;
    steps.apply_routing()?;
    steps.status(
        State::Ready,
        "Switch to the game and press Space to connect to your local server. Keep this window open while you play.",
    );
    Ok(())
}

fn serve(steps: &mut impl Steps) -> Result<()> {
    steps.status(
        State::Ready,
        "Server is ready. Stop it with Ctrl+C or `nfs-afterhours stop`.",
    );
    loop {
        if steps.server_exited() {
            return Err(Failure::session("Local server exited unexpectedly."));
        }
        if steps.stop_requested() {
            steps.status(State::Stopping, "Stopping server.");
            return Ok(());
        }
        if steps.play_requested() {
            match play(steps)? {
                Played::Stopped => return Ok(()),
                Played::Ended(message) => steps.status(State::Ready, &message),
            }
            continue;
        }
        steps.wait();
    }
}

/// How a game started from a serving session ended.
enum Played {
    /// A stop request closed the game; the session ends.
    Stopped,
    /// The game exited or could not start; serve on with this message.
    Ended(String),
}

/// Host the game in a serving session. A game that fails to start or exits
/// returns the session to serving once the game is closed and its lease
/// finished. A stop closes the game first, as in `start`. A failure to
/// close or retire the game, or a server exit, ends the session, and the
/// session cleanup takes over.
fn play(steps: &mut impl Steps) -> Result<Played> {
    if let Err(failure) = open_game(steps) {
        let stopping = steps.stop_requested();
        if stopping {
            steps.status(State::Stopping, "Closing game before server.");
        }
        if steps.game_exited().is_none() {
            steps.close_game()?;
        }
        steps.retire_game()?;
        return Ok(if stopping {
            Played::Stopped
        } else {
            Played::Ended(format!("{} The server is still running.", failure.message))
        });
    }
    loop {
        if steps.server_exited() {
            return Err(Failure::session("Local server exited unexpectedly."));
        }
        if steps.stop_requested() {
            steps.status(State::Stopping, "Closing game before server.");
            steps.close_game()?;
            steps.retire_game()?;
            return Ok(Played::Stopped);
        }
        if let Some(code) = steps.game_exited() {
            steps.retire_game()?;
            return Ok(Played::Ended(if code == Some(0) {
                "Game closed. The server is still running; `nfs-afterhours play` starts the game again."
                    .to_owned()
            } else {
                game_exit_message(code, "The server is still running.")
            }));
        }
        steps.wait();
    }
}

fn wait_for_routing(steps: &mut impl Steps) -> Result<()> {
    let mut deadline = steps.elapsed() + ROUTING_DEADLINE;
    let mut waiting_activation = false;
    loop {
        if steps.stop_requested() {
            return Err(Failure::session("Start cancelled."));
        }
        if steps.server_exited() {
            return Err(Failure::session("Local server exited during startup."));
        }
        match steps.poll_game()? {
            GamePoll::Activation => {
                if !waiting_activation {
                    steps.status(
                        State::Activation,
                        "Complete the EA sign-in or activation in its own window. It may need Internet access.\n         Take your time; Afterhours will wait. It never sees your password.\n         Keep this window open, or press Ctrl+C here to cancel. The game is not ready yet.",
                    );
                    waiting_activation = true;
                }
                // A human sign-in has no timer; keep cancellation available.
                deadline = steps.elapsed() + ROUTING_DEADLINE;
                steps.wait();
            }
            GamePoll::Exited(code) => {
                return Err(Failure::session(game_exit_message(
                    code,
                    "The game exited before local routing. Any activation handoff is outside this local session; cleanup closes its owned helpers. Inspect the installed activation log before retrying.",
                )));
            }
            GamePoll::NotReady => {
                if waiting_activation {
                    steps.status(
                        State::Starting,
                        "The activation window closed. Waiting for the game to finish starting...",
                    );
                    waiting_activation = false;
                }
                if steps.elapsed() > deadline {
                    return Err(Failure::session(
                        "Server exited or startup routing did not become ready.",
                    ));
                }
                steps.wait();
            }
            GamePoll::RoutingReady => return Ok(()),
        }
    }
}

fn cleanup(steps: &mut impl Steps, action: Action) -> Result<()> {
    if action == Action::Setup {
        // Preparation owns short-lived children too. Check that all of them
        // have exited before disarming the job, just as for start and serve.
        return steps.finish_guard();
    }
    // A serving session may still host a game it started on request; a
    // lease that was already finished, or never created, is a no-op.
    if steps.game_exited().is_none() {
        steps.close_game()?;
    }
    steps.finish_lease()?;
    if !steps.server_exited() {
        steps.stop_server()?;
    }
    // A process exit can precede the last asynchronous log write; drain
    // after both children are down.
    steps.drain_logs()?;
    steps.finish_guard()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records every call, lets a test fail any step and scripts the game.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        statuses: Vec<(State, String)>,
        fail: Option<(&'static str, &'static str)>,
        stop_at_call: Option<usize>,
        polls: std::collections::VecDeque<GamePoll>,
        game_exit: Option<Option<i32>>,
        game_exits_after_ready: Option<Option<i32>>,
        server_exits_after_ready: bool,
        /// The server exits once a started game is ready.
        server_exits_after_game: bool,
        server_started: bool,
        game_started: bool,
        /// Answers to successive play-request checks; then `false`.
        plays: std::collections::VecDeque<bool>,
        /// A stop is requested once this many statuses were recorded.
        stop_after_status: Option<usize>,
        now: Duration,
        tick: Duration,
    }

    impl Fake {
        fn step(&mut self, name: &'static str) -> Result<()> {
            self.calls.push(name.to_owned());
            match self.fail {
                Some((at, message)) if at == name => Err(Failure::session(message)),
                _ => Ok(()),
            }
        }
        fn names(&self) -> Vec<&str> {
            self.calls.iter().map(String::as_str).collect()
        }
        fn states(&self) -> Vec<State> {
            self.statuses.iter().map(|(s, _)| *s).collect()
        }
    }

    impl Steps for Fake {
        fn status(&mut self, state: State, message: &str) {
            self.calls.push(format!("status:{state}"));
            self.statuses.push((state, message.to_owned()));
        }
        fn stop_requested(&mut self) -> bool {
            self.calls.push("stop_requested".into());
            self.stop_at_call.is_some_and(|at| self.calls.len() >= at)
                || self
                    .stop_after_status
                    .is_some_and(|at| self.statuses.len() >= at)
        }
        fn elapsed(&mut self) -> Duration {
            self.now
        }
        fn wait(&mut self) {
            self.calls.push("wait".into());
            self.now += self.tick;
            if self
                .statuses
                .last()
                .is_some_and(|(s, _)| *s == State::Ready)
            {
                if self.game_started
                    && let Some(code) = self.game_exits_after_ready.take()
                {
                    self.game_exit = Some(code);
                }
                if self.server_exits_after_ready
                    || (self.game_started && self.server_exits_after_game)
                {
                    self.server_started = false;
                }
            }
        }
        fn prepare(&mut self) -> Result<()> {
            self.step("prepare")
        }
        fn start_server(&mut self) -> Result<()> {
            self.step("start_server")?;
            self.server_started = true;
            Ok(())
        }
        fn server_exited(&mut self) -> bool {
            !self.server_started
        }
        fn start_game(&mut self) -> Result<()> {
            self.step("start_game")?;
            self.game_started = true;
            self.game_exit = None;
            Ok(())
        }
        fn poll_game(&mut self) -> Result<GamePoll> {
            self.step("poll_game")?;
            let poll = self.polls.pop_front().unwrap_or(GamePoll::NotReady);
            // As in the live session, an observed exit stays observed.
            if let GamePoll::Exited(code) = poll {
                self.game_exit = Some(code);
            }
            Ok(poll)
        }
        fn apply_routing(&mut self) -> Result<()> {
            self.step("apply_routing")
        }
        fn game_exited(&mut self) -> Option<Option<i32>> {
            if !self.game_started {
                return Some(None);
            }
            self.game_exit
        }
        fn close_game(&mut self) -> Result<()> {
            self.step("close_game")?;
            self.game_exit = Some(Some(0));
            Ok(())
        }
        fn finish_lease(&mut self) -> Result<()> {
            self.step("finish_lease")
        }
        fn play_requested(&mut self) -> bool {
            let requested = self.plays.pop_front().unwrap_or(false);
            if requested {
                self.calls.push("play".into());
            }
            requested
        }
        fn retire_game(&mut self) -> Result<()> {
            self.step("retire_game")?;
            self.game_started = false;
            self.game_exit = None;
            Ok(())
        }
        fn stop_server(&mut self) -> Result<()> {
            self.step("stop_server")?;
            self.server_started = false;
            Ok(())
        }
        fn drain_logs(&mut self) -> Result<()> {
            self.step("drain_logs")
        }
        fn finish_guard(&mut self) -> Result<()> {
            self.step("finish_guard")
        }
    }

    fn fake() -> Fake {
        Fake {
            tick: Duration::from_millis(250),
            ..Fake::default()
        }
    }

    #[test]
    fn setup_prepares_and_finishes_without_starting_anything() {
        let mut f = fake();
        let outcome = run(&mut f, Action::Setup);
        assert!(outcome.succeeded());
        assert_eq!(
            f.names(),
            [
                "status:preparing",
                "stop_requested",
                "prepare",
                "finish_guard",
                "status:finished"
            ]
        );
        assert!(outcome.message.contains("Setup complete"));
    }

    #[test]
    fn setup_reports_cleanup_failure_and_keeps_the_job_armed() {
        let mut f = fake();
        f.fail = Some(("finish_guard", "preparation child still running"));
        let outcome = run(&mut f, Action::Setup);
        assert_eq!(outcome.state, State::Failed);
        assert!(outcome.message.contains("preparation child still running"));
        assert!(outcome.message.contains("job remains armed"));
    }

    #[test]
    fn a_full_start_passes_through_every_state_and_cleans_up_in_order() {
        let mut f = fake();
        f.polls = [GamePoll::NotReady, GamePoll::RoutingReady].into();
        f.game_exits_after_ready = Some(Some(0));
        let outcome = run(&mut f, Action::Start);
        assert!(outcome.succeeded(), "{outcome:?}");
        assert_eq!(
            f.states(),
            [
                State::Preparing,
                State::Starting,
                State::Ready,
                State::Finished
            ]
        );
        let tail: Vec<&str> = f
            .names()
            .into_iter()
            .filter(|c| {
                matches!(
                    *c,
                    "apply_routing"
                        | "close_game"
                        | "finish_lease"
                        | "stop_server"
                        | "drain_logs"
                        | "finish_guard"
                )
            })
            .collect();
        assert_eq!(
            tail,
            [
                "apply_routing",
                "finish_lease",
                "stop_server",
                "drain_logs",
                "finish_guard"
            ],
            "a game that exited on its own is not closed again"
        );
    }

    #[test]
    fn activation_pauses_the_deadline_and_is_reported_once() {
        let mut f = fake();
        f.tick = Duration::from_secs(50);
        f.polls = [
            GamePoll::Activation,
            GamePoll::Activation,
            GamePoll::Activation,
            GamePoll::NotReady,
            GamePoll::RoutingReady,
        ]
        .into();
        f.game_exits_after_ready = Some(Some(0));
        let outcome = run(&mut f, Action::Start);
        assert!(outcome.succeeded(), "{outcome:?}");
        let states = f.states();
        assert_eq!(
            states.iter().filter(|s| **s == State::Activation).count(),
            1
        );
        assert_eq!(
            states,
            [
                State::Preparing,
                State::Starting,
                State::Activation,
                State::Starting,
                State::Ready,
                State::Finished
            ]
        );
    }

    #[test]
    fn routing_that_never_becomes_ready_times_out() {
        let mut f = fake();
        f.tick = Duration::from_secs(30);
        let outcome = run(&mut f, Action::Start);
        assert_eq!(outcome.state, State::Failed);
        assert!(outcome.message.contains("did not become ready"));
        assert!(
            f.names().contains(&"close_game"),
            "the running game is closed"
        );
        assert!(f.names().ends_with(&[
            "stop_server",
            "drain_logs",
            "finish_guard",
            "status:failed"
        ]));
    }

    #[test]
    fn a_game_that_exits_before_routing_fails_with_its_code() {
        let mut f = fake();
        f.polls = [GamePoll::Exited(Some(0x1234))].into();
        let outcome = run(&mut f, Action::Start);
        assert_eq!(outcome.state, State::Failed);
        assert!(
            outcome
                .message
                .starts_with("Game exited with code 0x00001234.")
        );
        assert!(
            !f.names().contains(&"close_game"),
            "an exited game is not closed again"
        );
    }

    #[test]
    fn a_nonzero_game_exit_after_ready_is_a_failure() {
        let mut f = fake();
        f.polls = [GamePoll::RoutingReady].into();
        f.game_exits_after_ready = Some(Some(-1));
        let outcome = run(&mut f, Action::Start);
        assert_eq!(outcome.state, State::Failed);
        assert!(
            outcome
                .message
                .starts_with("Game exited with code 0xFFFFFFFF.")
        );
    }

    #[test]
    fn a_stop_request_while_ready_closes_the_game_then_the_server() {
        let mut f = fake();
        f.polls = [GamePoll::RoutingReady].into();
        // Trigger the stop on the first poll after reaching ready.
        f.stop_at_call = Some(12);
        let outcome = run(&mut f, Action::Start);
        assert!(outcome.succeeded(), "{outcome:?}");
        assert!(f.states().contains(&State::Stopping));
        let names = f.names();
        let close = names.iter().position(|c| *c == "close_game").unwrap();
        let stop = names.iter().position(|c| *c == "stop_server").unwrap();
        assert!(close < stop);
        assert_eq!(names.iter().filter(|c| **c == "close_game").count(), 1);
    }

    #[test]
    fn every_failing_step_still_runs_cleanup_and_reports_the_failure() {
        for step in [
            "prepare",
            "start_server",
            "start_game",
            "poll_game",
            "apply_routing",
        ] {
            let mut f = fake();
            f.polls = [GamePoll::RoutingReady].into();
            f.fail = Some((step, "boom"));
            let outcome = run(&mut f, Action::Start);
            assert_eq!(outcome.state, State::Failed, "{step}");
            assert_eq!(outcome.message, "boom", "{step}");
            let names = f.names();
            assert!(
                names.contains(&"drain_logs") && names.contains(&"finish_guard"),
                "{step}: {names:?}"
            );
            assert_eq!(names.last().copied(), Some("status:failed"));
        }
    }

    #[test]
    fn a_cleanup_failure_is_appended_to_the_outcome() {
        let mut f = fake();
        f.polls = [GamePoll::RoutingReady].into();
        f.game_exits_after_ready = Some(Some(0));
        f.fail = Some(("stop_server", "server stuck"));
        let outcome = run(&mut f, Action::Start);
        assert_eq!(outcome.state, State::Failed);
        assert!(
            outcome
                .message
                .starts_with("server stuck The session job remains armed")
        );
        let mut g = fake();
        g.polls = [GamePoll::Exited(None)].into();
        g.fail = Some(("drain_logs", "pump died"));
        // Exited(None) also fails the session, so both messages appear.
        let outcome = run(&mut g, Action::Start);
        assert!(outcome.message.contains("Cleanup: pump died"));
        assert!(
            !g.names().contains(&"finish_guard"),
            "cleanup stops at its first failure"
        );
    }

    #[test]
    fn serve_runs_until_a_stop_or_the_server_exits() {
        let mut f = fake();
        f.stop_at_call = Some(8);
        let outcome = run(&mut f, Action::Serve);
        assert!(outcome.succeeded(), "{outcome:?}");
        assert_eq!(
            f.states(),
            [
                State::Preparing,
                State::Ready,
                State::Stopping,
                State::Finished
            ]
        );
        assert!(
            !f.names().iter().any(|c| c.contains("game") || *c == "play"),
            "a serving session without a play request never touches a game"
        );
        let mut g = fake();
        g.server_exits_after_ready = true;
        let outcome = run(&mut g, Action::Serve);
        assert_eq!(outcome.state, State::Failed);
        assert!(outcome.message.contains("server exited unexpectedly"));
        assert!(
            !g.names().contains(&"stop_server"),
            "an exited server is not stopped again"
        );
    }

    /// Calls among the game, lease and shutdown steps, in order.
    fn game_calls(f: &Fake) -> Vec<&str> {
        f.names()
            .into_iter()
            .filter(|c| {
                matches!(
                    *c,
                    "play"
                        | "start_game"
                        | "apply_routing"
                        | "close_game"
                        | "retire_game"
                        | "finish_lease"
                        | "stop_server"
                        | "drain_logs"
                        | "finish_guard"
                )
            })
            .collect()
    }

    #[test]
    fn a_played_game_that_exits_returns_the_session_to_serving() {
        let mut f = fake();
        f.plays = [false, true].into();
        f.polls = [GamePoll::NotReady, GamePoll::RoutingReady].into();
        f.game_exits_after_ready = Some(Some(0));
        // Stop once the session is serving again after the game.
        f.stop_after_status = Some(5);
        let outcome = run(&mut f, Action::Serve);
        assert!(outcome.succeeded(), "{outcome:?}");
        assert_eq!(
            f.states(),
            [
                State::Preparing,
                State::Ready,
                State::Starting,
                State::Ready,
                State::Ready,
                State::Stopping,
                State::Finished
            ]
        );
        assert!(f.statuses[4].1.contains("The server is still running"));
        assert_eq!(
            game_calls(&f),
            [
                "play",
                "start_game",
                "apply_routing",
                "retire_game",
                "finish_lease",
                "stop_server",
                "drain_logs",
                "finish_guard"
            ],
            "an exited game is retired, not closed, and the server keeps running"
        );
    }

    #[test]
    fn a_crashed_game_reports_its_code_and_the_session_serves_on() {
        let mut f = fake();
        f.plays = [true].into();
        f.polls = [GamePoll::RoutingReady].into();
        f.game_exits_after_ready = Some(Some(-1073741819));
        f.stop_after_status = Some(5);
        let outcome = run(&mut f, Action::Serve);
        assert!(outcome.succeeded(), "{outcome:?}");
        assert_eq!(f.states()[4], State::Ready);
        assert!(
            f.statuses[4].1.contains("0xC0000005"),
            "{:?}",
            f.statuses[4]
        );
        assert!(f.statuses[4].1.contains("still running"));
    }

    #[test]
    fn play_then_stop_closes_the_game_before_the_server() {
        let mut f = fake();
        f.plays = [true].into();
        f.polls = [GamePoll::RoutingReady].into();
        // Stop while the game is ready.
        f.stop_after_status = Some(4);
        let outcome = run(&mut f, Action::Serve);
        assert!(outcome.succeeded(), "{outcome:?}");
        assert_eq!(
            f.states(),
            [
                State::Preparing,
                State::Ready,
                State::Starting,
                State::Ready,
                State::Stopping,
                State::Finished
            ]
        );
        assert_eq!(
            game_calls(&f),
            [
                "play",
                "start_game",
                "apply_routing",
                "close_game",
                "retire_game",
                "finish_lease",
                "stop_server",
                "drain_logs",
                "finish_guard"
            ]
        );
    }

    #[test]
    fn a_stop_during_play_startup_closes_the_game_and_ends_the_session() {
        let mut f = fake();
        f.plays = [true].into();
        // Routing never becomes ready; the stop arrives while starting.
        f.stop_after_status = Some(3);
        let outcome = run(&mut f, Action::Serve);
        assert!(outcome.succeeded(), "{outcome:?}");
        assert_eq!(
            f.states(),
            [
                State::Preparing,
                State::Ready,
                State::Starting,
                State::Stopping,
                State::Finished
            ]
        );
        assert_eq!(
            game_calls(&f),
            [
                "play",
                "start_game",
                "close_game",
                "retire_game",
                "finish_lease",
                "stop_server",
                "drain_logs",
                "finish_guard"
            ]
        );
    }

    #[test]
    fn a_game_that_cannot_start_leaves_the_session_serving() {
        // Failing to start, a routing timeout and an early exit each return
        // to serving with the reason; only a started game is closed.
        for (fail, polls, closed) in [
            (
                Some(("start_game", "Game changed after preflight.")),
                vec![],
                false,
            ),
            (None, vec![], true),
            (None, vec![GamePoll::Exited(Some(1))], false),
        ] {
            let mut f = fake();
            f.tick = Duration::from_secs(50);
            f.plays = [true].into();
            f.polls = polls.into();
            f.fail = fail;
            f.stop_after_status = Some(4);
            let outcome = run(&mut f, Action::Serve);
            assert!(outcome.succeeded(), "{fail:?}: {outcome:?}");
            assert_eq!(
                f.states(),
                [
                    State::Preparing,
                    State::Ready,
                    State::Starting,
                    State::Ready,
                    State::Stopping,
                    State::Finished
                ],
                "{fail:?}"
            );
            assert!(f.statuses[3].1.ends_with("The server is still running."));
            let names = f.names();
            assert_eq!(names.contains(&"close_game"), closed, "{fail:?}: {names:?}");
            assert!(names.contains(&"retire_game"));
            assert!(!names.contains(&"apply_routing"));
        }
    }

    #[test]
    fn a_session_can_play_again_after_a_game() {
        let mut f = fake();
        f.plays = [true, true].into();
        f.polls = [GamePoll::RoutingReady, GamePoll::RoutingReady].into();
        f.game_exits_after_ready = Some(Some(0));
        // Second game: stopped while ready.
        f.stop_after_status = Some(7);
        let outcome = run(&mut f, Action::Serve);
        assert!(outcome.succeeded(), "{outcome:?}");
        let names = f.names();
        assert_eq!(names.iter().filter(|c| **c == "start_game").count(), 2);
        assert_eq!(names.iter().filter(|c| **c == "retire_game").count(), 2);
        assert_eq!(names.iter().filter(|c| **c == "close_game").count(), 1);
    }

    #[test]
    fn a_server_exit_or_retire_failure_during_play_ends_the_session() {
        let mut f = fake();
        f.plays = [true].into();
        f.polls = [GamePoll::RoutingReady].into();
        f.server_exits_after_game = true;
        let outcome = run(&mut f, Action::Serve);
        assert_eq!(outcome.state, State::Failed);
        assert!(outcome.message.contains("server exited unexpectedly"));
        let calls = game_calls(&f);
        assert_eq!(
            calls[calls.len() - 4..],
            ["close_game", "finish_lease", "drain_logs", "finish_guard"],
            "cleanup closes the game; the exited server is not stopped again"
        );

        let mut g = fake();
        g.plays = [true].into();
        g.polls = [GamePoll::RoutingReady].into();
        g.game_exits_after_ready = Some(Some(0));
        g.fail = Some(("retire_game", "lease not released"));
        let outcome = run(&mut g, Action::Serve);
        assert_eq!(outcome.state, State::Failed);
        assert_eq!(outcome.message, "lease not released");
        assert!(g.names().contains(&"stop_server") && g.names().contains(&"finish_guard"));
    }

    #[test]
    fn a_stop_before_preparation_cancels_without_preparing() {
        let mut f = fake();
        f.stop_at_call = Some(1);
        let outcome = run(&mut f, Action::Start);
        assert_eq!(outcome.state, State::Failed);
        assert!(outcome.message.contains("cancelled before preparation"));
        assert!(!f.names().contains(&"prepare"));
    }
}
