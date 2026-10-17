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
use nfs_world::items::Derived;
use nfs_world::nitrous_fuel::{Buildup, Fuel};
use nfs_world::scoring::nos::Sample as NitrousSample;
use nfs_world::{
    logic::{EntityRef, Fire},
    participants::HostRpc,
    replication::{Record, section::Setup},
    scoring::events::RemoteEmission,
    scoring::{
        interpreter::{Award, Inputs, Output, Session, Vehicle},
        reports::{Binding as ReportBinding, Report},
        stats::Stats,
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
/// Other cars supplied to the queries per tick.
const MAX_OTHERS: usize = 64;

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
    /// The participant's runtime player index, the context of the inventory
    /// notifications that announce the credit's unlocks.
    pub player_index: Option<u32>,
}

/// A committed credit: the refreshed account view and item collection.
#[derive(Debug)]
pub(in crate::net) struct Credited {
    pub loaded: Arc<crate::persistent::Loaded>,
    pub items: Arc<nfs_world::items::Collection>,
    pub replayed: bool,
    /// Encoded Items notifications for what the credit changed, in order.
    pub notifications: Vec<Vec<u8>>,
    /// Changed roots no notification describes; the client sees them at the
    /// next inventory load.
    pub unannounced: usize,
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
    /// Per-car free-fall detectors for the owned airborne measurement.
    pub free_fall: BTreeMap<u16, FreeFall>,
    /// Log every synchronized change (`NFS_SCORING_TRACE`), not only flags.
    pub trace: bool,
    /// Owned nitrous fuel per world car (E920), ticked with the car's reports.
    pub nitrous: BTreeMap<u16, Nitrous>,
}

/// One car's nitrous fuel and the world time it was last advanced to.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::net) struct Nitrous {
    pub fuel: Fuel,
    pub buildup: Buildup,
    pub transitions: VecDeque<NitrousSample>,
}
impl Nitrous {
    pub fn sample(&self) -> NitrousSample {
        NitrousSample {
            engaged: self.fuel.engaged(),
            level: self.fuel.fraction(),
            capacity: self.fuel.capacity(),
        }
    }
    pub fn request(&mut self, active: bool) {
        if active != self.fuel.engaged() && self.transitions.len() < MAX_QUEUED_REPORTS {
            self.fuel.request(active);
            self.transitions.push_back(self.sample());
        }
    }
}

/// Owned airborne estimate (E916, inferred): the replicated chassis carries no
/// wheel-contact flag, so a car is taken as airborne while its vertical
/// velocity has fallen at about gravity for at least four consecutive
/// estimates (about 0.4 s at the 10 Hz report spacing) and has lost at least
/// [`Self::MIN_DROP`] since the run began. Crests reach two or three
/// near-gravity estimates while losing little speed (E921: two crests of run
/// e911v, runs of two and three, 2.5 m/s lost); a landing resets the run.
#[derive(Clone, Debug, Default)]
pub(super) struct FreeFall {
    /// Recent reports: chassis clock (seconds, from the report's identity
    /// words) and vertical velocity. The client clock keeps the acceleration
    /// estimate free of server arrival jitter (E918); the window averages the
    /// 1/32 m/s velocity quantization over about a tenth of a second (E919).
    recent: std::collections::VecDeque<(f64, f32)>,
    run: u8,
    /// Vertical velocity at the base of the run's first estimate.
    run_start: f32,
}
impl FreeFall {
    const MIN_RUN: u8 = 4;
    /// Vertical velocity lost over the run before the car counts as airborne, m/s.
    const MIN_DROP: f32 = 3.0;
    /// Shortest span the estimate is taken over, seconds.
    const SPAN: f64 = 0.09;
    const WINDOW: usize = 8;
    /// Vertical acceleration of a fall, m/s²: gravity with the quantization
    /// noise; the suspension's kick at a crest's lip reaches -14 (E921).
    const BAND: std::ops::RangeInclusive<f32> = -13.0..=-6.5;
    pub fn observe(&mut self, clock: f64, vertical: f32) -> bool {
        let mut acceleration = None;
        if self
            .recent
            .back()
            .is_some_and(|&(last, _)| clock <= last || clock - last > 0.5)
        {
            self.recent.clear();
            self.run = 0;
        }
        self.recent.push_back((clock, vertical));
        while self.recent.len() > Self::WINDOW {
            self.recent.pop_front();
        }
        // The oldest report at least SPAN before this one.
        let base = self
            .recent
            .iter()
            .rev()
            .find(|&&(at, _)| clock - at >= Self::SPAN)
            .copied();
        if let Some((at, old)) = base {
            let estimate = (vertical - old) / (clock - at) as f32;
            acceleration = Some(estimate);
            if Self::BAND.contains(&estimate) {
                if self.run == 0 {
                    self.run_start = old;
                }
                self.run = self.run.saturating_add(1);
            } else {
                self.run = 0;
            }
        }
        let airborne = self.run >= Self::MIN_RUN && self.run_start - vertical >= Self::MIN_DROP;
        // Diagnostic for the airtime calibration (E919): every report with a
        // notable vertical speed or a fall in progress.
        if vertical.abs() > 1.5 || self.run > 0 {
            tracing::info!(
                clock,
                vertical,
                acceleration,
                run = self.run,
                drop = self.run_start - vertical,
                airborne,
                window = self.recent.len(),
                "free-fall sample"
            );
        }
        airborne
    }
}

#[cfg(test)]
mod flush_tests {
    use super::super::PlayerListener;
    use nfs_world::replication::{Record, Update, player_view};

    fn record(id: u16) -> Record {
        Record {
            id,
            initial: None,
            update: Update::PlayerView(player_view::Update {
                base: None,
                view: None,
            }),
        }
    }

    #[test]
    fn periodic_flush_waits_for_the_session_queue_and_a_fired_event_does_not() {
        let mut listener = PlayerListener::new(806_588_082);
        listener.scoring_state.dirty.insert(7, record(7));
        listener.scoring_state.last_flush_ms = 0;
        // Interval passed, listener queue empty, but the session still holds
        // a section: the periodic flush waits and the change stays merged.
        listener.session_backlog = 1;
        listener.flush_scoring(1_000, false);
        assert!(listener.pending.is_empty());
        assert_eq!(listener.scoring_state.dirty.len(), 1);
        // A fired graph event flushes behind the queued sections anyway.
        listener.flush_scoring(1_000, true);
        assert_eq!(listener.pending.len(), 1);
        assert!(listener.scoring_state.dirty.is_empty());
        // Structure never waits for the session queue: a deletion (like a
        // creation) flushes at once, so no host call overtakes its object.
        listener.pending.clear();
        listener.scoring_state.dirty_deleted.push(9);
        listener.session_backlog = 3;
        listener.flush_scoring(1_000, false);
        assert_eq!(listener.pending.len(), 1);
        assert_eq!(listener.pending[0].deleted, vec![9]);
        // With the session queue empty again the periodic flush resumes.
        listener.pending.clear();
        listener.scoring_state.dirty.insert(8, record(8));
        listener.session_backlog = 0;
        listener.flush_scoring(1_050, false);
        assert!(listener.pending.is_empty(), "interval not yet passed");
        listener.flush_scoring(1_100, false);
        assert_eq!(listener.pending.len(), 1);
        assert_eq!(listener.pending[0].records.len(), 1);
        assert_eq!(listener.pending[0].records[0].id, 8);
    }
}

#[cfg(test)]
mod nitrous_tests {
    use super::*;
    use nfs_world::nitrous_fuel::{Boosts, Configuration, Curve};
    #[test]
    fn brief_taps_and_repeated_requests_preserve_transition_order() {
        let config = Arc::new(
            Configuration::new(
                8.0,
                Curve::new(vec![[0.0, 1.0], [2.0, 1.0]]).unwrap(),
                Curve::new(vec![[0.0, 0.0], [2.0, 0.0]]).unwrap(),
                Boosts::default(),
            )
            .unwrap(),
        );
        let mut n = Nitrous {
            fuel: Fuel::full(config, 5).unwrap(),
            buildup: Buildup::default(),
            transitions: VecDeque::new(),
        };
        n.request(true);
        n.request(true);
        n.fuel.tick(0.5, Buildup::default()).unwrap();
        n.request(false);
        n.request(false);
        assert_eq!(n.transitions.len(), 2);
        let start = n.transitions.pop_front().unwrap();
        let stop = n.transitions.pop_front().unwrap();
        assert!(start.engaged);
        assert!(!stop.engaged);
        assert_eq!(start.level, 1.0);
        assert_eq!(stop.level, 7.5 / 8.0);
        let mut query = nfs_world::scoring::nos::Node::default();
        assert!(
            query
                .step(start)
                .unwrap()
                .contains(&nfs_world::scoring::nos::Effect::Emit(
                    nfs_world::scoring::nos::ENGAGED
                ))
        );
        assert!(
            query
                .step(stop)
                .unwrap()
                .contains(&nfs_world::scoring::nos::Effect::Emit(
                    nfs_world::scoring::nos::DISENGAGED
                ))
        );
    }
}

#[cfg(test)]
mod free_fall_tests {
    use super::FreeFall;
    #[test]
    fn a_jump_counts_after_four_estimates_and_a_crest_never_does() {
        // Launch at 6 m/s up, gravity thereafter, reports every 0.1 s.
        let mut fall = FreeFall::default();
        let mut flagged_at = None;
        for step in 0..12 {
            let clock = 10.0 + 0.1 * f64::from(step);
            let vertical = 6.0 - 0.981 * step as f32;
            if fall.observe(clock, vertical) && flagged_at.is_none() {
                flagged_at = Some(step);
            }
        }
        assert_eq!(flagged_at, Some(4));
        // A crest of run e911v (E921, 14:22:22): three near-gravity estimates
        // while the road levels out, then a flat road.
        let mut fall = FreeFall::default();
        let crest = [
            (179.133, 1.71875),
            (179.267, 2.375),
            (179.400, 3.09375),
            (179.532, 3.5625),
            (179.666, 3.90625),
            (179.801, 4.4375),
            (179.900, 3.84375),
            (180.033, 2.34375),
            (180.167, 1.375),
            (180.267, 1.0),
            (180.367, 0.9),
        ];
        assert!(
            crest
                .iter()
                .all(|&(clock, vertical)| !fall.observe(clock, vertical))
        );
    }
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

/// Entity event messages for one remote emission: the official host fires
/// registry 86 on the participant ghost with the authored row index as the
/// event and the row's networked bus ordinal as the entity (E914: style row
/// 27 on entity 3, speed 37 on 5, outlaw 57 on 12). Rows on buses without
/// a client entity are counted, not fabricated.
fn remote_fires(
    bus_entities: &[Option<u16>],
    ghost: u16,
    emission: &RemoteEmission,
) -> (Vec<Fire>, usize) {
    let mut fires = Vec::with_capacity(emission.connections.len());
    let mut skipped = 0;
    for remote in &emission.connections {
        match bus_entities.get(usize::from(remote.bus)).copied().flatten() {
            Some(entity) => fires.push(Fire {
                event: u32::from(remote.index),
                player: None,
                target: EntityRef {
                    ghost,
                    entity: u32::from(entity),
                },
            }),
            None => skipped += 1,
        }
    }
    (fires, skipped)
}

/// Every other replicated car with decodable chassis physics, for the draft,
/// proximity and speed-match queries. Other participants' cars are
/// replicated into this world; traffic is client-side and absent here.
fn other_vehicles(players: &Players, own: u16) -> Vec<nfs_world::scoring::draft::Vehicle> {
    let mut others = Vec::new();
    for (id, object) in players.objects().iter() {
        if id == own || others.len() >= MAX_OTHERS {
            continue;
        }
        let replication::Update::Vehicle { fields, .. } = &object.current else {
            continue;
        };
        let physics = fields.iter().find_map(|field| match field {
            Some(nfs_world::replication::vehicle::Update::Chassis(c)) => c.physics.as_ref(),
            _ => None,
        });
        let Some(motion) = physics.and_then(|p| p.decode([0.; 3]).ok()) else {
            continue;
        };
        others.push(nfs_world::scoring::draft::Vehicle {
            identity: u32::from(id),
            position: motion.position,
            velocity: motion.velocity,
        });
    }
    others
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
        stats: None,
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
            (
                replication::Update::Vehicle { fields, .. },
                replication::Update::Vehicle {
                    fields: changed, ..
                },
            ) => {
                // A car's later field change replaces the earlier one; the
                // nitrous index keeps whichever half the later change omits.
                for (slot, change) in fields.iter_mut().zip(changed) {
                    match (slot.as_mut(), change) {
                        (_, None) => {}
                        (
                            Some(nfs_world::replication::vehicle::Update::Index { index, value }),
                            Some(nfs_world::replication::vehicle::Update::Index {
                                index: new_index,
                                value: new_value,
                            }),
                        ) => {
                            if new_index.is_some() {
                                *index = new_index;
                            }
                            if new_value.is_some() {
                                *value = new_value;
                            }
                        }
                        (_, change) => *slot = change,
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
        content: &Content,
        players: &Players,
        car: u16,
        now_ms: u64,
        nitrous: Option<&Nitrous>,
    ) -> Result<Inputs, replication::Error> {
        let physics = players
            .vehicle_physics(HOST_SELECTOR as u8, self.persona, car)
            .map_err(fail("vehicle physics"))?;
        let motion = physics.decode([0.; 3]).map_err(fail("physics decode"))?;
        let mut vehicle =
            Vehicle::from_motion(u32::from(car), &motion).map_err(fail("vehicle basis"))?;
        vehicle.nos = nitrous.map(Nitrous::sample);
        // Road measurements from the installed network (E916); absent without
        // a network or off the network, which preserves the query state.
        let road = match content.road() {
            Some(network) => network
                .sample(motion.position, motion.velocity)
                .map_err(fail("road sample"))?,
            None => None,
        };
        Ok(Inputs {
            vehicle: Some(vehicle),
            road,
            others: other_vehicles(players, car),
            stats: self.vehicle_stats(content, players, car),
            ..idle_inputs(self.persona, now_ms)
        })
    }

    /// Installed fuel configuration and the persisted handling setting.
    fn nitrous_fuel(&self, content: &Content, players: &Players, car: u16) -> Option<Fuel> {
        let items = &self.inventory.as_ref()?.items;
        let vehicle = players.vehicle_item_of(HOST_SELECTOR as u8, self.persona, car)?;
        // The installed nitrous is the one the car replicates (a car may own
        // several nitrous items; E921).
        let guid = self
            .vehicles
            .as_deref()?
            .vehicles
            .nitrous_definition(items, vehicle)?;
        let setting = self
            .vehicles
            .as_deref()?
            .vehicles
            .nitrous_setting(items, vehicle)?;
        Fuel::full(content.nitrous(&guid)?, setting).ok()
    }

    /// Current and stock figures of the participant's car (E916): the dyno
    /// cache the client stores on the car item and the item definition's
    /// defaults. Absent without the item catalog, the inventory or a car item.
    fn vehicle_stats(&self, content: &Content, players: &Players, car: u16) -> Option<Stats> {
        let catalog = content.items()?;
        let items = &self.inventory.as_ref()?.items;
        let item =
            items
                .items
                .get(&players.vehicle_item_of(HOST_SELECTOR as u8, self.persona, car)?)?;
        let Derived::RaceVehicle {
            zero_to_sixty,
            top_speed_mph,
            ..
        } = &item.derived
        else {
            return None;
        };
        let Some(Derived::RaceVehicle {
            zero_to_sixty: stock_zero_to_sixty,
            top_speed_mph: stock_top_speed_mph,
            ..
        }) = catalog
            .definition(&item.definition)
            .ok()?
            .default_derived
            .as_ref()
        else {
            return None;
        };
        let stats = Stats {
            top_speed_mph: *top_speed_mph,
            stock_top_speed_mph: *stock_top_speed_mph,
            zero_to_sixty: f32::from_bits(*zero_to_sixty),
            stock_zero_to_sixty: f32::from_bits(*stock_zero_to_sixty),
        };
        stats.validate().ok()?;
        Some(stats)
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
                        tracing::info!(
                            participant,
                            ?which,
                            node,
                            ?report,
                            "scoring report received"
                        );
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
        if let Some(live) = state.live.get(&participant)
            && let Some(nitrous) = state.nitrous.get_mut(&live.car)
        {
            nitrous
                .buildup
                .merge(output.buildup)
                .map_err(fail("nitrous buildup"))?;
        }
        // Boolean synchronized changes are the HUD flags (category and
        // sub-event popups); log them to time the popups against the drive.
        // With NFS_SCORING_TRACE set, every synchronized change is logged for
        // the frame-by-frame comparison with the official stream (E920).
        let trace = state.trace;
        for (node, value) in &output.synced {
            if trace || matches!(value, nfs_world::scoring::value::Value::Boolean(_)) {
                let flag = value;
                let identity = content
                    .participant(which)
                    .program
                    .nodes
                    .get(usize::from(*node))
                    .map(|n| (n.identity.asset.as_str(), n.identity.index));
                tracing::info!(participant, ?which, node, ?identity, ?flag, "scoring flag");
            }
        }
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
        for emission in &output.remote {
            let (fires, skipped) =
                remote_fires(&content.participant(which).bus_entities, ghost, emission);
            if skipped > 0 {
                tracing::warn!(
                    participant,
                    ?which,
                    source = ?emission.source,
                    skipped,
                    "scoring remote rows on buses without a client entity were dropped"
                );
            }
            for fire in &fires {
                tracing::info!(
                    participant,
                    ?which,
                    source = ?emission.source,
                    row = fire.event,
                    entity = fire.target.entity,
                    ghost = fire.target.ghost,
                    "scoring remote row fired"
                );
            }
            collected
                .replies
                .extend(fires.into_iter().map(HostRpc::Event));
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
        state.free_fall.remove(&live.car);
        state.nitrous.remove(&live.car);
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
        if let Some(fuel) = self.nitrous_fuel(content, players, car) {
            tracing::info!(
                participant,
                car,
                capacity = fuel.capacity(),
                "owned nitrous fuel seeded"
            );
            state.nitrous.insert(
                car,
                Nitrous {
                    fuel,
                    buildup: Buildup::default(),
                    transitions: VecDeque::new(),
                },
            );
        }
        let inputs = self.scoring_inputs(content, players, car, now_ms, state.nitrous.get(&car))?;
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
    /// has room and the flush interval has passed. A periodic flush also
    /// waits while the world session still holds queued sections, so at most
    /// one scoring section is in flight and later changes keep merging per
    /// entity (E923: flushing whenever the listener's own queue was empty
    /// kept the session queue full and delayed every later section).
    /// `force` flushes ahead of the interval and behind queued sections, so a
    /// fired graph event never overtakes the property values it refers to
    /// (sections drain before RPC notifications; E914 shows the official
    /// popup reads the amount the row refers to at once).
    fn flush_scoring(&mut self, now_ms: u64, force: bool) {
        let state = &mut self.scoring_state;
        if state.dirty.is_empty() && state.dirty_deleted.is_empty() {
            return;
        }
        if self.pending.len() >= MAX_PENDING {
            return;
        }
        // Creations and deletions never wait: host calls on these objects
        // move to the session as soon as the listener queue is empty, and a
        // call ahead of its object's creation is refused (E925: deferring
        // the scoring creation behind the session backlog made the session
        // refuse the scoring calls and close the world connection).
        let structural = !state.dirty_deleted.is_empty()
            || state.dirty.values().any(|record| record.initial.is_some());
        let force = force || structural;
        if !force
            && (!self.pending.is_empty()
                || self.session_backlog > 0
                || now_ms.saturating_sub(state.last_flush_ms) < FLUSH_INTERVAL_MS)
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
            self.flush_scoring(now_ms, false);
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
            let inputs = self.scoring_inputs(
                &content,
                &players,
                live.car,
                now_ms,
                state.nitrous.get(&live.car),
            )?;
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
            let mut inputs =
                self.scoring_inputs(&content, &players, car, now_ms, state.nitrous.get(&car))?;
            if let Some(vehicle) = inputs.vehicle.as_mut()
                && let Ok(physics) = players.vehicle_physics(HOST_SELECTOR as u8, self.persona, car)
            {
                let [high, low, _] = physics.identity_words;
                let clock = f64::from_bits((u64::from(high) << 32) | u64::from(low));
                if clock.is_finite() {
                    let airborne = state
                        .free_fall
                        .entry(car)
                        .or_default()
                        .observe(clock, vehicle.velocity[1]);
                    vehicle.airborne = Some(airborne);
                }
            }
            // Preserve transitions even when start and release arrive between
            // scheduler passes. Component notifications deliver their snapshots
            // without advancing unrelated queries, timers or fuel.
            let transitions = state
                .nitrous
                .get_mut(&car)
                .map(|n| std::mem::take(&mut n.transitions))
                .unwrap_or_default();
            for sample in transitions {
                if let Some(vehicle) = inputs.vehicle.as_mut() {
                    vehicle.nos = Some(sample);
                }
                for which in Which::ALL {
                    let (output, ghost) = {
                        let live = state
                            .live
                            .get_mut(&participant)
                            .ok_or(replication::Error::Shape)?;
                        let entity = &mut live.entities[which as usize];
                        (
                            entity
                                .session
                                .update_nitrous(&inputs)
                                .map_err(fail("nitrous transition"))?,
                            entity.ghost,
                        )
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
                    airborne = vehicle.airborne,
                    stats = ?inputs.stats,
                    others = inputs.others.len(),
                    road = ?inputs.road,
                    controls = ?players
                        .vehicle_physics(HOST_SELECTOR as u8, self.persona, car)
                        .ok()
                        .map(|p| p.controls),
                    "scoring vehicle sample"
                );
            }
            while remaining > 0.0 {
                let step = remaining.min(TICK_SECONDS);
                remaining -= step;
                inputs.now = start + f64::from(covered - remaining);
                if let Some(nitrous) = state.nitrous.get_mut(&car) {
                    let buildup = if nitrous.fuel.engaged() {
                        nitrous.buildup
                    } else {
                        std::mem::take(&mut nitrous.buildup)
                    };
                    nitrous
                        .fuel
                        .tick(step, buildup)
                        .map_err(fail("nitrous fuel"))?;
                    if let Some(vehicle) = inputs.vehicle.as_mut() {
                        vehicle.nos = Some(nitrous.sample());
                    }
                }
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
                player_index: players
                    .runtime_index(HOST_SELECTOR as u8, self.persona, participant)
                    .map(u32::from),
            });
        }
        if collected.replies.len()
            > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len())
        {
            return Err(replication::Error::Bound);
        }
        let fired = collected
            .replies
            .iter()
            .any(|reply| matches!(reply, HostRpc::Event(_)));
        self.players = players;
        self.scoring_state = state;
        self.flush_scoring(now_ms, fired);
        self.pending_rpcs.extend(collected.replies);
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
        // Adopt the refreshed collection (unlock grants join a credit) and keep
        // the garage population's spawn snapshot in step with it: the garage
        // return compares the live inventory with that snapshot and refuses a
        // changed one (E917: every frame carrying the return RPC was refused
        // after a credit had replaced the collection behind the snapshot).
        if let Some(inventory) = &mut self.inventory
            && inventory.items.items.len() <= credited.items.items.len()
            && inventory.items != credited.items
        {
            let refreshed = Inventory {
                slots: inventory.slots,
                items: credited.items,
            };
            let synced = match &mut self.population {
                Some(population) => population
                    .refresh_inventory(
                        &players,
                        HOST_SELECTOR as u8,
                        self.persona,
                        inventory,
                        &refreshed,
                    )
                    .is_ok(),
                None => true,
            };
            if synced {
                *inventory = refreshed;
            } else {
                // Keep the spawn-time collection: the next garage load reads
                // the committed inventory anyway, and the return stays valid.
                tracing::warn!(
                    participant = request.participant,
                    "credited collection not adopted; garage population out of step"
                );
            }
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

#[cfg(test)]
mod remote_tests {
    use super::remote_fires;
    use nfs_world::scoring::events::{Endpoint, Remote, RemoteEmission};

    #[test]
    fn remote_rows_fire_on_the_networked_bus_ordinal_and_others_are_counted() {
        let emission = RemoteEmission {
            source: Endpoint {
                node: 81,
                event: 0x4a75_7ba6,
            },
            connections: vec![
                Remote { bus: 2, index: 27 },
                Remote { bus: 3, index: 5 },
                Remote { bus: 9, index: 37 },
            ],
            context: None,
        };
        let entities = [
            Some(1),
            Some(2),
            Some(3),
            None,
            Some(4),
            None,
            None,
            None,
            None,
            Some(5),
        ];
        let (fires, skipped) = remote_fires(&entities, 598, &emission);
        assert_eq!(skipped, 1);
        assert_eq!(fires.len(), 2);
        assert_eq!(
            (
                fires[0].event,
                fires[0].target.ghost,
                fires[0].target.entity
            ),
            (27, 598, 3)
        );
        assert_eq!((fires[1].event, fires[1].target.entity), (37, 5));
        assert!(fires.iter().all(|f| f.player.is_none()));
        // A bus beyond the table is also counted, never fired.
        let beyond = RemoteEmission {
            connections: vec![Remote { bus: 40, index: 1 }],
            ..emission
        };
        assert_eq!(remote_fires(&entities, 598, &beyond), (Vec::new(), 1));
    }
}
