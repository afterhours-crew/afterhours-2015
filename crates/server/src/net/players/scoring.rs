// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Live free-roam scoring for owned participants: the two installed driving
//! participants are created when a participant drives in free roam, ticked
//! from its car's accepted movement, fed its decoded query reports, and
//! retired with the root disable event when it leaves free roam. Keeper
//! awards become one category credit per bank, committed by the storage
//! exchange; the committed snapshot refreshes the REP presentation fields.
//!
//! Replication of scoring state is coalesced: property changes are merged per
//! entity and flushed as one section at a bounded rate when the queue has
//! room, so the driving traffic and the send window are never starved.
use super::*;
use crate::scoring::{Content, TIME_SCALE, Which};
use nfs_world::{
    participants::HostRpc,
    replication::{Record, section::Setup},
    scoring::{
        interpreter::{Award, Inputs, Output, Session, Vehicle},
        reports::{Binding as ReportBinding, Report},
    },
};
use std::sync::Arc;

/// The participant root's disable notification, which banks every category
/// (installed topology: OnDisabled forwards into the scoring prefab's bank
/// splitter; E887).
pub const ROOT_DISABLED: u32 = 0xfc4b_bbb4;
const MAX_LIVE: usize = 16;
pub(super) const MAX_QUEUED_REPORTS: usize = 64;
const MAX_PENDING_AWARDS: usize = 32;
/// One engine update. Elapsed time is covered in steps of this size, like
/// the client's fixed update, so short periodic delays fire once per step.
const TICK_SECONDS: f32 = 1.0 / 30.0;
/// Longest elapsed time one advance may cover; older time is dropped.
const MAX_TICK_SECONDS: f32 = 0.5;
/// Minimum interval between replication flushes of coalesced scoring state.
const FLUSH_INTERVAL_MS: u64 = 100;
/// Coalesced records waiting for a flush (one per live entity at most, plus
/// reputation fields); more than this means the queue never drains.
const MAX_DIRTY: usize = 64;

#[derive(Clone, Debug)]
pub(super) struct Entity {
    pub ghost: u16,
    pub session: Session,
    pub reports: Vec<(u16, ReportBinding)>,
}

#[derive(Clone, Debug)]
pub(super) struct Live {
    pub car: u16,
    pub entities: [Entity; 2],
    pub last_ms: u64,
}

#[derive(Clone, Debug)]
pub(super) struct QueuedReport {
    pub participant: u16,
    pub which: Which,
    pub node: u16,
    pub report: Report,
}

/// One banked credit awaiting the durable transaction.
#[derive(Clone, Debug)]
pub(in crate::net) struct AwardRequest {
    pub participant: u16,
    pub credit: nfs_services::reputation::Scores,
    /// Retry key, stable for this credit; the store answers a repeated id
    /// without a second mutation.
    pub batch_id: u64,
    pub thresholds: nfs_services::reputation::Thresholds,
}

/// A committed credit: the refreshed account view and item collection.
#[derive(Debug)]
pub(in crate::net) struct Credited {
    pub loaded: Arc<crate::persistent::Loaded>,
    pub items: Arc<nfs_world::items::Collection>,
    pub replayed: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    pub live: BTreeMap<u16, Live>,
    pub reported: BTreeSet<u16>,
    pub reports: Vec<QueuedReport>,
    pub pending_awards: VecDeque<AwardRequest>,
    pub sequence: u64,
    pub batch_base: u64,
    /// Records already applied to the committed world, merged per object,
    /// waiting for the next flush. A creation keeps its initial fields.
    pub dirty: BTreeMap<u16, Record>,
    pub dirty_deleted: Vec<u16>,
    pub last_flush_ms: u64,
}

/// Everything one advance produces, collected across sessions.
#[derive(Default)]
struct Collected {
    replies: Vec<HostRpc>,
    awards: Vec<(u16, Award)>,
}

#[derive(Clone, Copy)]
struct Target {
    participant: u16,
    car: u16,
}

/// One live session's identity: its participant, program and ghost.
#[derive(Clone, Copy)]
struct Scope {
    participant: u16,
    which: Which,
    ghost: u16,
}

fn packed(records: Vec<Record>, deleted: Vec<u16>) -> Section {
    Section {
        float_bits: None,
        flag: false,
        deleted,
        setup: (!records.is_empty()).then_some(Setup::Packed {
            tag: None,
            width: None,
            axes: [None; 3],
        }),
        records,
    }
}

/// Log the failing stage and map it to the listener's replication error.
fn fail<E: std::fmt::Debug>(stage: &'static str) -> impl Fn(E) -> replication::Error {
    move |error| {
        tracing::warn!(stage, ?error, "scoring fault");
        replication::Error::Shape
    }
}

fn idle_inputs(persona: u64, now_ms: u64) -> Inputs {
    Inputs {
        participant: persona,
        vehicle: None,
        others: Vec::new(),
        road: None,
        now: now_ms as f64 / 1000.0,
        time_scale: TIME_SCALE,
    }
}

/// Merge a record into the coalesced set: one record per object, later field
/// changes replacing earlier ones, a creation keeping its initial fields.
fn merge_record(
    dirty: &mut BTreeMap<u16, Record>,
    record: Record,
) -> Result<(), replication::Error> {
    match dirty.get_mut(&record.id) {
        None => {
            if dirty.len() >= MAX_DIRTY {
                return Err(replication::Error::Bound);
            }
            dirty.insert(record.id, record);
            Ok(())
        }
        Some(existing) => match (&mut existing.update, record.update) {
            (
                replication::Update::Entity { fields, .. },
                replication::Update::Entity {
                    fields: changed, ..
                },
            ) => {
                for (slot, change) in fields.iter_mut().zip(changed) {
                    if change.is_some() {
                        *slot = change;
                    }
                }
                Ok(())
            }
            _ => Err(replication::Error::TypeMismatch),
        },
    }
}

impl PlayerListener {
    /// `session_micros` is the host's wall clock at world start; credit batch
    /// ids are unique per session and sequence so a repeated id only ever
    /// means a retry of the same credit.
    pub(in crate::net) fn with_scoring(
        mut self,
        scoring: Option<Arc<Content>>,
        session_micros: u64,
    ) -> Self {
        self.scoring = scoring;
        self.scoring_state.batch_base = (session_micros & 0x0000_0FFF_FFFF_FFFF) << 20;
        self
    }

    /// Stop scoring for this session after a fault. Created entities stay in
    /// the client world unchanged; no further ticks, reports or credits.
    pub(in crate::net) fn disable_scoring(&mut self) {
        let base = self.scoring_state.batch_base;
        self.scoring = None;
        self.scoring_state = State {
            batch_base: base,
            ..State::default()
        };
    }

    fn scoring_inputs(
        &self,
        players: &Players,
        car: u16,
        now_ms: u64,
    ) -> Result<Inputs, replication::Error> {
        let physics = players
            .vehicle_physics(HOST_SELECTOR as u8, self.persona, car)
            .map_err(fail("vehicle physics"))?;
        let motion = physics.decode([0.; 3]).map_err(fail("physics decode"))?;
        let vehicle =
            Vehicle::from_motion(u32::from(car), &motion).map_err(fail("vehicle basis"))?;
        Ok(Inputs {
            vehicle: Some(vehicle),
            ..idle_inputs(self.persona, now_ms)
        })
    }

    /// Decode one incoming RPC against the live query bindings. `Some` means
    /// the call belongs to scoring and is queued for the next advance.
    pub(super) fn scoring_report(
        &self,
        players: &Players,
        body: nfs_protocol::world::BitSpan<'_>,
    ) -> Result<Option<QueuedReport>, replication::Error> {
        for (&participant, live) in &self.scoring_state.live {
            for (which, entity) in Which::ALL.into_iter().zip(&live.entities) {
                for (node, binding) in &entity.reports {
                    let report = binding.receive(
                        players,
                        HOST_SELECTOR as u8,
                        self.persona,
                        body,
                        |id| players.objects().get(id).is_some(),
                    )?;
                    if let Some(report) = report {
                        return Ok(Some(QueuedReport {
                            participant,
                            which,
                            node: *node,
                            report,
                        }));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Apply one session output: property changes go to the world copy and
    /// the coalesced record set, commands become entity calls, awards queue.
    fn collect_output(
        content: &Content,
        players: &mut Players,
        state: &mut State,
        scope: Scope,
        output: Output,
        collected: &mut Collected,
    ) -> Result<(), replication::Error> {
        let Scope {
            participant,
            which,
            ghost,
        } = scope;
        if let Some(update) = content.update(which, &output.synced)?
            && let Some(record) = players.change(ghost, update)?
        {
            merge_record(&mut state.dirty, record)?;
        }
        for (node, command) in output.commands {
            collected.replies.push(HostRpc::Query(
                content.command(which, players, ghost, node, command)?,
            ));
        }
        if !output.remote.is_empty() {
            tracing::debug!(
                participant,
                ?which,
                remote = output.remote.len(),
                "scoring remote events are not encoded yet"
            );
        }
        for award in &output.awards {
            tracing::info!(
                participant,
                ?which,
                keeper = award.keeper,
                amount = award.amount,
                "scoring award"
            );
        }
        collected
            .awards
            .extend(output.awards.into_iter().map(|award| (participant, award)));
        Ok(())
    }

    fn retire(
        &self,
        content: &Content,
        players: &mut Players,
        state: &mut State,
        participant: u16,
        now_ms: u64,
        collected: &mut Collected,
    ) -> Result<(), replication::Error> {
        let live = state
            .live
            .remove(&participant)
            .ok_or(replication::Error::Shape)?;
        let inputs = idle_inputs(self.persona, now_ms);
        let mut ghosts = Vec::with_capacity(2);
        for (which, entity) in Which::ALL.into_iter().zip(live.entities) {
            let program = &content.participant(which).program;
            let root = program
                .nodes
                .iter()
                .position(|n| n.class == "ParticipantLogicEntityData")
                .ok_or(replication::Error::Shape)? as u16;
            let mut session = entity.session;
            let output = session
                .event(root, ROOT_DISABLED, &inputs)
                .map_err(fail("root disable"))?;
            collected
                .awards
                .extend(output.awards.into_iter().map(|award| (participant, award)));
            ghosts.push(entity.ghost);
        }
        players.remove_entities(&ghosts)?;
        for ghost in ghosts {
            state.dirty.remove(&ghost);
            state.dirty_deleted.push(ghost);
        }
        tracing::info!(participant, "owned scoring participants retired");
        Ok(())
    }

    fn create(
        &self,
        content: &Content,
        players: &mut Players,
        state: &mut State,
        target: Target,
        now_ms: u64,
        collected: &mut Collected,
    ) -> Result<(), replication::Error> {
        let Target { participant, car } = target;
        if state.live.len() >= MAX_LIVE {
            return Err(replication::Error::Bound);
        }
        let inputs = self.scoring_inputs(players, car, now_ms)?;
        let mut entities = Vec::with_capacity(2);
        for which in Which::ALL {
            let (session, output) = content
                .start(which, &inputs)
                .map_err(fail("session start"))?;
            let creation = content
                .creation(which, &session, participant, players)
                .map_err(fail("creation record"))?;
            let record = players
                .spawn_entities(vec![creation], content.creation_content())?
                .remove(0);
            let ghost = record.id;
            merge_record(&mut state.dirty, record)?;
            let reports = content.report_bindings(which, players, ghost)?;
            // Startup publications are already in the creation; only its
            // commands and awards remain to queue.
            let startup = Output {
                synced: Vec::new(),
                ..output
            };
            Self::collect_output(
                content,
                players,
                state,
                Scope {
                    participant,
                    which,
                    ghost,
                },
                startup,
                collected,
            )?;
            entities.push(Entity {
                ghost,
                session,
                reports,
            });
        }
        let [acquisition, scoring] =
            <[Entity; 2]>::try_from(entities).map_err(|_| replication::Error::Shape)?;
        state.live.insert(
            participant,
            Live {
                car,
                entities: [acquisition, scoring],
                last_ms: now_ms,
            },
        );
        tracing::info!(participant, car, "owned scoring participants created");
        Ok(())
    }

    /// Queue one section with everything coalesced so far, when the queue
    /// has room and the flush interval has passed.
    fn flush_scoring(&mut self, now_ms: u64) {
        let state = &mut self.scoring_state;
        if state.dirty.is_empty() && state.dirty_deleted.is_empty() {
            return;
        }
        if !self.pending.is_empty()
            || self.pending.len() >= MAX_PENDING
            || now_ms.saturating_sub(state.last_flush_ms) < FLUSH_INTERVAL_MS
        {
            return;
        }
        let records = std::mem::take(&mut state.dirty).into_values().collect();
        let deleted = std::mem::take(&mut state.dirty_deleted);
        state.last_flush_ms = now_ms;
        self.pending.push_back(packed(records, deleted));
    }

    /// Create, tick, report into and retire scoring sessions. Runs between
    /// frames with the committed world; property changes are applied to the
    /// world immediately and replicated by the bounded flush.
    pub(in crate::net) fn advance_scoring(
        &mut self,
        now_ms: u64,
    ) -> Result<(), replication::Error> {
        let Some(content) = self.scoring.clone() else {
            return Ok(());
        };
        if !self.pending_entries.is_empty() {
            return Ok(());
        }
        let active: BTreeSet<u16> = self
            .participants
            .in_free_roam()
            .into_iter()
            .filter(|p| self.free_roam_presented.contains(p))
            .collect();
        let retiring: Vec<u16> = self
            .scoring_state
            .live
            .keys()
            .copied()
            .filter(|p| !active.contains(p))
            .collect();
        let creating: Vec<Target> = active
            .iter()
            .filter(|p| !self.scoring_state.live.contains_key(p))
            .filter_map(|&participant| {
                self.world_cars
                    .iter()
                    .find(|(_, (owner, ready))| *owner == participant && *ready)
                    .map(|(&car, _)| Target { participant, car })
            })
            .collect();
        let idle = retiring.is_empty()
            && creating.is_empty()
            && self.scoring_state.reports.is_empty()
            && self.scoring_state.reported.is_empty();
        if idle {
            self.flush_scoring(now_ms);
            return Ok(());
        }
        let mut state = self.scoring_state.clone();
        let mut players = self.players.clone();
        let mut collected = Collected::default();
        for participant in retiring {
            self.retire(
                &content,
                &mut players,
                &mut state,
                participant,
                now_ms,
                &mut collected,
            )?;
        }
        for target in creating {
            self.create(
                &content,
                &mut players,
                &mut state,
                target,
                now_ms,
                &mut collected,
            )?;
        }
        // Deliver queued reports, then tick every live session whose car
        // reported movement since the last advance, in fixed engine steps.
        let reports = std::mem::take(&mut state.reports);
        for queued in reports {
            let Some(live) = state.live.get(&queued.participant) else {
                continue;
            };
            let inputs = self.scoring_inputs(&players, live.car, now_ms)?;
            let (output, ghost) = {
                let live = state
                    .live
                    .get_mut(&queued.participant)
                    .ok_or(replication::Error::Shape)?;
                let entity = &mut live.entities[queued.which as usize];
                let output = entity
                    .session
                    .report(queued.node, &queued.report, &inputs)
                    .map_err(fail("query report"))?;
                (output, entity.ghost)
            };
            Self::collect_output(
                &content,
                &mut players,
                &mut state,
                Scope {
                    participant: queued.participant,
                    which: queued.which,
                    ghost,
                },
                output,
                &mut collected,
            )?;
        }
        let reported = std::mem::take(&mut state.reported);
        let participants: Vec<u16> = state.live.keys().copied().collect();
        for participant in participants {
            let (car, last_ms) = {
                let live = &state.live[&participant];
                (live.car, live.last_ms)
            };
            if !reported.contains(&car) || now_ms <= last_ms {
                continue;
            }
            let covered = ((now_ms - last_ms) as f32 / 1000.0).min(MAX_TICK_SECONDS);
            let mut remaining = covered;
            let mut inputs = self.scoring_inputs(&players, car, now_ms)?;
            let start = inputs.now - f64::from(covered);
            if now_ms / 1000 != last_ms / 1000
                && let Some(vehicle) = &inputs.vehicle
            {
                let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let [right, up, forward, position] = vehicle.transform;
                let v = vehicle.velocity;
                tracing::info!(
                    participant,
                    car,
                    speed = dot(v, v).sqrt(),
                    forward = dot(v, forward),
                    lateral = dot(v, right),
                    vertical = dot(v, up),
                    ?position,
                    ?forward,
                    "scoring vehicle sample"
                );
            }
            while remaining > 0.0 {
                let step = remaining.min(TICK_SECONDS);
                remaining -= step;
                inputs.now = start + f64::from(covered - remaining);
                for which in Which::ALL {
                    let (output, ghost) = {
                        let live = state
                            .live
                            .get_mut(&participant)
                            .ok_or(replication::Error::Shape)?;
                        let entity = &mut live.entities[which as usize];
                        let output = entity
                            .session
                            .tick(step, &inputs)
                            .map_err(fail("session tick"))?;
                        (output, entity.ghost)
                    };
                    Self::collect_output(
                        &content,
                        &mut players,
                        &mut state,
                        Scope {
                            participant,
                            which,
                            ghost,
                        },
                        output,
                        &mut collected,
                    )?;
                }
            }
            if let Some(live) = state.live.get_mut(&participant) {
                live.last_ms = now_ms;
            }
        }
        // Bank awards into durable credits, one per participant.
        let thresholds = self
            .logic
            .as_deref()
            .and_then(|l| l.reputation_thresholds())
            .cloned();
        let mut grouped: BTreeMap<u16, Vec<Award>> = BTreeMap::new();
        for (participant, award) in collected.awards {
            grouped.entry(participant).or_default().push(award);
        }
        for (participant, awards) in grouped {
            let Some(credit) = content.credit(&awards)? else {
                continue;
            };
            let Some(thresholds) = thresholds.clone() else {
                tracing::warn!(
                    participant,
                    "scoring credit without reputation thresholds; dropped"
                );
                continue;
            };
            if state.pending_awards.len() >= MAX_PENDING_AWARDS {
                return Err(replication::Error::Bound);
            }
            state.sequence = state
                .sequence
                .checked_add(1)
                .ok_or(replication::Error::Bound)?;
            if state.sequence >= 1 << 20 {
                return Err(replication::Error::Bound);
            }
            let batch_id = state.batch_base | state.sequence;
            tracing::info!(
                participant,
                total = credit.total,
                speed = credit.speed,
                style = credit.style,
                build = credit.build,
                crew = credit.crew,
                outlaw = credit.outlaw,
                batch_id,
                "owned scoring credit banked"
            );
            state.pending_awards.push_back(AwardRequest {
                participant,
                credit,
                batch_id,
                thresholds,
            });
        }
        if collected.replies.len()
            > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len())
        {
            return Err(replication::Error::Bound);
        }
        self.pending_rpcs.extend(collected.replies);
        self.players = players;
        self.scoring_state = state;
        self.flush_scoring(now_ms);
        Ok(())
    }

    pub(in crate::net) fn take_award(&mut self) -> Option<AwardRequest> {
        self.scoring_state.pending_awards.pop_front()
    }

    /// Accept a committed credit: adopt the account view, keep the item
    /// collection for later purchases and refresh the presented REP fields.
    /// Field changes join the coalesced scoring flush.
    pub(in crate::net) fn awarded(
        &mut self,
        request: &AwardRequest,
        credited: Credited,
    ) -> Result<(), replication::Error> {
        if let Some(before) = &self.persistent
            && credited.loaded.generation() < before.generation()
        {
            return Err(replication::Error::Shape);
        }
        let scores = nfs_services::reputation::Scores::load(&credited.loaded)
            .map_err(|_| replication::Error::Shape)?;
        let values = request.thresholds.evaluate(scores);
        let mut players = self.players.clone();
        let mut dirty = self.scoring_state.dirty.clone();
        let mut changed = 0usize;
        if let Some(logic) = self.logic.as_deref() {
            for (participant, ghosts) in &self.logic_ghosts {
                if !self
                    .players
                    .owns_participant(HOST_SELECTOR as u8, self.persona, *participant)
                {
                    continue;
                }
                for (capture, index, value) in crate::scoring::reputation_updates(logic, &values) {
                    let Some(&ghost) = ghosts.get(&capture) else {
                        continue;
                    };
                    let replication::Update::Entity { binding, .. } = &players
                        .objects()
                        .get(ghost)
                        .ok_or(replication::Error::UnknownObject)?
                        .current
                    else {
                        continue;
                    };
                    let binding = binding.clone();
                    let kinds = binding.profile()?.kinds().to_vec();
                    if kinds.get(index) != Some(&nfs_world::replication::entity::Kind::I32Property)
                    {
                        return Err(replication::Error::TypeMismatch);
                    }
                    let mut fields = vec![None; kinds.len()];
                    fields[index] = Some(nfs_world::replication::entity::Update::I32Property(
                        Some(value),
                    ));
                    if let Some(record) =
                        players.change(ghost, replication::Update::Entity { binding, fields })?
                    {
                        merge_record(&mut dirty, record)?;
                        changed += 1;
                    }
                }
            }
        }
        if let Some(inventory) = &mut self.inventory
            && inventory.items.items.len() <= credited.items.items.len()
        {
            inventory.items = credited.items;
        }
        self.persistent = Some(credited.loaded);
        tracing::info!(
            participant = request.participant,
            batch_id = request.batch_id,
            replayed = credited.replayed,
            total = scores.total,
            level = values.get(nfs_services::reputation::Field::Level),
            fields = changed,
            "owned reputation credit committed"
        );
        self.scoring_state.dirty = dirty;
        self.players = players;
        Ok(())
    }
}
