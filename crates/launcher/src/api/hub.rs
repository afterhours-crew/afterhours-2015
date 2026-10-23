// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The session's events, kept for the API's event streams.
//!
//! The supervisor publishes every event it writes (status changes,
//! diagnostics, map frames) to one [`Hub`]. Each stream reader keeps its own
//! cursor: it starts with the newest status and map frame, then reads every
//! later event in order. The hub keeps a bounded tail of recent events; a
//! reader that falls further behind continues from the oldest event still
//! kept, so a slow reader loses events but never holds up the session.

use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::Duration,
};

/// Most events kept for readers that fall behind.
pub const KEPT_EVENTS: usize = 512;
/// Most bytes of event text kept for readers that fall behind.
pub const KEPT_BYTES: usize = 4 * 1024 * 1024;

/// One event as published: its place in the session's order, its kind
/// (the record's `event` property) and the record's JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub sequence: u64,
    pub kind: &'static str,
    pub json: Arc<str>,
}

#[derive(Debug, Default)]
struct State {
    next: u64,
    recent: VecDeque<Published>,
    bytes: usize,
    status: Option<Published>,
    map_feed: Option<Published>,
    closed: bool,
}

/// What a reader gets from [`Hub::wait`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Events in order, and the cursor to continue from.
    Events(Vec<Published>, u64),
    /// Nothing new within the wait.
    Idle,
    /// The session is over and every kept event was read.
    Closed,
}

/// The session's events for its stream readers.
#[derive(Debug, Default)]
pub struct Hub {
    state: Mutex<State>,
    changed: Condvar,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Add an event. Nothing is added once the hub is closed.
    pub fn publish(&self, kind: &'static str, json: String) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        let event = Published {
            sequence: state.next,
            kind,
            json: json.into(),
        };
        state.next += 1;
        match kind {
            "status" => state.status = Some(event.clone()),
            "map-feed" => state.map_feed = Some(event.clone()),
            _ => {}
        }
        state.bytes += event.json.len();
        state.recent.push_back(event);
        while state.recent.len() > KEPT_EVENTS || state.bytes > KEPT_BYTES {
            match state.recent.pop_front() {
                Some(old) => state.bytes -= old.json.len(),
                None => break,
            }
        }
        drop(state);
        self.changed.notify_all();
    }

    /// The session is over: readers get what is left, then the end.
    pub fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }

    /// Start reading: the newest status and map frame, in order, and the
    /// cursor of the next event.
    pub fn subscribe(&self) -> (Vec<Published>, u64) {
        let state = self.lock();
        let mut snapshot: Vec<Published> = [state.status.clone(), state.map_feed.clone()]
            .into_iter()
            .flatten()
            .collect();
        snapshot.sort_by_key(|e| e.sequence);
        (snapshot, state.next)
    }

    /// Every kept event from `cursor` on, waiting up to `timeout` for one.
    pub fn wait(&self, cursor: u64, timeout: Duration) -> Next {
        let mut state = self.lock();
        let mut waited = false;
        loop {
            if state.next > cursor {
                let events: Vec<Published> = state
                    .recent
                    .iter()
                    .filter(|e| e.sequence >= cursor)
                    .cloned()
                    .collect();
                return Next::Events(events, state.next);
            }
            if state.closed {
                return Next::Closed;
            }
            if waited {
                return Next::Idle;
            }
            let (next, result) = self
                .changed
                .wait_timeout(state, timeout)
                .unwrap_or_else(|p| p.into_inner());
            state = next;
            waited = result.timed_out();
        }
    }
}
