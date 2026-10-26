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
    pub decode_errors: usize,
    pub unsupported_operations: u64,
    /// Human map sync, rival source collection, rival map sync.
    pub optional_map_failures: [usize; 3],
    pub waiting_fallbacks: usize,
    pub arrival_fallbacks: usize,
    pub crash_fallbacks: usize,
    pub event_intro_fallbacks: usize,
    /// Traffic, rivals, scoring, events. Faults remain visible after recovery.
    pub disabled_systems: [bool; 4],
}

impl WorldHealth {
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
    pub items_working: bool,
}
