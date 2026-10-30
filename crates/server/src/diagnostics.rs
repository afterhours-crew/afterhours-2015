// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Connection-owned diagnostics. Keys must contain bounded route metadata,
//! never request bodies, account identifiers or arbitrary user text.
use std::{collections::BTreeMap, fmt::Debug, time::Instant};

const MAX_ROUTES: usize = 64;
const WINDOW_MS: u64 = 10_000;
const WARNINGS_PER_WINDOW: u32 = 16;
const REPEAT_MS: u64 = 30_000;

#[derive(Default, Debug)]
struct Count {
    seen: u64,
    pending: u64,
    last_warning: Option<u64>,
}

#[derive(Debug)]
struct Tracker<K> {
    routes: BTreeMap<K, Count>,
    overflow: Count,
    total: u64,
    warnings: u64,
    last_ms: u64,
    window_ms: u64,
    window_warnings: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct Notice<K> {
    route: Option<K>,
    seen: u64,
    suppressed: u64,
}

impl<K: Ord + Clone> Tracker<K> {
    fn new() -> Self {
        Self {
            routes: BTreeMap::new(),
            overflow: Count::default(),
            total: 0,
            warnings: 0,
            last_ms: 0,
            window_ms: 0,
            window_warnings: 0,
        }
    }

    fn observe(&mut self, key: K, now_ms: u64) -> Option<Notice<K>> {
        // A clock regression must not replenish the output budget.
        let now_ms = now_ms.max(self.last_ms);
        self.last_ms = now_ms;
        if now_ms.saturating_sub(self.window_ms) >= WINDOW_MS {
            self.window_ms = now_ms;
            self.window_warnings = 0;
        }
        self.total = self.total.saturating_add(1);
        let route =
            (self.routes.contains_key(&key) || self.routes.len() < MAX_ROUTES).then_some(key);
        let count = match &route {
            Some(key) => self.routes.entry(key.clone()).or_default(),
            None => &mut self.overflow,
        };
        count.seen = count.seen.saturating_add(1);
        count.pending = count.pending.saturating_add(1);
        if self.window_warnings >= WARNINGS_PER_WINDOW
            || count
                .last_warning
                .is_some_and(|last| now_ms.saturating_sub(last) < REPEAT_MS)
        {
            return None;
        }
        let notice = Notice {
            route,
            seen: count.seen,
            suppressed: count.pending.saturating_sub(1),
        };
        count.pending = 0;
        count.last_warning = Some(now_ms);
        self.window_warnings += 1;
        self.warnings = self.warnings.saturating_add(1);
        Some(notice)
    }
}

/// Drop runs on orderly disconnect, early return and async task cancellation.
/// Process termination can still prevent any final log from being written.
#[derive(Debug)]
pub(crate) struct Unsupported<K: Ord + Clone + Debug> {
    service: &'static str,
    connection: u32,
    started: Instant,
    tracker: Tracker<K>,
}

impl<K: Ord + Clone + Debug> Unsupported<K> {
    pub(crate) fn new(service: &'static str, connection: u32) -> Self {
        Self {
            service,
            connection,
            started: Instant::now(),
            tracker: Tracker::new(),
        }
    }

    pub(crate) fn connection(&self) -> u32 {
        self.connection
    }

    pub(crate) fn total(&self) -> u64 {
        self.tracker.total
    }

    pub(crate) fn record(&mut self, route: K) {
        self.record_correlated(route, None);
    }

    pub(crate) fn record_correlated(&mut self, route: K, correlation: Option<u64>) {
        let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(notice) = self.tracker.observe(route, now) {
            tracing::warn!(
                service = self.service,
                connection = self.connection,
                ?correlation,
                route = ?notice.route,
                route_overflow = notice.route.is_none(),
                seen = notice.seen,
                suppressed = notice.suppressed,
                "unsupported operation received"
            );
        }
    }
}

impl<K: Ord + Clone + Debug> Drop for Unsupported<K> {
    fn drop(&mut self) {
        if self.tracker.total == 0 {
            return;
        }
        let unreported = self
            .tracker
            .routes
            .values()
            .filter(|count| count.last_warning.is_none())
            .count();
        tracing::info!(
            service = self.service,
            connection = self.connection,
            unsupported_total = self.tracker.total,
            warnings = self.tracker.warnings,
            suppressed_total = self.tracker.total.saturating_sub(self.tracker.warnings),
            tracked_routes = self.tracker.routes.len(),
            unreported_routes = unreported,
            overflow_total = self.tracker.overflow.seen,
            "unsupported operation summary"
        );
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    pub(crate) fn capture(f: impl FnOnce()) -> String {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_a_suspended_connection_future_reports_its_summary() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let output = testing::capture(|| {
            let mut task = Box::pin(async {
                let mut diagnostics = Unsupported::new("world", 99);
                diagnostics.record(3);
                std::future::pending::<()>().await;
            });
            assert_eq!(
                task.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                Poll::Pending
            );
            drop(task);
        });
        assert!(output.contains("connection=99"), "{output}");
        assert!(output.contains("unsupported operation summary"), "{output}");
    }

    #[test]
    fn first_repeat_and_distinct_routes_report_counts_with_injected_time() {
        let mut tracker = Tracker::new();
        assert_eq!(
            tracker.observe(7, 0),
            Some(Notice {
                route: Some(7),
                seen: 1,
                suppressed: 0
            })
        );
        for now in 1..100 {
            assert_eq!(tracker.observe(7, now), None);
        }
        assert_eq!(tracker.observe(8, 100).unwrap().seen, 1);
        assert_eq!(
            tracker.observe(7, REPEAT_MS),
            Some(Notice {
                route: Some(7),
                seen: 101,
                suppressed: 99
            })
        );
    }

    #[test]
    fn bursts_and_route_churn_have_fixed_bounds_and_preserve_totals() {
        let mut tracker = Tracker::new();
        let emitted = (0..10_000)
            .filter(|&key| tracker.observe(key, 0).is_some())
            .count();
        assert_eq!(emitted, WARNINGS_PER_WINDOW as usize);
        assert_eq!(tracker.routes.len(), MAX_ROUTES);
        assert_eq!(tracker.total, 10_000);
        assert_eq!(tracker.overflow.seen, 10_000 - MAX_ROUTES as u64);
        let overflow = tracker.observe(10_001, WINDOW_MS).unwrap();
        assert_eq!(overflow.route, None);
        assert_eq!(overflow.suppressed, 10_000 - MAX_ROUTES as u64);
        assert_eq!(tracker.routes.len(), MAX_ROUTES);
        // A previously unreported retained route can report on its next input.
        assert_eq!(tracker.observe(20, WINDOW_MS).unwrap().suppressed, 1);
    }

    #[test]
    fn connection_isolation_clock_regression_and_counter_saturation() {
        let mut a = Tracker::new();
        let mut b = Tracker::new();
        assert!(a.observe(1, WINDOW_MS).is_some());
        assert!(a.observe(1, 0).is_none());
        assert_eq!(b.observe(1, 0).unwrap().seen, 1);
        a.total = u64::MAX;
        a.routes.get_mut(&1).unwrap().seen = u64::MAX;
        a.routes.get_mut(&1).unwrap().pending = u64::MAX;
        let notice = a.observe(1, WINDOW_MS + REPEAT_MS).unwrap();
        assert_eq!(
            (a.total, notice.seen, notice.suppressed),
            (u64::MAX, u64::MAX, u64::MAX - 1)
        );
    }

    #[test]
    fn warnings_are_visible_and_drop_reports_suppression_even_on_early_return() {
        let output = testing::capture(|| {
            let mut diagnostics = Unsupported::new("world", 42);
            diagnostics.record(7);
            diagnostics.record(7);
        });
        assert_eq!(output.matches("WARN").count(), 1, "{output}");
        for expected in [
            "connection=42",
            "route=Some(7)",
            "unsupported_total=2",
            "suppressed_total=1",
        ] {
            assert!(output.contains(expected), "{output}");
        }
        let quiet = testing::capture(|| {
            let _unused = Unsupported::<u8>::new("world", 43);
        });
        assert!(quiet.is_empty(), "{quiet}");
    }
}
