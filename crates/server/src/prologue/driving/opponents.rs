// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Story-owned opponents share the mode's membership and commands. The client
//! acknowledges human sequences; AI-only sequence timers remain host-owned.
use super::*;
use nfs_content::events::{Sequence, lifecycle::StateConfiguration};
use nfs_world::replication::{Initial, Update, players::ai::GoalChanges};
#[cfg(test)]
pub(crate) mod tests;

impl Mode {
    pub fn with_opponents(
        mut self,
        roster: nfs_content::events::ai::Roster,
        vehicles: crate::vehicle_content::Content,
        items: &nfs_services::item_content::ItemContent,
    ) -> Result<Self, Error> {
        self.wait_state()?;
        if self.data.game_mode.pacing.is_none() {
            return Err(Error::Unsupported);
        }
        self.opponents = Some(Arc::new(crate::event_ai::Content::for_story(
            roster, vehicles, items,
        )?));
        Ok(self)
    }
    pub fn opponents(&self) -> Option<&crate::event_ai::Content> {
        self.opponents.as_deref()
    }
    fn wait_state(&self) -> Result<(u16, Property), Error> {
        let states = self
            .data
            .game_mode
            .lifecycle
            .states
            .iter()
            .filter_map(|s| match &s.configuration {
                StateConfiguration::WaitForAi { manager } => {
                    Some((s.serializer as u16, manager.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        match states.as_slice() {
            [state] => Ok(state.clone()),
            _ => Err(Error::Shape),
        }
    }
}
impl Active {
    pub(crate) fn owns_vehicle(&self, players: &Players, vehicle: u16) -> bool {
        players.owns_participant_vehicle(
            self.owner.connection,
            self.owner.persona,
            self.owner.participant,
            vehicle,
        ) || self.delegated(players).contains_key(&vehicle)
    }
    pub(crate) fn delegated(&self, players: &Players) -> BTreeMap<u16, u64> {
        self.opponents
            .iter()
            .flat_map(|r| &r.members)
            .filter_map(|m| {
                (players.ai_ids(
                    self.owner.connection,
                    self.owner.persona,
                    m.selection.car_key,
                ) == Some(m.ids))
                .then_some((m.ids.vehicle, m.selection.car_key))
            })
            .collect()
    }
    pub(crate) fn locations(
        &self,
        players: &Players,
    ) -> Result<Vec<crate::rival_content::population::Location>, Error> {
        self.opponents
            .iter()
            .flat_map(|r| &r.members)
            .map(|m| {
                self.member(players, m.ids.participant.into())?;
                Ok((
                    m.ids.participant,
                    players
                        .ai_vehicle_physics(
                            self.owner.connection,
                            self.owner.persona,
                            m.selection.car_key,
                        )?
                        .decode([0.; 3])?
                        .position,
                ))
            })
            .collect()
    }
    pub(super) fn members(&self) -> Vec<u16> {
        std::iter::once(self.owner.participant)
            .chain(
                self.opponents
                    .iter()
                    .flat_map(|r| r.members.iter().map(|m| m.ids.participant)),
            )
            .collect()
    }
    pub fn opponents(&self) -> Option<&crate::event_ai::Racers> {
        self.opponents.as_ref()
    }
    pub(super) fn member(&self, players: &Players, participant: u64) -> Result<u16, Error> {
        let p = u16::try_from(participant).map_err(invalid)?;
        let valid = if p == self.owner.participant {
            players.owns_participant(self.owner.connection, self.owner.persona, p)
        } else {
            self.opponents.iter().flat_map(|r| &r.members).any(|m| {
                m.ids.participant == p
                    && players.ai_ids(
                        self.owner.connection,
                        self.owner.persona,
                        m.selection.car_key,
                    ) == Some(m.ids)
            })
        };
        if valid {
            Ok(p)
        } else {
            Err(Error::UnknownObject)
        }
    }
    fn opponent(&self, participant: u16) -> Result<crate::event_ai::Member, Error> {
        self.opponents
            .iter()
            .flat_map(|r| &r.members)
            .find(|m| m.ids.participant == participant)
            .copied()
            .ok_or(Error::UnknownObject)
    }
    pub(super) fn participant_car_id(
        &self,
        content: &Mode,
        participant: u16,
    ) -> Result<u32, Error> {
        if participant == self.owner.participant {
            return self.car_id.ok_or(Error::Shape);
        }
        content
            .opponents()
            .ok_or(Error::Unsupported)?
            .car_id(self.opponent(participant)?.selection.persona)
    }

    /// All cars are constructed before admission. Retain both the roster and
    /// native WaitForAi membership as one transaction; failure consumes neither.
    pub fn admit(
        &mut self,
        content: &Mode,
        players: &mut Players,
        racers: &mut crate::event_ai::Racers,
    ) -> Result<Output, Error> {
        if self.opponents.is_some()
            || self.car_id.is_some()
            || self.ending
            || self.left
            || racers.members.is_empty()
            || content
                .opponents()
                .is_none_or(|c| c.roster().spawns.len() != racers.members.len())
        {
            return Err(Error::Shape);
        }
        let (waiting, manager) = content.wait_state()?;
        let mut next = self.clone();
        let mut staged = players.clone();
        let mut out = Output::default();
        next.opponents = Some(racers.clone());
        let members = racers
            .members
            .iter()
            .map(|m| m.ids.participant)
            .collect::<Vec<_>>();
        if members.iter().collect::<BTreeSet<_>>().len() != members.len() {
            return Err(Error::Shape);
        }
        for &p in &members {
            next.member(&staged, p.into())?;
            next.host
                .ai_member(manager.clone(), p.into(), true)
                .map_err(invalid)?;
            out.calls.push(HostRpc::GameMode(event_logic::call(
                staged.objects(),
                next.ghost,
                0,
                2,
                Some(p),
                Arguments::None,
            )?));
            let effects = next.dispatch(vec![Command::State(Action::Enter {
                state: waiting,
                participant: p.into(),
            })])?;
            next.effects(content, &mut staged, &effects, false, &mut out)?;
        }
        let effects = next.dispatch(vec![Command::State(Action::CompleteAll {
            state: waiting,
            only_incomplete: true,
        })])?;
        next.effects(content, &mut staged, &effects, false, &mut out)?;
        *self = next;
        *players = staged;
        racers.members.clear();
        racers.authority = Default::default();
        Ok(out)
    }

    pub(super) fn opponent_sequence(
        &mut self,
        content: &Mode,
        players: &mut Players,
        p: u16,
        state: u16,
        sequence: &Sequence,
        out: &mut Output,
    ) -> Result<(), Error> {
        let member = self.opponent(p)?;
        if let Some(goto) = sequence.goto {
            let goals = event_logic::goto_goals(goto)?
                .into_iter()
                .map(|initial| {
                    let update = match &initial {
                        Initial::GotoWaypointGoal(g) if !g.progress_updates => {
                            Update::GotoWaypointGoal(
                                nfs_world::replication::goto_waypoint::Update::NoProgress,
                            )
                        }
                        Initial::SpeedLimitGoal(_) => Update::SpeedLimitGoal,
                        _ => return Err(Error::Unsupported),
                    };
                    Ok((initial, update))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let changes = players.replace_ai_goals(
                self.owner.connection,
                self.owner.persona,
                member.selection.car_key,
                goals,
                None,
            )?;
            out.records.extend(changes.records);
            out.deleted.extend(changes.deleted);
        }
        if let Some(duration) = content.timing.get(&usize::from(state))
            && self
                .opponent_deadlines
                .insert((state, p), self.seconds + duration)
                .is_some()
        {
            return Err(Error::DuplicateObject);
        }
        Ok(())
    }
    pub(super) fn start_race_commands(
        &self,
        content: &Mode,
        players: &mut Players,
    ) -> Result<(crate::event_ai::Commands, GoalChanges), Error> {
        let route = event_logic::route_goal(&content.data.game_mode)?;
        if let Some(racers) = &self.opponents {
            let ai = content.opponents().ok_or(Error::Unsupported)?;
            let skills = std::iter::once(Ok(0.5))
                .chain(racers.members.iter().map(|m| {
                    ai.roster()
                        .pacing_skills
                        .get(m.selection.persona)
                        .copied()
                        .flatten()
                        .ok_or(Error::Shape)
                }))
                .collect::<Result<Vec<_>, _>>()?;
            crate::event_ai::Commands::start(
                players,
                self.owner.connection,
                self.owner.persona,
                self.owner.participant,
                racers,
                route,
                &skills,
            )
        } else {
            crate::event_ai::Commands::start_player(
                players,
                self.owner.connection,
                self.owner.persona,
                self.owner.participant,
                route,
                0.5,
            )
        }
    }
    pub(super) fn control_updates(
        &self,
        players: &mut Players,
        out: &mut Output,
    ) -> Result<(), Error> {
        if let Some(callbacks) = &self.callbacks {
            for (participant, enabled) in callbacks.controls() {
                let p = self.member(players, participant)?;
                let actor = if p == self.owner.participant {
                    players
                        .owned_actor(self.owner.connection, self.owner.persona, p)
                        .ok_or(Error::UnknownObject)?
                } else {
                    self.opponent(p)?.ids.actor
                };
                out.records.extend(players.change(
                    actor,
                    Update::Actor(nfs_world::replication::ActorUpdate {
                        flag_a: Some(enabled),
                        ..Default::default()
                    }),
                )?);
            }
        }
        Ok(())
    }
}
