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
                channels: self
                    .host
                    .plan()
                    .data()
                    .bindings
                    .clocks
                    .values()
                    .cloned()
                    .map(|key| (key, seconds))
                    .collect(),
            })
            .map_err(invalid)?;
        self.seconds = seconds;
        Ok(())
    }

    pub fn arrived(&self) -> bool {
        self.arrived
    }
    /// Release the owner's own route goals as its destination crossing does,
    /// for a garage return that begins first: the entry spline replaces them
    /// (the request comes from the garage box, not the checkpoint; E1248).
    pub fn release_owner_routes(&mut self, players: &mut Players) -> Result<Output, Error> {
        let mut staged = players.clone();
        let mut routes = self.routes.clone();
        let mut out = Output::default();
        for ((_, p), (_, goal)) in &mut routes {
            if *p != self.owner.participant {
                continue;
            }
            let Some(id) = goal.take() else { continue };
            let released = staged.release_player_route(
                self.owner.connection,
                self.owner.persona,
                self.owner.participant,
                id,
                None,
            )?;
            out.records.extend(released.records);
            out.deleted.extend(released.deleted);
        }
        self.routes = routes;
        *players = staged;
        Ok(out)
    }

    pub fn crossing(
        &mut self,
        content: &Mode,
        players: &mut Players,
        crossing: Crossing,
        seconds: f64,
    ) -> Result<Output, Error> {
        let mut out = Output::default();
        if self.left || !crossing.inside {
            return Ok(out);
        }
        let participant = if players.owns_participant_vehicle(
            self.owner.connection,
            self.owner.persona,
            self.owner.participant,
            crossing.vehicle,
        ) {
            self.owner.participant
        } else if let Some(member) = self
            .opponents
            .iter()
            .flat_map(|r| &r.members)
            .find(|m| m.ids.vehicle == crossing.vehicle)
        {
            self.member(players, member.ids.participant.into())?
        } else {
            return Ok(out);
        };
        let Some((state, race)) = &self.progress else {
            return Ok(out);
        };
        let next_checkpoint = race
            .participant(participant)
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
            participant,
            next_checkpoint,
            crossing.seconds,
        )?
        else {
            return Ok(out);
        };
        for ((_, p), (_, goal)) in &mut next.routes {
            if *p != participant {
                continue;
            }
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
        if let Some((_, _, command)) = &mut next.pacing {
            let changes = command.crossed(&mut staged, participant, progress)?;
            out.records.extend(changes.records);
            out.deleted.extend(changes.deleted);
        }
        let (target, selector) = field(staged.objects(), next.ghost, state.into())?;
        out.calls.push(HostRpc::GameMode(event_logic::call(
            staged.objects(),
            target,
            selector,
            2,
            Some(participant),
            Arguments::Progress {
                passed: progress.passed as u32,
                next: progress.next as u32,
                seconds: progress.seconds.to_bits(),
                lap: u32::from(progress.lap),
            },
        )?));
        let effects = next.dispatch(vec![Command::Timing(
            event_logic::lifecycle::timed::Command::Waypoint {
                source: state,
                participant: participant.into(),
                lap: progress.lap,
                seconds: progress.seconds,
            },
        )])?;
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
        next.launch_overdue(seconds, &mut out);
        next.timed_due(seconds, &mut out);
        if let Some(callbacks) = &mut next.callbacks {
            let actions = callbacks.tick(seconds).map_err(invalid)?;
            let effects = next.dispatch(actions)?;
            next.effects(content, &mut staged, &effects, false, &mut out)?;
        }
        let elapsed = next
            .opponent_deadlines
            .iter()
            .filter_map(|(&key, &deadline)| (seconds >= deadline).then_some(key))
            .collect::<Vec<_>>();
        for (state, p) in elapsed {
            next.opponent_deadlines.remove(&(state, p));
            if next
                .host
                .machine()
                .members(state)
                .map_err(invalid)?
                .contains(&u64::from(p))
            {
                let effects = next.dispatch(vec![Command::State(Action::Complete {
                    state,
                    participant: p.into(),
                })])?;
                next.effects(content, &mut staged, &effects, false, &mut out)?;
            }
        }
        let elapsed = next
            .sequence_deadlines
            .iter()
            .filter_map(|(&state, &deadline)| (seconds >= deadline).then_some(state))
            .collect::<Vec<_>>();
        for state in elapsed {
            let instances = next.sequences.get_mut(&state).ok_or(Error::UnknownObject)?;
            if let Some(stop) = instances.stop(
                content.sequences.get(&state).ok_or(Error::Unsupported)?,
                &staged,
                next.owner,
            )? {
                out.calls.push(HostRpc::SequenceStop(stop));
            }
            next.sequence_deadlines.remove(&state);
        }
        for state in &content.data.game_mode.lifecycle.states {
            if state.kind != StateKind::Countdown {
                continue;
            }
            let effects = next.dispatch(vec![Command::Timing(
                event_logic::lifecycle::timed::Command::Countdown(state.serializer as u16),
            )])?;
            next.effects(content, &mut staged, &effects, false, &mut out)?;
        }
        for transition in &content.data.game_mode.lifecycle.transitions {
            if !matches!(
                transition.kind,
                TransitionKind::FinishLine | TransitionKind::Dnf
            ) {
                continue;
            }
            let effects = next.dispatch(vec![Command::Timing(
                event_logic::lifecycle::timed::Command::Transition(transition.serializer),
            )])?;
            next.effects(content, &mut staged, &effects, false, &mut out)?;
        }
        *self = next;
        *players = staged;
        Ok(out)
    }
}
