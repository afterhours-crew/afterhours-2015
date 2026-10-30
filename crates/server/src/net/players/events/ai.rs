// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::event_ai::{Commands, Racers, navigation::Sample};
use nfs_world::replication::{Initial, Update, players::ai::GoalChanges};

#[derive(Clone, Debug)]
pub(super) struct Grid {
    pub racers: Racers,
    participants: Vec<u16>,
    pub commands: Option<Commands>,
    route: Option<nfs_world::race_pacing::Route>,
    next_pacing: u64,
    next_positions: u64,
    pub(super) positions: BTreeMap<u16, u16>,
    pub outros: BTreeMap<u16, u64>,
    pub deadlines: BTreeMap<u16, f64>,
}

pub(super) fn goals(values: Vec<Initial>) -> Result<Vec<(Initial, Update)>, replication::Error> {
    values
        .into_iter()
        .map(|initial| {
            let update = match &initial {
                Initial::GotoWaypointGoal(g) if !g.progress_updates => {
                    Update::GotoWaypointGoal(replication::goto_waypoint::Update::NoProgress)
                }
                Initial::SpeedLimitGoal(_) => Update::SpeedLimitGoal,
                Initial::WanderGoal(_) => Update::WanderGoal,
                _ => return Err(replication::Error::TypeMismatch),
            };
            Ok((initial, update))
        })
        .collect()
}

pub(super) fn append(out: &mut Out, changes: GoalChanges) {
    out.records.extend(changes.records);
    out.deleted.extend(changes.deleted);
}

impl State {
    /// Event cars remain racers in the location service throughout the intro,
    /// race and outro. Adoption removes a member here as it enters the roaming
    /// population, so the two sources never publish the same participant twice.
    pub(in crate::net::players) fn locations(
        &self,
        players: &Players,
        persona: u64,
    ) -> Result<Vec<crate::rival_content::population::Location>, replication::Error> {
        self.entries
            .values()
            .filter_map(|e| e.race.as_ref()?.ai.as_ref())
            .flat_map(|grid| &grid.racers.members)
            .map(|member| {
                let key = member.selection.car_key;
                if players.ai_ids(HOST_SELECTOR as u8, persona, key) != Some(member.ids) {
                    return Err(replication::Error::UnknownObject);
                }
                Ok((
                    member.ids.participant,
                    players
                        .ai_vehicle_physics(HOST_SELECTOR as u8, persona, key)?
                        .decode([0.; 3])?
                        .position,
                ))
            })
            .collect()
    }

    pub(in crate::net::players) fn preserved(&self, players: &Players, persona: u64) -> Vec<u16> {
        self.entries
            .values()
            .filter_map(|e| e.race.as_ref()?.ai.as_ref())
            .flat_map(|g| g.participants.iter().copied())
            .filter(|&p| players.owns_ai_participant(HOST_SELECTOR as u8, persona, p))
            .collect()
    }
    pub(in crate::net::players) fn delegated(
        &self,
        players: &Players,
        persona: u64,
    ) -> BTreeMap<u16, u64> {
        self.entries
            .values()
            .filter_map(|e| e.race.as_ref()?.ai.as_ref())
            .flat_map(|grid| &grid.racers.members)
            .filter_map(|member| {
                let key = member.selection.car_key;
                (players.ai_ids(HOST_SELECTOR as u8, persona, key) == Some(member.ids))
                    .then_some((member.ids.vehicle, key))
            })
            .collect()
    }
}

impl PlayerListener {
    pub(super) fn event_grid(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        out: &mut Out,
    ) -> Result<Option<Grid>, replication::Error> {
        let Some(ai) = content.ai() else {
            return Ok(None);
        };
        // Missing prepared navigation cannot silently degrade an authored grid.
        content
            .navigation()
            .ok_or(replication::Error::Unsupported)?;
        self.rival_population
            .as_ref()
            .ok_or(replication::Error::Unsupported)?;
        let registry = match out.registrations.as_mut() {
            Some(registry) => registry,
            None => out.registrations.insert(
                self.population
                    .as_ref()
                    .ok_or(replication::Error::Unsupported)?
                    .registrations()
                    .clone(),
            ),
        };
        let selection = state.ai_cursor.select(ai)?;
        let created = ai.create(
            players,
            registry,
            HOST_SELECTOR as u8,
            self.persona,
            &selection,
        )?;
        out.records.extend(created.records);
        out.messages.extend(created.messages);
        // These calls must follow every creation section, including the cars.
        out.rpcs.extend(created.calls);
        Ok(Some(Grid {
            participants: created
                .racers
                .members
                .iter()
                .map(|m| m.ids.participant)
                .collect(),
            racers: created.racers,
            commands: None,
            route: None,
            next_pacing: 0,
            next_positions: 0,
            positions: BTreeMap::new(),
            outros: BTreeMap::new(),
            deadlines: BTreeMap::new(),
        }))
    }

    pub(super) fn event_ai_intro(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        race: &mut Race,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let Some(grid) = &race.ai else {
            return Ok(());
        };
        let intro = goals(
            content
                .intro_goals()?
                .ok_or(replication::Error::Unsupported)?,
        )?;
        let spawner = content.spawner(names::INTRO_SEQUENCE)?;
        for member in &grid.racers.members {
            let p = Some(member.ids.participant);
            for (state, method) in [
                (Gm::Root, 2),
                (Gm::WaitForAi, 0),
                (Gm::WaitForAi, 1),
                (Gm::Main, 0),
                (Gm::Main, 1),
            ] {
                out.rpcs.push(HostRpc::GameMode(content.state_call(
                    players.objects(),
                    race.game_mode,
                    state,
                    method,
                    p,
                    Arguments::None,
                )?));
            }
            out.rpcs.push(HostRpc::GameMode(content.field_call(
                players.objects(),
                race.game_mode,
                spawner,
                0,
                p,
            )?));
            append(
                out,
                players.replace_ai_goals(
                    HOST_SELECTOR as u8,
                    self.persona,
                    member.selection.car_key,
                    intro.clone(),
                    None,
                )?,
            );
        }
        Ok(())
    }

    pub(super) fn event_ai_start(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        participant: u16,
        race: &mut Race,
        now_ms: u64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let Some(grid) = &mut race.ai else {
            return Ok(());
        };
        let navigation = content.navigation().ok_or(replication::Error::Shape)?;
        let position = players
            .vehicle_physics(HOST_SELECTOR as u8, self.persona, race.car)?
            .decode([0.; 3])?
            .position;
        let route = navigation.start(position)?;
        let samples = samples(
            content,
            players,
            participant,
            race.car,
            &grid.racers,
            None,
            self.persona,
        )?;
        let measured = navigation.measure(
            &route,
            race.progress.as_ref().ok_or(replication::Error::Shape)?,
            &samples,
            Some(participant),
        )?;
        let (Initial::GotoWaypointGoal(goal), _) = content.race_goal()? else {
            return Err(replication::Error::TypeMismatch);
        };
        let (commands, changes) = Commands::start(
            players,
            HOST_SELECTOR as u8,
            self.persona,
            participant,
            &grid.racers,
            goal,
            &measured.skills,
        )?;
        append(out, changes);
        grid.commands = Some(commands);
        grid.route = Some(route);
        grid.next_pacing = now_ms.saturating_add(2001);
        grid.next_positions = now_ms;
        let deadline = race.deadline.ok_or(replication::Error::Shape)?;
        grid.deadlines = grid
            .racers
            .members
            .iter()
            .map(|m| (m.ids.participant, deadline))
            .collect();
        Ok(())
    }

    pub(super) fn event_ai_countdown(
        &self,
        content: &event_logic::Content,
        players: &Players,
        race: &Race,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let Some(grid) = &race.ai else {
            return Ok(());
        };
        let intro = content.spawner(names::INTRO_SEQUENCE)?;
        for member in &grid.racers.members {
            let p = Some(member.ids.participant);
            out.rpcs.push(HostRpc::GameMode(content.field_call(
                players.objects(),
                race.game_mode,
                intro,
                1,
                p,
            )?));
            out.rpcs.push(HostRpc::GameMode(content.state_call(
                players.objects(),
                race.game_mode,
                Gm::Countdown,
                0,
                p,
                Arguments::None,
            )?));
        }
        Ok(())
    }

    pub(super) fn event_ai_racing_calls(
        &self,
        content: &event_logic::Content,
        players: &Players,
        race: &Race,
        deadline: f64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let Some(grid) = &race.ai else {
            return Ok(());
        };
        for member in &grid.racers.members {
            let p = Some(member.ids.participant);
            for (state, method, arguments) in [
                (Gm::Countdown, 1, Arguments::None),
                (Gm::Waypoints, 0, Arguments::None),
                (
                    Gm::Positions,
                    0,
                    Arguments::EventPosition {
                        position: 0,
                        active: true,
                    },
                ),
                (Gm::Dnf, 0, Arguments::time(deadline)),
            ] {
                out.rpcs.push(HostRpc::GameMode(content.state_call(
                    players.objects(),
                    race.game_mode,
                    state,
                    method,
                    p,
                    arguments,
                )?));
            }
            out.rpcs.push(HostRpc::GameMode(
                content.state_call(
                    players.objects(),
                    race.game_mode,
                    Gm::Waypoints,
                    3,
                    p,
                    Arguments::Word(
                        content
                            .ai()
                            .ok_or(replication::Error::Shape)?
                            .car_id(member.selection.persona)?,
                    ),
                )?,
            ));
        }
        Ok(())
    }

    pub(super) fn event_ai_tick(
        &self,
        catalog: &Catalog,
        players: &mut Players,
        state: &mut State,
        now_ms: u64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        for (&human, entry) in &mut state.entries {
            let Some(race) = entry.race.as_mut() else {
                continue;
            };
            let content = catalog
                .get(race.event_id)
                .ok_or(replication::Error::UnknownObject)?;
            let Some(grid) = &mut race.ai else {
                continue;
            };
            if !matches!(
                race.exit,
                Some(Exit::Restart | Exit::Abandon | Exit::TimedOut)
            ) {
                let now = self
                    .event_seconds(now_ms)
                    .ok_or(replication::Error::Shape)?;
                let expired: Vec<_> = grid
                    .deadlines
                    .iter()
                    .filter(|(_, deadline)| now >= **deadline)
                    .map(|(&participant, _)| participant)
                    .collect();
                for participant in expired {
                    let progress = race.progress.as_mut().ok_or(replication::Error::Shape)?;
                    if !progress.withdraw(participant) {
                        return Err(replication::Error::Shape);
                    }
                    append(
                        out,
                        grid.commands
                            .as_mut()
                            .ok_or(replication::Error::Shape)?
                            .withdraw(players, participant)?,
                    );
                    out.rpcs.push(HostRpc::GameMode(content.state_call(
                        players.objects(),
                        race.game_mode,
                        Gm::Positions,
                        0,
                        Some(participant),
                        Arguments::EventPosition {
                            position: 0,
                            active: false,
                        },
                    )?));
                    for (state, method) in [
                        (Gm::Dnf, 1),
                        (Gm::Waypoints, 1),
                        (Gm::DnfState, 0),
                        (Gm::DnfState, 1),
                        (Gm::Root, 3),
                        (Gm::Final, 0),
                    ] {
                        out.rpcs.push(HostRpc::GameMode(content.state_call(
                            players.objects(),
                            race.game_mode,
                            state,
                            method,
                            Some(participant),
                            Arguments::None,
                        )?));
                    }
                    let index = grid
                        .racers
                        .members
                        .iter()
                        .position(|m| m.ids.participant == participant)
                        .ok_or(replication::Error::Shape)?;
                    self.event_ai_adopt(players, &mut grid.racers, index, out)?;
                    grid.participants.retain(|&p| p != participant);
                    grid.deadlines.remove(&participant);
                }
                let completed: Vec<_> = grid
                    .outros
                    .iter()
                    .filter(|(_, due)| **due <= now_ms)
                    .map(|(&participant, _)| participant)
                    .collect();
                for participant in completed {
                    // Final-state entry releases this participant, which invokes
                    // the authored individual-completion handoff. The human's
                    // race and the other opponents continue independently.
                    for (state, method) in [
                        (Gm::Root, 3),
                        (Gm::Final, 0),
                        (Gm::ResultReporting, 0),
                        (Gm::ResultReporting, 1),
                    ] {
                        out.rpcs.push(HostRpc::GameMode(content.state_call(
                            players.objects(),
                            race.game_mode,
                            state,
                            method,
                            Some(participant),
                            Arguments::None,
                        )?));
                    }
                    out.rpcs.push(HostRpc::GameMode(content.field_call(
                        players.objects(),
                        race.game_mode,
                        content.spawner(names::OUTRO_SEQUENCE)?,
                        1,
                        Some(participant),
                    )?));
                    let index = grid
                        .racers
                        .members
                        .iter()
                        .position(|m| m.ids.participant == participant)
                        .ok_or(replication::Error::Shape)?;
                    self.event_ai_adopt(players, &mut grid.racers, index, out)?;
                    grid.participants.retain(|&p| p != participant);
                    grid.outros.remove(&participant);
                }
            }
            let Some(commands) = &grid.commands else {
                continue;
            };
            if commands.members().is_empty()
                || (now_ms < grid.next_pacing && now_ms < grid.next_positions)
                || matches!(
                    race.exit,
                    Some(Exit::Restart | Exit::Abandon | Exit::TimedOut)
                )
            {
                continue;
            }
            let progress = race.progress.as_ref().ok_or(replication::Error::Shape)?;
            let samples = samples(
                content,
                players,
                human,
                race.car,
                &grid.racers,
                Some(commands.members()),
                self.persona,
            )?;
            let reference = commands.members().contains(&human).then_some(human);
            let measured = content
                .navigation()
                .ok_or(replication::Error::Shape)?
                .measure(
                    grid.route.as_ref().ok_or(replication::Error::Shape)?,
                    progress,
                    &samples,
                    reference,
                )?;
            if now_ms >= grid.next_pacing {
                out.records
                    .extend(commands.pace(players, &measured.skills)?);
                grid.next_pacing = now_ms.saturating_add(2001);
            }
            if now_ms >= grid.next_positions {
                for (participant, position) in progress.positions(&measured.remaining)? {
                    // Results retain finish order, but the presentation collector
                    // contains only racers still participating. A later tick must
                    // never undo the removal sent by the finish transition.
                    if progress
                        .participant(participant)
                        .is_none_or(|p| p.finished.is_some())
                    {
                        continue;
                    }
                    if grid.positions.insert(participant, position) == Some(position) {
                        continue;
                    }
                    out.rpcs.push(HostRpc::GameMode(content.state_call(
                        players.objects(),
                        race.game_mode,
                        Gm::Positions,
                        0,
                        Some(participant),
                        Arguments::EventPosition {
                            position: position.into(),
                            active: true,
                        },
                    )?));
                    if participant == human {
                        out.rpcs.push(HostRpc::GameMode(content.state_call(
                            players.objects(),
                            race.game_mode,
                            Gm::LocalPosition,
                            0,
                            None,
                            Arguments::Word(position.into()),
                        )?));
                    }
                }
                grid.next_positions = now_ms.saturating_add(
                    (content
                        .event()
                        .game_mode
                        .standings_interval
                        .ok_or(replication::Error::Shape)?
                        * 1000.)
                        .round() as u64,
                );
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn event_ai_finish(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        race: &mut Race,
        participant: u16,
        position: u16,
        now_ms: u64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let grid = race.ai.as_mut().ok_or(replication::Error::Shape)?;
        grid.deadlines.remove(&participant);
        let member = grid
            .racers
            .members
            .iter()
            .find(|m| m.ids.participant == participant)
            .ok_or(replication::Error::UnknownObject)?;
        let p = Some(participant);
        for (state, method, arguments) in [
            (Gm::Waypoints, 1, Arguments::None),
            (
                Gm::Positions,
                0,
                Arguments::EventPosition {
                    position: position.into(),
                    active: false,
                },
            ),
            (Gm::ResultReporting, 0, Arguments::None),
            (Gm::ResultReporting, 1, Arguments::None),
        ] {
            out.rpcs.push(HostRpc::GameMode(content.state_call(
                players.objects(),
                race.game_mode,
                state,
                method,
                p,
                arguments,
            )?));
        }
        out.rpcs.push(HostRpc::GameMode(content.field_call(
            players.objects(),
            race.game_mode,
            content.spawner(names::OUTRO_SEQUENCE)?,
            0,
            p,
        )?));
        append(
            out,
            players.replace_ai_goals(
                HOST_SELECTOR as u8,
                self.persona,
                member.selection.car_key,
                goals(content.outro_goals()?)?,
                None,
            )?,
        );
        let timeout = content
            .sequence(names::OUTRO_SEQUENCE)?
            .wander
            .ok_or(replication::Error::Shape)?
            .timeout;
        grid.outros.insert(
            participant,
            now_ms.saturating_add((f64::from(timeout) * 1000.) as u64),
        );
        Ok(())
    }

    fn event_ai_adopt(
        &self,
        players: &mut Players,
        racers: &mut Racers,
        index: usize,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let member = *racers.members.get(index).ok_or(replication::Error::Bound)?;
        let rivals = match out.rivals.as_mut() {
            Some(rivals) => rivals,
            None => out.rivals.insert(
                self.rival_population
                    .clone()
                    .ok_or(replication::Error::Unsupported)?,
            ),
        };
        let changes = rivals.adopt(
            players,
            HOST_SELECTOR as u8,
            self.persona,
            member,
            &mut racers.authority,
        )?;
        append(out, changes);
        racers.members.remove(index);
        Ok(())
    }

    pub(super) fn event_ai_stop(
        &self,
        players: &mut Players,
        race: &mut Race,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let Some(grid) = &mut race.ai else {
            return Ok(());
        };
        if let Some(commands) = &mut grid.commands {
            append(out, commands.clear(players)?);
        }
        while !grid.racers.members.is_empty() {
            self.event_ai_adopt(players, &mut grid.racers, 0, out)?;
        }
        grid.outros.clear();
        grid.deadlines.clear();
        Ok(())
    }
}

fn samples(
    content: &event_logic::Content,
    players: &Players,
    human: u16,
    car: u16,
    racers: &Racers,
    active: Option<&[u16]>,
    persona: u64,
) -> Result<Vec<Sample>, replication::Error> {
    let include = |p| active.is_none_or(|ids| ids.contains(&p));
    let mut samples = Vec::new();
    if include(human) {
        samples.push(Sample {
            participant: human,
            position: players
                .vehicle_physics(HOST_SELECTOR as u8, persona, car)?
                .decode([0.; 3])?
                .position,
            base_skill: 0.5,
        });
    }
    for member in &racers.members {
        if include(member.ids.participant) {
            samples.push(Sample {
                participant: member.ids.participant,
                position: players
                    .ai_vehicle_physics(HOST_SELECTOR as u8, persona, member.selection.car_key)?
                    .decode([0.; 3])?
                    .position,
                base_skill: content
                    .ai()
                    .ok_or(replication::Error::Shape)?
                    .roster()
                    .pacing_skills[member.selection.persona]
                    .ok_or(replication::Error::Shape)?,
            });
        }
    }
    Ok(samples)
}
