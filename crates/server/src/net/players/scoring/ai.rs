// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! AI sessions follow participant membership, independently of delegated cars.
use super::*;
use nfs_content::{
    events::discovery::{Endpoint, membership::Fact},
    scoring::ai_deployment::membership::Owner,
};
use nfs_world::{
    replication::{players::ai::Participant, sublevel},
    scoring::{draft, properties::Kind, proximity::Member, value::Value},
};

#[derive(Clone, Debug)]
pub(in crate::net::players) struct Live {
    pub entity: Entity,
    pub car: Option<(u64, u16)>,
    pub last_ms: u64,
}

fn contains(
    players: &Players,
    endpoint: Endpoint,
    participant: u16,
) -> Result<bool, replication::Error> {
    let world = players.objects();
    let scene = world
        .scene(endpoint.scene_key)
        .ok_or(replication::Error::UnknownObject)?;
    let Some(replication::Initial::SubLevel { fields, .. }) = world.get(scene).map(|o| &o.initial)
    else {
        return Err(replication::Error::TypeMismatch);
    };
    let Some(sublevel::Initial::RpcReferences { value, .. }) = fields.get(endpoint.serializer)
    else {
        return Err(replication::Error::TypeMismatch);
    };
    Ok(value.contains(&participant))
}

impl PlayerListener {
    pub(super) fn desired_ai_scoring(
        &self,
        content: &Content,
    ) -> Result<BTreeMap<u16, Participant>, replication::Error> {
        let players = &self.players;
        // The scheduler runs before the player joins. An empty world has no
        // AI eligibility to evaluate and no account owner to look up yet.
        if players.ai_count() == 0 && players.ai_slot_count() == 0 {
            return Ok(BTreeMap::new());
        }
        let owned = players.ai_participants(HOST_SELECTOR as u8, self.persona)?;
        if owned.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut racers = BTreeSet::new();
        if let Some(population) = &self.rival_population {
            racers.extend(
                population
                    .locations(players, HOST_SELECTOR as u8, self.persona)?
                    .into_iter()
                    .map(|(id, _)| id),
            );
        }
        racers.extend(
            self.event_state
                .locations(players, self.persona)?
                .into_iter()
                .map(|(id, _)| id),
        );
        for (first, second, _) in self.intro_modes.values() {
            for mode in [first, second] {
                racers.extend(mode.locations(players)?.into_iter().map(|(id, _)| id));
            }
        }
        let mut traffic = BTreeSet::new();
        if let Some(population) = &self.traffic_population {
            for (car, key) in population.delegated(players, HOST_SELECTOR as u8, self.persona) {
                let ids = players
                    .ai_ids(HOST_SELECTOR as u8, self.persona, key)
                    .ok_or(replication::Error::UnknownObject)?;
                if ids.vehicle != car || racers.contains(&ids.participant) {
                    return Err(replication::Error::Shape);
                }
                traffic.insert(ids.participant);
            }
        }
        let deployment = &content.installed().ai;
        let mut desired = BTreeMap::new();
        for participant in owned {
            // Only the population owner establishes a role. A car, mode, name
            // or distance cannot prove non-membership in the cops' set.
            if !racers.contains(&participant.participant)
                && !traffic.contains(&participant.participant)
            {
                tracing::warn!(
                    participant = participant.participant,
                    "AI scoring participant has no population owner"
                );
                return Err(replication::Error::Unsupported);
            }
            let mut facts = BTreeMap::new();
            for fact in deployment.eligibility.facts() {
                let value = match fact {
                    Fact::States(endpoints) => {
                        let mut any = false;
                        for endpoint in endpoints {
                            any |= contains(players, *endpoint, participant.participant)?;
                        }
                        any
                    }
                    Fact::Mutable(id) if deployment.eligibility.empty_sets().contains(id) => false,
                    _ => {
                        let binding = deployment
                            .membership
                            .iter()
                            .find(|b| &b.fact == fact)
                            .ok_or(replication::Error::Unsupported)?;
                        match &binding.owner {
                            Owner::Ai { manager } => {
                                contains(players, *manager, participant.participant)?
                            }
                            Owner::Traffic { .. } => traffic.contains(&participant.participant),
                            // These populations never create dispatch or roadblock
                            // participants. A future cop owner must supply its own
                            // explicit membership; unknown populations reject above.
                            Owner::Cops { .. } => false,
                        }
                    }
                };
                facts.insert(fact, value);
            }
            if deployment
                .eligibility
                .evaluate(|f| facts.get(f).copied())
                .map_err(fail("AI eligibility"))?
            {
                desired.insert(participant.participant, participant);
            }
        }
        Ok(desired)
    }

    fn ai_scoring_inputs(
        &self,
        content: &Content,
        players: &Players,
        participant: Participant,
        now_ms: u64,
    ) -> Result<Inputs, replication::Error> {
        let mut inputs = idle_inputs(u64::from(participant.participant), now_ms);
        if let Some((key, car)) = participant.car {
            let physics = players.ai_vehicle_physics(HOST_SELECTOR as u8, self.persona, key)?;
            let motion = physics.decode([0.; 3])?;
            inputs.vehicle = Some(
                Vehicle::from_physics(u32::from(car), &physics)
                    .map_err(fail("AI scoring physics"))?,
            );
            inputs.others = other_vehicles(players, car);
            inputs.road = content
                .road()
                .map(|network| network.sample(motion.position, motion.velocity))
                .transpose()
                .map_err(fail("AI road sample"))?
                .flatten();
        }
        Ok(inputs)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn advance_ai_scoring(
        &self,
        content: &Content,
        players: &mut Players,
        state: &mut State,
        desired: &BTreeMap<u16, Participant>,
        now_ms: u64,
        collected: &mut Collected,
    ) -> Result<(), replication::Error> {
        let (reports, other): (Vec<_>, Vec<_>) = std::mem::take(&mut state.reports)
            .into_iter()
            .partition(|report| report.which == Which::Ai);
        state.reports = other;
        let retiring: Vec<_> = state
            .ai
            .keys()
            .filter(|id| !desired.contains_key(id))
            .copied()
            .collect();
        for participant in retiring {
            let live = state
                .ai
                .remove(&participant)
                .ok_or(replication::Error::Shape)?;
            let output = live
                .entity
                .session
                .retire(&idle_inputs(u64::from(participant), now_ms))
                .map_err(fail("AI scoring retirement"))?;
            let present = players.objects().get(live.entity.ghost).is_some();
            if present {
                Self::collect_output(
                    content,
                    players,
                    state,
                    Scope {
                        participant,
                        which: Which::Ai,
                        ghost: live.entity.ghost,
                    },
                    output,
                    collected,
                )?;
            } else if players.owns_ai_participant(HOST_SELECTOR as u8, self.persona, participant)
                || output != Output::default()
            {
                // Population teardown already queued this registered child's
                // deletion before its participant. No late calls may target it.
                return Err(replication::Error::Unsupported);
            }
            let unpublished = state
                .dirty
                .remove(&live.entity.ghost)
                .is_some_and(|record| record.initial.is_some());
            if present {
                players.remove_entities(&[live.entity.ghost])?;
                if !unpublished {
                    state.dirty_deleted.push(live.entity.ghost);
                }
            }
            tracing::debug!(
                participant,
                ghost = live.entity.ghost,
                "AI scoring participant retired"
            );
        }
        for (&participant, &owned) in desired {
            let inputs = self.ai_scoring_inputs(content, players, owned, now_ms)?;
            if !state.ai.contains_key(&participant) {
                if state.ai.len() >= nfs_world::replication::players::ai::MAX_AI {
                    return Err(replication::Error::Bound);
                }
                let (session, output) = content
                    .start(Which::Ai, &inputs)
                    .map_err(fail("AI scoring start"))?;
                let creation = content.creation(Which::Ai, &session, participant, players)?;
                let record = players
                    .spawn_entities(vec![creation], content.creation_content())?
                    .remove(0);
                let ghost = record.id;
                players.bind_ai_entity(HOST_SELECTOR as u8, self.persona, participant, ghost)?;
                merge_record(&mut state.dirty, record)?;
                let reports = content.report_bindings(Which::Ai, players, ghost)?;
                state.ai.insert(
                    participant,
                    Live {
                        entity: Entity {
                            which: Which::Ai,
                            ghost,
                            session,
                            reports,
                        },
                        car: owned.car,
                        last_ms: now_ms,
                    },
                );
                tracing::debug!(participant, ghost, car = ?owned.car, "AI scoring participant started");
                Self::collect_output(
                    content,
                    players,
                    state,
                    Scope {
                        participant,
                        which: Which::Ai,
                        ghost,
                    },
                    Output {
                        synced: Vec::new(),
                        ..output
                    },
                    collected,
                )?;
            }
            let live = state
                .ai
                .get_mut(&participant)
                .ok_or(replication::Error::Shape)?;
            if players.objects().get(live.entity.ghost).is_none() {
                return Err(replication::Error::UnknownObject);
            }
            let changed_car = live.car != owned.car;
            live.car = owned.car;
            for binding in &content.installed().ai.pacing {
                let manager = owned.managers.ok_or(replication::Error::UnknownObject)?;
                if manager.scene != binding.actors.scene_key
                    || manager.actors != binding.actors.serializer
                {
                    return Err(replication::Error::Shape);
                }
                let value = players
                    .ai_pacing_sample(HOST_SELECTOR as u8, self.persona, participant)?
                    .map(|sample| binding.library.evaluate(Some(owned.personality), sample))
                    .transpose()
                    .map_err(fail("AI scoring pacing"))?;
                live.entity
                    .session
                    .set_pacing_sample(&binding.sources, u64::from(participant), value)
                    .map_err(fail("AI scoring pacing sample"))?;
            }
            let last_ms = live.last_ms;
            for report in reports
                .iter()
                .filter(|report| report.participant == participant)
            {
                let live = state
                    .ai
                    .get_mut(&participant)
                    .ok_or(replication::Error::Shape)?;
                let output = live
                    .entity
                    .session
                    .report(report.node, &report.report, &inputs)
                    .map_err(fail("AI scoring report"))?;
                let ghost = live.entity.ghost;
                Self::collect_output(
                    content,
                    players,
                    state,
                    Scope {
                        participant,
                        which: Which::Ai,
                        ghost,
                    },
                    output,
                    collected,
                )?;
            }
            if now_ms <= last_ms && !changed_car {
                continue;
            }
            let mut remaining =
                ((now_ms.saturating_sub(last_ms)) as f32 / 1000.).min(MAX_TICK_SECONDS);
            let mut step_inputs = inputs;
            step_inputs.now -= f64::from(remaining);
            loop {
                let step = remaining.min(TICK_SECONDS);
                step_inputs.now += f64::from(step);
                let (output, ghost) = {
                    let live = state
                        .ai
                        .get_mut(&participant)
                        .ok_or(replication::Error::Shape)?;
                    (
                        live.entity
                            .session
                            .tick(step, &step_inputs)
                            .map_err(fail("AI scoring tick"))?,
                        live.entity.ghost,
                    )
                };
                Self::collect_output(
                    content,
                    players,
                    state,
                    Scope {
                        participant,
                        which: Which::Ai,
                        ghost,
                    },
                    output,
                    collected,
                )?;
                remaining -= step;
                if remaining <= 0. {
                    break;
                }
            }
            state
                .ai
                .get_mut(&participant)
                .ok_or(replication::Error::Shape)?
                .last_ms = now_ms;
        }
        Ok(())
    }
}

/// Query membership is the AI session's authored Boolean, projected through
/// its current car. Carless participants retain their session but add no car.
pub(super) fn members(
    content: &Content,
    players: &Players,
    state: &State,
    persona: u64,
) -> Result<Vec<Member>, replication::Error> {
    let mut result = Vec::new();
    for (&participant, live) in &state.ai {
        let Some((key, car)) = live.car else {
            continue;
        };
        let motion = players
            .ai_vehicle_physics(HOST_SELECTOR as u8, persona, key)?
            .decode([0.; 3])?;
        for binding in &content.installed().ai.boolean_sets {
            if live.entity.session.participant_value(
                u64::from(participant),
                &binding.property,
                Kind::Boolean,
            ) == Some(Value::Boolean(true))
            {
                if result.len() >= nfs_world::scoring::proximity::MAX_VEHICLES {
                    return Err(replication::Error::Bound);
                }
                result.push(Member {
                    set: binding.set.port,
                    vehicle: draft::Vehicle {
                        identity: u32::from(car),
                        position: motion.position,
                        velocity: motion.velocity,
                    },
                });
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
