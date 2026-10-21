// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::replication::{Initial, Update, goto_waypoint, pacing};
use std::collections::BTreeMap;

/// One event's route commands and shared pacing group. Opponent simulation stays
/// delegated after its route completes; the event supplies its next command.
#[derive(Clone, Debug)]
pub struct Commands {
    connection: u8,
    persona: u64,
    human: u16,
    pacing: Option<u16>,
    members: Vec<u16>,
    routes: BTreeMap<u16, Route>,
}

#[derive(Clone, Copy, Debug)]
struct Route {
    actor: u16,
    goal: u16,
    car: Option<u64>,
}

fn extend(out: &mut ai::GoalChanges, changes: ai::GoalChanges) {
    out.records.extend(changes.records);
    out.deleted.extend(changes.deleted);
    out.goals.extend(changes.goals);
}

impl Commands {
    /// Create all commands atomically. The human must have released intro
    /// control. Opponents replace their intro commands, preserving their cars.
    /// Pacing values are calculated by the caller, in human-then-grid order.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        players: &mut Players,
        connection: u8,
        persona: u64,
        human: u16,
        racers: &Racers,
        route: goto_waypoint::Initial,
        skills: &[f32],
    ) -> Result<(Self, ai::GoalChanges), Error> {
        if racers.members.is_empty()
            || racers.members.len() > nfs_world::race_pacing::MAX_OPPONENTS
            || skills.len() != racers.members.len() + 1
            || skills.iter().any(|v| !v.is_finite())
            || !route.progress_updates
            || route.waypoint != 0
        {
            return Err(Error::Shape);
        }
        let actor = players
            .owned_actor(connection, persona, human)
            .ok_or(Error::UnknownObject)?;
        for member in &racers.members {
            if players.ai_ids(connection, persona, member.selection.car_key) != Some(member.ids) {
                return Err(Error::UnknownObject);
            }
        }
        let members: Vec<_> = std::iter::once(human)
            .chain(racers.members.iter().map(|m| m.ids.participant))
            .collect();
        let mut world = players.clone();
        let pacing = world.create_ai_pacing_group(
            connection,
            persona,
            members
                .iter()
                .zip(skills)
                .map(|(&participant, &skill)| pacing::Entry {
                    participant,
                    value: skill.to_bits(),
                })
                .collect(),
        )?;
        let group = pacing.id;
        let mut changes = ai::GoalChanges {
            records: vec![pacing],
            ..Default::default()
        };
        let first =
            world.start_player_race_goals(connection, persona, human, route.clone(), group)?;
        let mut routes = BTreeMap::from([(
            human,
            Route {
                actor,
                goal: first.goals[0],
                car: None,
            },
        )]);
        extend(&mut changes, first);
        for member in &racers.members {
            let assigned = world.replace_ai_goals(
                connection,
                persona,
                member.selection.car_key,
                vec![(
                    Initial::GotoWaypointGoal(route.clone()),
                    Update::GotoWaypointGoal(goto_waypoint::Update::Progress(Some([0, 0]))),
                )],
                Some(group),
            )?;
            routes.insert(
                member.ids.participant,
                Route {
                    actor: member.ids.actor,
                    goal: assigned.goals[0],
                    car: Some(member.selection.car_key),
                },
            );
            extend(&mut changes, assigned);
        }
        *players = world;
        Ok((
            Self {
                connection,
                persona,
                human,
                pacing: Some(group),
                members,
                routes,
            },
            changes,
        ))
    }

    pub fn pacing(&self) -> Option<u16> {
        self.pacing
    }

    /// Current command members, in their original human-then-grid order.
    pub fn members(&self) -> &[u16] {
        &self.members
    }

    fn check(&self, players: &Players, participant: u16, route: Route) -> Result<(), Error> {
        let group = self.pacing.ok_or(Error::UnknownObject)?;
        let owned = match route.car {
            Some(key) => players
                .ai_ids(self.connection, self.persona, key)
                .is_some_and(|ids| ids.participant == participant && ids.actor == route.actor),
            None => {
                participant == self.human
                    && players.owned_actor(self.connection, self.persona, participant)
                        == Some(route.actor)
            }
        };
        if !owned
            || !players.objects().get(route.actor).is_some_and(|object| {
                matches!(&object.current, Update::Actor(a)
                if a.references_a.as_deref() == Some(&[route.goal, group]))
            })
        {
            return Err(Error::UnknownObject);
        }
        Ok(())
    }

    /// Apply accepted channel progress. A finish releases only this route.
    /// Repeated finish/release is inert and cannot remove another command.
    pub fn crossed(
        &mut self,
        players: &mut Players,
        participant: u16,
        crossing: nfs_world::race_progress::Crossing,
    ) -> Result<ai::GoalChanges, Error> {
        let Some(&route) = self.routes.get(&participant) else {
            return Ok(ai::GoalChanges::default());
        };
        self.check(players, participant, route)?;
        if let Some(pair) = crossing.goal_progress {
            let record = players.change(
                route.goal,
                Update::GotoWaypointGoal(goto_waypoint::Update::Progress(Some(pair))),
            )?;
            return Ok(ai::GoalChanges {
                records: record.into_iter().collect(),
                ..Default::default()
            });
        }
        if crossing.finish.is_none() {
            return Err(Error::Shape);
        }
        self.withdraw(players, participant)
    }

    /// Release one route and its pacing membership, whether it finished or left
    /// the racing state through DNF. Successor goals remain the caller's choice.
    pub fn withdraw(
        &mut self,
        players: &mut Players,
        participant: u16,
    ) -> Result<ai::GoalChanges, Error> {
        let Some(&route) = self.routes.get(&participant) else {
            return Ok(ai::GoalChanges::default());
        };
        self.check(players, participant, route)?;
        // Releasing a member removes its serialized pacing entry as well as its
        // route. Stage both changes so a malformed group cannot partly finish.
        let group = self.pacing.ok_or(Error::UnknownObject)?;
        let Some(Update::AiPacingGroup(pacing::Update {
            entries: Some(entries),
        })) = players.objects().get(group).map(|object| &object.current)
        else {
            return Err(Error::TypeMismatch);
        };
        if entries.iter().map(|e| e.participant).collect::<Vec<_>>() != self.members {
            return Err(Error::Shape);
        }
        let entries = entries
            .iter()
            .copied()
            .filter(|e| e.participant != participant)
            .collect();
        let mut world = players.clone();
        let mut changes = self.release(&mut world, participant, route)?;
        changes.records.extend(world.change(
            group,
            Update::AiPacingGroup(pacing::Update {
                entries: Some(entries),
            }),
        )?);
        *players = world;
        self.routes.remove(&participant);
        self.members.retain(|&p| p != participant);
        Ok(changes)
    }

    fn release(
        &self,
        players: &mut Players,
        participant: u16,
        route: Route,
    ) -> Result<ai::GoalChanges, Error> {
        match route.car {
            Some(key) => players.replace_ai_goals(self.connection, self.persona, key, vec![], None),
            None => players.release_player_race_goals(
                self.connection,
                self.persona,
                participant,
                route.goal,
                self.pacing.ok_or(Error::UnknownObject)?,
            ),
        }
    }

    /// Update the remaining command members. Finished racers no longer affect
    /// group size, rank or reference-player selection.
    pub fn pace(&self, players: &mut Players, skills: &[f32]) -> Result<Option<Record>, Error> {
        let group = self.pacing.ok_or(Error::UnknownObject)?;
        if skills.len() != self.members.len() || skills.iter().any(|v| !v.is_finite()) {
            return Err(Error::Shape);
        }
        for (&participant, &route) in &self.routes {
            self.check(players, participant, route)?;
        }
        let entries = self
            .members
            .iter()
            .zip(skills)
            .map(|(&participant, &skill)| pacing::Entry {
                participant,
                value: skill.to_bits(),
            })
            .collect();
        players.change(
            group,
            Update::AiPacingGroup(pacing::Update {
                entries: Some(entries),
            }),
        )
    }

    /// Cancellation and event teardown release remaining routes before the
    /// pacing group. Already finished cars keep their successor commands.
    pub fn clear(&mut self, players: &mut Players) -> Result<ai::GoalChanges, Error> {
        let Some(group) = self.pacing else {
            return Ok(ai::GoalChanges::default());
        };
        let mut world = players.clone();
        let mut changes = ai::GoalChanges::default();
        for (&participant, &route) in &self.routes {
            self.check(&world, participant, route)?;
            extend(&mut changes, self.release(&mut world, participant, route)?);
        }
        if world.remove_ai_pacing_group(self.connection, self.persona, group)? {
            changes.deleted.push(group);
        }
        *players = world;
        self.routes.clear();
        self.members.clear();
        self.pacing = None;
        Ok(changes)
    }
}
