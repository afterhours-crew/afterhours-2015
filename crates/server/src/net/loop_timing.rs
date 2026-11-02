// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded wall-clock timing of one world loop iteration, split by stage.
//!
//! The loop must finish each iteration inside [`super::WORLD_TICK`] or the
//! outgoing replication queue falls behind the client. A single aggregate
//! delay warning cannot say which stage grew, so every stage records its own
//! duration. The accumulator is fixed-size and allocation-free; it reports an
//! over-budget iteration immediately and a periodic summary on the world
//! clock so a launcher log shows the trend without a debugger.

use std::time::{Duration, Instant};

/// World-clock interval between two summaries.
pub(super) const SUMMARY_MS: u64 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stage {
    Sequences,
    LevelPoll,
    Settled,
    Traffic,
    Rivals,
    Scoring,
    MapFeed,
    Events,
    Movement,
    Receive,
    Queue,
    Send,
}

const STAGES: [Stage; 12] = [
    Stage::Sequences,
    Stage::LevelPoll,
    Stage::Settled,
    Stage::Traffic,
    Stage::Rivals,
    Stage::Scoring,
    Stage::MapFeed,
    Stage::Events,
    Stage::Movement,
    Stage::Receive,
    Stage::Queue,
    Stage::Send,
];

impl Stage {
    pub(super) fn name(self) -> &'static str {
        match self {
            Stage::Sequences => "sequences",
            Stage::LevelPoll => "level_poll",
            Stage::Settled => "settled",
            Stage::Traffic => "traffic",
            Stage::Rivals => "rivals",
            Stage::Scoring => "scoring",
            Stage::MapFeed => "map_feed",
            Stage::Events => "events",
            Stage::Movement => "movement",
            Stage::Receive => "receive",
            Stage::Queue => "queue",
            Stage::Send => "send",
        }
    }
    fn index(self) -> usize {
        STAGES.iter().position(|s| *s == self).unwrap_or(0)
    }
    /// Stages that run on this thread while the client waits. The receive
    /// stage also contains the idle wait for the next datagram or tick, so it
    /// is excluded from the budget check.
    fn is_work(self) -> bool {
        self != Stage::Receive
    }
}

/// Result of one finished iteration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Finish {
    /// Work on this thread in this iteration, excluding the receive wait.
    pub(super) work: Duration,
    /// The work exceeded the budget.
    pub(super) slow: bool,
    /// A summary became due on the world clock.
    pub(super) summary: Option<Summary>,
}

/// Aggregate over the iterations since the previous summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Summary {
    pub(super) iterations: u64,
    pub(super) slow: u64,
    pub(super) mean_work: Duration,
    pub(super) max_work: Duration,
    /// Per stage: name, mean, max.
    pub(super) stages: Vec<(&'static str, Duration, Duration)>,
}

pub(super) struct Timing {
    budget: Duration,
    mark: Instant,
    current: [Duration; STAGES.len()],
    sum: [Duration; STAGES.len()],
    max: [Duration; STAGES.len()],
    iterations: u64,
    slow: u64,
    sum_work: Duration,
    max_work: Duration,
    summary_at: Option<u64>,
}

impl Timing {
    pub(super) fn new(budget: Duration, now: Instant) -> Self {
        Self {
            budget,
            mark: now,
            current: [Duration::ZERO; STAGES.len()],
            sum: [Duration::ZERO; STAGES.len()],
            max: [Duration::ZERO; STAGES.len()],
            iterations: 0,
            slow: 0,
            sum_work: Duration::ZERO,
            max_work: Duration::ZERO,
            summary_at: None,
        }
    }

    /// Start an iteration at `now`.
    pub(super) fn begin(&mut self, now: Instant) {
        self.mark = now;
        self.current = [Duration::ZERO; STAGES.len()];
    }

    /// Attribute the time since the previous mark to `stage`.
    pub(super) fn record(&mut self, stage: Stage, now: Instant) {
        let elapsed = now.saturating_duration_since(self.mark);
        self.mark = now;
        let i = stage.index();
        self.current[i] = self.current[i].saturating_add(elapsed);
    }

    /// The stages of the current iteration that took any time, in
    /// milliseconds, for an over-budget warning.
    pub(super) fn current_ms(&self) -> Vec<(&'static str, u64)> {
        STAGES
            .iter()
            .zip(self.current.iter())
            .filter(|(_, d)| !d.is_zero())
            .map(|(s, d)| (s.name(), d.as_millis() as u64))
            .collect()
    }

    /// Close the iteration at world time `world_ms`.
    pub(super) fn finish(&mut self, world_ms: u64) -> Finish {
        let mut work = Duration::ZERO;
        for (stage, d) in STAGES.iter().zip(self.current.iter()) {
            let i = stage.index();
            self.sum[i] = self.sum[i].saturating_add(*d);
            self.max[i] = self.max[i].max(*d);
            if stage.is_work() {
                work = work.saturating_add(*d);
            }
        }
        self.iterations += 1;
        self.sum_work = self.sum_work.saturating_add(work);
        self.max_work = self.max_work.max(work);
        let slow = work > self.budget;
        if slow {
            self.slow += 1;
        }
        let due = match self.summary_at {
            None => {
                self.summary_at = Some(world_ms);
                false
            }
            Some(at) => world_ms.saturating_sub(at) >= SUMMARY_MS,
        };
        let summary = due.then(|| self.take_summary(world_ms));
        Finish {
            work,
            slow,
            summary,
        }
    }

    fn take_summary(&mut self, world_ms: u64) -> Summary {
        let n = self.iterations.max(1) as u32;
        let summary = Summary {
            iterations: self.iterations,
            slow: self.slow,
            mean_work: self.sum_work / n,
            max_work: self.max_work,
            stages: STAGES
                .iter()
                .zip(self.sum.iter().zip(self.max.iter()))
                .filter(|(_, (sum, _))| !sum.is_zero())
                .map(|(s, (sum, max))| (s.name(), *sum / n, *max))
                .collect(),
        };
        self.sum = [Duration::ZERO; STAGES.len()];
        self.max = [Duration::ZERO; STAGES.len()];
        self.iterations = 0;
        self.slow = 0;
        self.sum_work = Duration::ZERO;
        self.max_work = Duration::ZERO;
        self.summary_at = Some(world_ms);
        summary
    }
}

impl Summary {
    /// Stage rows as (name, mean ms, max ms) for structured logging.
    pub(super) fn stage_ms(&self) -> Vec<(&'static str, u64, u64)> {
        self.stages
            .iter()
            .map(|(name, mean, max)| (*name, mean.as_millis() as u64, max.as_millis() as u64))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn stages_accumulate_and_only_work_counts_against_the_budget() {
        let start = Instant::now();
        let mut timing = Timing::new(ms(50), start);
        timing.begin(start);
        timing.record(Stage::Scoring, start + ms(30));
        // A long receive wait is idle time, not work.
        timing.record(Stage::Receive, start + ms(330));
        timing.record(Stage::Send, start + ms(340));
        let finish = timing.finish(0);
        assert_eq!(finish.work, ms(40));
        assert!(!finish.slow);
        assert!(finish.summary.is_none());
        assert_eq!(
            timing.current_ms(),
            vec![("scoring", 30), ("receive", 300), ("send", 10)]
        );

        timing.begin(start + ms(340));
        timing.record(Stage::Scoring, start + ms(400));
        let finish = timing.finish(100);
        assert_eq!(finish.work, ms(60));
        assert!(finish.slow);
    }

    #[test]
    fn summary_is_due_on_the_world_clock_and_resets() {
        let start = Instant::now();
        let mut timing = Timing::new(ms(50), start);
        let mut at = start;
        for i in 0..3 {
            timing.begin(at);
            at += ms(10 + i * 10);
            timing.record(Stage::Traffic, at);
            at += ms(5);
            timing.record(Stage::Queue, at);
            assert!(timing.finish(i * 1_000).summary.is_none());
        }
        timing.begin(at);
        at += ms(70);
        timing.record(Stage::Traffic, at);
        let finish = timing.finish(SUMMARY_MS);
        assert!(finish.slow);
        let summary = finish.summary.expect("summary due");
        assert_eq!(summary.iterations, 4);
        assert_eq!(summary.slow, 1);
        // (15 + 25 + 35 + 70) / 4
        assert_eq!(summary.mean_work, Duration::from_micros(36_250));
        assert_eq!(summary.max_work, ms(70));
        assert_eq!(
            summary.stage_ms(),
            vec![("traffic", 32, 70), ("queue", 3, 5)]
        );
        // The next window starts empty and is not due again immediately.
        timing.begin(at);
        timing.record(Stage::Send, at + ms(1));
        let finish = timing.finish(SUMMARY_MS + 1);
        assert!(finish.summary.is_none());
        let finish_later = {
            timing.begin(at);
            timing.record(Stage::Send, at + ms(1));
            timing.finish(2 * SUMMARY_MS)
        };
        let summary = finish_later.summary.expect("second summary");
        assert_eq!(summary.iterations, 2);
        assert_eq!(summary.slow, 0);
        assert_eq!(summary.stage_ms(), vec![("send", 1, 1)]);
    }

    /// Diagnostic, not a gate: how long a 50 ms sleep and a 50 ms tokio
    /// interval actually take in this process. Run with `--ignored
    /// --nocapture`. A background process can be held to the coarse system
    /// timer, which would stretch the world tick.
    #[test]
    #[ignore]
    fn measure_timer_cadence() {
        let mut periods = Vec::new();
        for _ in 0..40 {
            let at = Instant::now();
            std::thread::sleep(ms(50));
            periods.push(at.elapsed().as_secs_f64() * 1000.);
        }
        let mean = periods.iter().sum::<f64>() / periods.len() as f64;
        println!(
            "std sleep 50 ms: mean {mean:.2} ms, max {:.2}",
            periods.iter().cloned().fold(0., f64::max)
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut tick = tokio::time::interval(ms(50));
            tick.tick().await;
            let mut periods = Vec::new();
            for _ in 0..40 {
                let at = Instant::now();
                tick.tick().await;
                periods.push(at.elapsed().as_secs_f64() * 1000.);
            }
            let mean = periods.iter().sum::<f64>() / periods.len() as f64;
            println!(
                "tokio interval 50 ms: mean {mean:.2} ms, max {:.2}",
                periods.iter().cloned().fold(0., f64::max)
            );
            let mut periods = Vec::new();
            for _ in 0..40 {
                let at = Instant::now();
                tokio::time::sleep(ms(1)).await;
                periods.push(at.elapsed().as_secs_f64() * 1000.);
            }
            let mean = periods.iter().sum::<f64>() / periods.len() as f64;
            println!("tokio sleep 1 ms: mean {mean:.2} ms");
        });
    }

    #[test]
    fn stage_order_is_stable_and_every_stage_has_a_name() {
        for (i, stage) in STAGES.iter().enumerate() {
            assert_eq!(stage.index(), i);
            assert!(!stage.name().is_empty());
        }
        assert!(!Stage::Receive.is_work());
        assert!(Stage::Scoring.is_work());
    }
}
