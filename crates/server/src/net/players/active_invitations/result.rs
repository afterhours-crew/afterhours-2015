// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_content::messages::result::Invitation;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Choice {
    started_ms: u64,
    answer: Option<bool>,
    nonrecommended: bool,
    pub(super) hash: Option<u32>,
    pub(super) queued: bool,
}
impl Choice {
    pub(super) fn new(started_ms: u64) -> Self {
        Self {
            started_ms,
            answer: None,
            nonrecommended: false,
            hash: None,
            queued: false,
        }
    }
    pub(super) fn select(
        &mut self,
        plan: &Invitation,
        world_ms: u64,
        answer: Option<bool>,
        won: Option<bool>,
    ) -> Result<(), replication::Error> {
        let elapsed = world_ms
            .checked_sub(self.started_ms)
            .ok_or(replication::Error::Shape)?;
        if self.hash.is_some() {
            return Ok(());
        }
        // Completion is an event, not a continuously evaluated property. A
        // subsequent cache update must not change a completed false answer.
        if self.answer.is_none() {
            self.answer = answer;
        }
        let reached =
            |d: std::time::Duration| u128::from(elapsed) >= d.as_nanos().div_ceil(1_000_000);
        if self.answer == Some(true) {
            self.hash = Some(plan.hashes()[2]);
        } else {
            self.nonrecommended |=
                self.answer == Some(false) || reached(plan.recommendation_after());
            if self.nonrecommended
                && let Some(won) = won
            {
                self.hash = Some(plan.hashes()[usize::from(!won)]);
            } else if reached(plan.default_after()) {
                self.hash = Some(plan.hashes()[0]);
            }
        }
        Ok(())
    }
}
impl State {
    pub(in crate::net::players) fn result_won(&self, objective: u64) -> Option<bool> {
        self.results.get(&objective).map(|(_, won)| *won)
    }
    fn record_result(
        &mut self,
        objective: u64,
        batch: u64,
        position: u32,
    ) -> Result<(), replication::Error> {
        let value = (batch, position == 0); // Durable result position is zero-based.
        if let Some(previous) = self.results.get(&objective) {
            return if *previous == value {
                Ok(())
            } else {
                Err(replication::Error::Shape)
            };
        }
        if self.results.len() >= 16 {
            return Err(replication::Error::Bound);
        }
        self.results.insert(objective, value);
        Ok(())
    }
}
impl PlayerListener {
    /// Called only while adopting an account worker's successful result credit.
    /// Prepare a clone before publication checks; commit it alongside the new
    /// account generation. Retried delivery of that same credit is idempotent.
    pub(in crate::net::players) fn prepare_result_invitation(
        &self,
        request: &scoring::AwardRequest,
    ) -> Result<State, replication::Error> {
        let mut next = self.active_invitations.clone();
        let Some(story) = &request.story else {
            return Ok(next);
        };
        let Some(content) = &story.progression.messages else {
            return Ok(next);
        };
        if !content
            .result
            .iter()
            .any(|p| p.result_objective() == story.objective)
        {
            return Ok(next);
        }
        if !self
            .players
            .owns_participant(HOST_SELECTOR as u8, self.persona, request.participant)
        {
            return Err(replication::Error::UnknownObject);
        }
        let event = request.event.as_ref().ok_or(replication::Error::Shape)?;
        next.record_result(story.objective, request.batch_id, event.position)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn plan() -> Invitation {
        Invitation::new(
            1,
            2,
            3,
            [10, 20, 30],
            123,
            Duration::from_secs(1),
            Duration::from_secs(5),
        )
        .unwrap()
    }
    #[test]
    fn recommendation_completion_and_result_latches_obey_order_and_authored_delays() {
        let p = plan();
        for (answer, won, want) in [
            (Some(true), Some(false), 30),
            (Some(false), Some(true), 10),
            (Some(false), Some(false), 20),
        ] {
            let mut c = Choice::new(100);
            c.select(&p, 100, answer, won).unwrap();
            assert_eq!(c.hash, Some(want));
            c.select(&p, 9000, Some(true), Some(true)).unwrap();
            assert_eq!(c.hash, Some(want));
        }
        let mut c = Choice::new(100);
        c.select(&p, 1099, None, Some(false)).unwrap();
        assert_eq!(c.hash, None);
        c.select(&p, 1100, None, Some(false)).unwrap();
        assert_eq!(c.hash, Some(20));
        let mut c = Choice::new(100);
        c.select(&p, 1100, None, None).unwrap();
        assert_eq!(c.hash, None);
        c.select(&p, 1200, Some(true), None).unwrap();
        assert_eq!(c.hash, Some(30));
        // A completed negative lookup cannot become positive when the cache
        // changes, while a result arriving after that lookup still counts.
        let mut c = Choice::new(100);
        c.select(&p, 100, Some(false), None).unwrap();
        c.select(&p, 200, Some(true), Some(false)).unwrap();
        assert_eq!(c.hash, Some(20));
        let mut c = Choice::new(100);
        c.select(&p, 5099, Some(false), None).unwrap();
        assert_eq!(c.hash, None);
        c.select(&p, 5100, Some(false), None).unwrap();
        assert_eq!(c.hash, Some(10));
        let before = c.clone();
        assert!(c.select(&p, 99, None, None).is_err());
        assert_eq!(c, before);
    }
    #[test]
    fn result_ownership_is_session_local_bounded_and_retry_safe() {
        let mut a = State::default();
        a.record_result(2, 10, 1).unwrap();
        a.record_result(2, 10, 1).unwrap();
        assert_eq!(a.results[&2], (10, false));
        assert!(a.record_result(2, 10, 0).is_err());
        assert!(a.record_result(2, 11, 1).is_err());
        assert!(State::default().results.is_empty());
        for o in 3..=17 {
            a.record_result(o, 10, 0).unwrap();
        }
        assert!(a.record_result(18, 10, 0).is_err());
        assert_eq!(a.results.len(), 16);
    }
}
