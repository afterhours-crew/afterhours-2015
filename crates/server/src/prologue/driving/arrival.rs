// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Destination reports use the shared race clock and native finish arbitration.
//! Reaching a waypoint does not itself complete the persistent story objective.
use super::*;
use event_logic::lifecycle::timed::Clocks;
use nfs_content::events::lifecycle::TransitionKind;
use nfs_world::{game_mode::Crossing, replication::Update};

impl Active {
    pub fn owner(&self) -> Owner {
        self.owner
    }
    pub fn clock(&mut self, seconds: f64) -> Result<(), Error> {
        if !seconds.is_finite() || seconds < self.seconds {
            return Err(Error::Shape);
        }
        self.host
            .clocks(Clocks {
                receipt_seconds: seconds,
                channels: BTreeMap::new(),
            })
            .map_err(invalid)?;
        self.seconds = seconds;
        Ok(())
    }

    pub fn arrived(&self) -> bool {
        self.arrived
    }

    pub fn crossing(
        &mut self,
        content: &Mode,
        players: &mut Players,
        crossing: Crossing,
        seconds: f64,
    ) -> Result<Output, Error> {
        let mut out = Output::default();
        if !crossing.inside
            || !players.owns_participant_vehicle(
                self.owner.connection,
                self.owner.persona,
                self.owner.participant,
                crossing.vehicle,
            )
        {
            return Ok(out);
        }
        let Some((state, race)) = &self.progress else {
            return Ok(out);
        };
        let next_checkpoint = race
            .participant(self.owner.participant)
            .ok_or(Error::UnknownObject)?
            .next;
        if content
            .data
            .game_mode
            .checkpoints
            .get(next_checkpoint)
            .and_then(|&(k, s)| players.objects().scene_endpoint(k, s))
            != Some((crossing.scene, crossing.selector))
        {
            return Ok(out);
        }
        let state = *state;
        let mut next = self.clone();
        let mut staged = players.clone();
        next.clock(seconds)?;
        let Some(progress) = next.progress.as_mut().ok_or(Error::Shape)?.1.cross(
            next.owner.participant,
            next_checkpoint,
            crossing.seconds,
        )?
        else {
            return Ok(out);
        };
        for (_, goal) in next.routes.values_mut() {
            let Some(id) = *goal else { continue };
            if let Some(pair) = progress.goal_progress {
                out.records.extend(staged.change(
                    id,
                    Update::GotoWaypointGoal(
                        nfs_world::replication::goto_waypoint::Update::Progress(Some(pair)),
                    ),
                )?);
            } else {
                let released = staged.release_player_route(
                    next.owner.connection,
                    next.owner.persona,
                    next.owner.participant,
                    id,
                    None,
                )?;
                out.records.extend(released.records);
                out.deleted.extend(released.deleted);
                *goal = None;
            }
        }
        let (target, selector) = field(staged.objects(), next.ghost, state.into())?;
        out.calls.push(HostRpc::GameMode(event_logic::call(
            staged.objects(),
            target,
            selector,
            2,
            Some(next.owner.participant),
            Arguments::Progress {
                passed: progress.passed as u32,
                next: progress.next as u32,
                seconds: progress.seconds.to_bits(),
                lap: u32::from(progress.lap),
            },
        )?));
        let effects = next
            .host
            .waypoint(
                state,
                next.owner.participant.into(),
                progress.lap,
                progress.seconds,
                &mut |_, e| {
                    next.callbacks
                        .as_mut()
                        .map_or_else(|| Ok(vec![]), |r| r.effect(e))
                },
            )
            .map_err(invalid)?;
        next.effects(content, &mut staged, &effects, false, &mut out)?;
        *self = next;
        *players = staged;
        Ok(out)
    }

    pub fn advance(
        &mut self,
        content: &Mode,
        players: &mut Players,
        seconds: f64,
    ) -> Result<Output, Error> {
        if self.left {
            return Ok(Output::default());
        }
        let mut next = self.clone();
        let mut staged = players.clone();
        let mut out = Output::default();
        next.clock(seconds)?;
        for transition in &content.data.game_mode.lifecycle.transitions {
            if transition.kind != TransitionKind::FinishLine {
                continue;
            }
            let effects = next
                .host
                .update_transition(transition.serializer, &mut |_, e| {
                    next.callbacks
                        .as_mut()
                        .map_or_else(|| Ok(vec![]), |r| r.effect(e))
                })
                .map_err(invalid)?;
            next.effects(content, &mut staged, &effects, false, &mut out)?;
        }
        *self = next;
        *players = staged;
        Ok(out)
    }
}
