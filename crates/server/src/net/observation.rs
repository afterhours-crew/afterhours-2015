// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only world-loop evidence for offline validation. These are server
//! observations, not proof that a client displayed or accepted an output.

use nfs_world::participants::{Stage, intro::Chapter};

/// Cumulative failures, including recoveries that otherwise hide a bad run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorldHealth {
    pub refused_frames: usize,
    /// Application, transport and file error events, including failures before
    /// listener delivery or while preparing a response. Deferred work is excluded.
    pub session_application_errors: usize,
    pub session_link_errors: usize,
    pub session_file_errors: usize,
    pub decode_errors: usize,
    pub unsupported_operations: u64,
    /// Human map sync, rival source collection, rival map sync.
    pub optional_map_failures: [usize; 3],
    /// Authored event eligibility projection/publication failures, including recovery.
    pub event_availability_failures: usize,
    pub driving_distance_failures: usize,
    pub waiting_fallbacks: usize,
    pub arrival_fallbacks: usize,
    pub crash_fallbacks: usize,
    pub event_intro_fallbacks: usize,
    /// Traffic, rivals, scoring, events. Faults remain visible after recovery.
    pub disabled_systems: [bool; 4],
}

impl WorldHealth {
    pub(super) fn include_session(&mut self, failures: nfs_world::session::Failures) {
        self.session_application_errors = self.session_application_errors.max(failures.application);
        self.session_link_errors = self.session_link_errors.max(failures.link);
        self.session_file_errors = self.session_file_errors.max(failures.file);
    }

    pub(super) fn include_application(&mut self, stats: nfs_world::application::Stats) {
        // Session ordering can refuse a frame before the domain listener runs.
        // Both counters include listener refusals; adding them counts twice.
        self.refused_frames = self
            .refused_frames
            .max(stats.frames_complete.saturating_sub(stats.frames_accepted));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_errors_are_unhealthy_without_any_completed_listener_frame() {
        let mut health = WorldHealth::default();
        health.include_application(nfs_world::application::Stats::default());
        health.include_session(nfs_world::session::Failures {
            application: 1,
            link: 2,
            file: 3,
        });
        assert_ne!(health, WorldHealth::default());
        assert_eq!(health.refused_frames, 0);
        assert_eq!(health.session_application_errors, 1);
        assert_eq!(health.session_link_errors, 2);
        assert_eq!(health.session_file_errors, 3);
        let recorded = health.clone();
        health.include_session(nfs_world::session::Failures::default());
        health.include_application(nfs_world::application::Stats::default());
        assert_eq!(health, recorded);
    }

    #[test]
    fn session_refusals_remain_unhealthy_after_recovery_without_double_counting() {
        let mut health = WorldHealth::default();
        let mut stats = nfs_world::application::Stats {
            frames_complete: 3,
            frames_accepted: 2,
            ..Default::default()
        };
        health.include_application(stats);
        assert_ne!(health, WorldHealth::default());
        stats.frames_complete += 1;
        stats.frames_accepted += 1;
        health.include_application(stats);
        assert_eq!(health.refused_frames, 1);
        health.refused_frames = 2;
        stats.frames_complete += 1;
        health.include_application(stats);
        assert_eq!(health.refused_frames, 2);
    }
    #[test]
    fn empty_listener_queues_do_not_hide_session_notifications_or_file_receipts() {
        let mut value = crate::net::players::PlayerListener::new(7).observation(0, 0, 0, false);
        assert!(value.queues_drained());
        value.rpc_pending = 283;
        assert!(!value.queues_drained());
        value.rpc_pending = 0;
        value.files_pending = 1;
        assert!(!value.queues_drained());
        value.files_pending = 0;
        value.items_working = true;
        assert!(!value.queues_drained());
        value.items_working = false;
        assert!(value.queues_drained());
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParticipantObservation {
    pub participant: u16,
    pub stage: Option<Stage>,
    /// This stays separate from `stage`: the intro drives also use FreeRoam.
    pub chapter: Option<Chapter>,
}

/// A bounded snapshot. Participant ids are session-local; no account data or
/// message bodies are exposed. Queue sizes are pending work, not delivery proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldObservation {
    pub world_ms: u64,
    pub health: WorldHealth,
    pub participants: Vec<ParticipantObservation>,
    /// Replication sections, combined entries, RPCs, content, Speedwalls,
    /// pending story writes, pending teleports.
    pub owned_queues: [usize; 7],
    pub replication_pending: usize,
    pub content_pending: usize,
    /// Notifications already transferred from the listener into the session.
    pub rpc_pending: usize,
    /// Session file transfers, including those awaiting their final receipt.
    pub files_pending: usize,
    pub items_working: bool,
}

impl WorldObservation {
    /// All observed producer/session work is drained. This is not gameplay or
    /// rendering acceptance, nor a receipt for every ordinary application frame.
    pub fn queues_drained(&self) -> bool {
        self.owned_queues == [0; 7]
            && self.replication_pending == 0
            && self.content_pending == 0
            && self.rpc_pending == 0
            && self.files_pending == 0
            && !self.items_working
    }
}
