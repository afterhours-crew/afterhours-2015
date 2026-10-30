// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Live scoring for owned participants: the two installed driving
//! participants are created when a participant drives in free roam, ticked
//! from its car's accepted movement, fed its decoded query reports, and
//! retired with the root disable event when it leaves free roam. An owned
//! race event keeps data acquisition, retires the free-roam driving-style
//! participant and runs the event's on the entity the race creates (E994:
//! ghost 1355 stays, 1354 goes at the start, 1605 lives from the race start
//! to the results; #138). Keeper awards become one category credit per bank,
//! committed by the storage exchange; the committed snapshot refreshes the
//! REP presentation fields.
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
    pub which: Which,
    pub ghost: u16,
    pub session: Session,
    pub reports: Vec<(u16, ReportBinding)>,
}

/// Driving participants running for one participant.
const MAX_ENTITIES: usize = 2;

#[derive(Clone, Debug)]
pub(super) struct Live {
    pub car: u16,
    /// Data acquisition, then the free-roam or the event driving-style
    /// participant.
    pub entities: Vec<Entity>,
    pub last_ms: u64,
}
impl Live {
    fn entity_mut(&mut self, which: Which) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.which == which)
    }
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
    /// An owned event completion paid with this credit: its cash, earned-cash
    /// statistic and race-event record join the same transaction.
    pub event: Option<nfs_services::event_rewards::Completion>,
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
    /// Log every synchronized change (`NFS_SCORING_TRACE`), not only flags.
    pub trace: bool,
    /// Owned nitrous fuel per world car (E920), ticked with the car's reports.
    pub nitrous: BTreeMap<u16, Nitrous>,
    /// Banked credits whose commit is outstanding. Their driving-style
    /// records and calls wait for it, so the REP total the client reads
    /// when it builds the REP ring arrives with the banked amount: official
    /// E1026 sends the award event, the driving-style update and the REP
    /// update in one frame. Client `ClientWYWScoreDisplayEntity` (handler
    /// RVA 0x044149d0, queue builder 0x044173f0) starts the ring at
    /// `TotalRep - RepRewarded`; an older total below the amount makes the
    /// first segment endless and the ring stays at level 0 (E1248).
    pub held: BTreeSet<u64>,
    pub held_rpcs: VecDeque<HostRpc>,
    pub held_since_ms: u64,
}

/// A commit that has not answered by then releases its held output.
const HOLD_LIMIT_MS: u64 = 2_000;

/// One car's nitrous fuel and the world time it was last advanced to.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::net) struct Nitrous {
    pub fuel: Fuel,
    pub buildup: Buildup,
    pub transitions: VecDeque<NitrousSample>,
    pub last_ms: f64,
    pub clock: Option<(f64, u64)>,
}
impl Nitrous {
    pub fn sample(&self) -> NitrousSample {
        NitrousSample {
            engaged: self.fuel.engaged(),
            level: self.fuel.fraction(),
            capacity: self.fuel.capacity(),
        }
    }
    pub fn observe_clock(&mut self, clock: f64, now_ms: u64) {
        if clock.is_finite() && clock >= 0.0 {
            self.clock = Some((clock, now_ms));
        }
    }
    pub fn advance_to(&mut self, now_ms: f64) -> Result<(), nfs_world::scoring::Error> {
        if !now_ms.is_finite() {
            return Err(nfs_world::scoring::Error::NonFinite);
        }
        if now_ms <= self.last_ms {
            return Ok(());
        }
        let mut remaining =
            ((now_ms - self.last_ms) / 1000.0).min(f64::from(MAX_TICK_SECONDS)) as f32;
        let mut fuel = self.fuel.clone();
        let mut buildup = self.buildup;
        while remaining > 0.0 {
            let step = remaining.min(TICK_SECONDS);
            fuel.tick(step, buildup)?;
            if !fuel.engaged() {
                buildup.near_misses = 0;
                buildup.cop_slams = 0;
            }
            remaining -= step;
        }
        self.fuel = fuel;
        self.last_ms = now_ms;
        if !self.fuel.engaged() {
            self.buildup = Buildup::default();
        }
        Ok(())
    }
    pub fn request(
        &mut self,
        active: bool,
        client_seconds: f32,
        now_ms: u64,
    ) -> Result<(), nfs_world::scoring::Error> {
        if !client_seconds.is_finite() || client_seconds < 0.0 {
            return Err(nfs_world::scoring::Error::Bound);
        }
        if active == self.fuel.engaged() {
            return Ok(());
        }
        if self.transitions.len() >= MAX_QUEUED_REPORTS {
            return Err(nfs_world::scoring::Error::Bound);
        }
        let mut next = self.clone();
        next.advance_to(now_ms as f64)?;
        // Anchor the game clock to this car's last accepted chassis report.
        // An old/missing anchor uses arrival time only. Compensation is bounded
        // independently of arbitrary finite timestamps in a client request.
        let late = next
            .clock
            .filter(|&(_, at)| now_ms >= at && now_ms - at <= 500)
            .map(|(clock, at)| {
                ((clock + (now_ms - at) as f64 / 1000.0) as f32 - client_seconds)
                    .clamp(0.0, MAX_TICK_SECONDS)
            })
            .unwrap_or(0.0);
        if let Some(level) = next.fuel.request_late(active, late)? {
            next.transitions.push_back(NitrousSample {
                engaged: active,
                level,
                capacity: next.fuel.capacity(),
            });
        }
        *self = next;
        Ok(())
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

    fn fire() -> nfs_world::participants::HostRpc {
        nfs_world::participants::HostRpc::Event(nfs_world::logic::Fire {
            event: 3,
            player: None,
            target: nfs_world::logic::EntityRef {
                ghost: 337,
                entity: 1,
            },
        })
    }

    /// E1248: the REP ring stayed at level 0 because the banked amount
    /// arrived 190-290 ms before the REP total. A bank's records and calls
    /// now wait for its commit and leave with the committed REP fields.
    #[test]
    fn a_banked_credit_holds_its_output_until_the_commit_releases_it() {
        let mut listener = PlayerListener::new(806_588_082);
        listener.scoring_state.held.insert(5);
        listener.scoring_state.held_since_ms = 1_000;
        listener.scoring_state.dirty.insert(7, record(7));
        listener.scoring_state.held_rpcs.push_back(fire());
        // Neither the interval nor a fired event flushes a held bank.
        listener.flush_scoring(1_500, true);
        assert!(listener.pending.is_empty());
        assert!(listener.pending_rpcs.is_empty());
        // Another batch's answer does not release this one.
        listener.release_award(4);
        assert!(listener.pending.is_empty());
        // The commit merges the REP record, then both leave together, the
        // records ahead of the calls.
        listener.scoring_state.dirty.insert(9, record(9));
        listener.release_award(5);
        assert_eq!(listener.pending.len(), 1);
        let ids: Vec<u16> = listener.pending[0].records.iter().map(|r| r.id).collect();
        assert_eq!(ids, [7, 9]);
        assert_eq!(listener.pending_rpcs.len(), 1);
        assert!(listener.scoring_state.held_rpcs.is_empty());
        // A repeated answer is a no-op.
        listener.release_award(5);
        assert_eq!(listener.pending.len(), 1);
        assert_eq!(listener.pending_rpcs.len(), 1);
    }

    #[test]
    fn structure_flushes_through_a_hold_and_an_unanswered_commit_times_out() {
        let mut listener = PlayerListener::new(806_588_082);
        listener.scoring_state.held.insert(6);
        listener.scoring_state.held_since_ms = 2_000;
        listener.scoring_state.dirty_deleted.push(11);
        listener.flush_scoring(2_100, false);
        assert_eq!(listener.pending.len(), 1, "a deletion never waits");
        listener.pending.clear();
        listener.scoring_state.dirty.insert(8, record(8));
        listener.scoring_state.held_rpcs.push_back(fire());
        listener.flush_scoring(3_999, true);
        assert!(listener.pending.is_empty());
        listener.flush_scoring(2_000 + super::HOLD_LIMIT_MS, false);
        assert_eq!(listener.pending.len(), 1);
        assert_eq!(listener.pending_rpcs.len(), 1);
        assert!(listener.scoring_state.held.is_empty());
        // The late answer finds nothing held.
        listener.release_award(6);
        assert_eq!(listener.pending.len(), 1);
    }
}

#[cfg(test)]
mod nitrous_tests {
    use super::*;
    use nfs_world::nitrous_fuel::{Boosts, Configuration, Curve};
    fn tank() -> Nitrous {
        let config = Arc::new(
            Configuration::new(
                8.0,
                Curve::new(vec![[0.0, 1.0], [2.0, 1.0]]).unwrap(),
                Curve::new(vec![[0.0, 0.0], [2.0, 0.0]]).unwrap(),
                Boosts::default(),
            )
            .unwrap(),
        );
        Nitrous {
            fuel: Fuel::full(config, 5).unwrap(),
            buildup: Buildup::default(),
            transitions: VecDeque::new(),
            last_ms: 0.0,
            clock: None,
        }
    }
    #[test]
    fn brief_taps_and_repeated_requests_preserve_transition_order() {
        let mut n = tank();
        n.request(true, 0.0, 0).unwrap();
        n.request(true, 0.0, 0).unwrap();
        n.request(false, 0.5, 500).unwrap();
        n.request(false, 0.5, 500).unwrap();
        assert_eq!(n.transitions.len(), 2);
        let start = n.transitions.pop_front().unwrap();
        let stop = n.transitions.pop_front().unwrap();
        assert!(start.engaged);
        assert!(!stop.engaged);
        assert_eq!(start.level, 1.0);
        assert!((stop.level - 7.5 / 8.0).abs() < 1e-6);
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

    #[test]
    fn same_batch_tap_uses_client_clock_and_samples_before_refund() {
        let mut n = tank();
        n.observe_clock(10.5, 500);
        n.request(true, 10.25, 500).unwrap();
        n.request(false, 10.375, 500).unwrap();
        assert_eq!(n.transitions.len(), 2);
        assert_eq!(n.transitions[0].level, 1.0);
        assert_eq!(n.transitions[1].level, 7.75 / 8.0);
        assert_eq!(n.fuel.fraction(), 7.875 / 8.0);
        let before = n.clone();
        n.advance_to(400.0).unwrap();
        n.advance_to(500.0).unwrap();
        assert_eq!(n, before, "scheduler must not burn the interval again");
    }

    #[test]
    fn missing_clock_counts_arrival_time_and_overflow_preserves_state() {
        let mut n = tank();
        n.request(true, 100.0, 100).unwrap();
        n.request(false, 100.1, 200).unwrap();
        assert!((n.transitions[1].level - 7.9 / 8.0).abs() < 1e-6);
        for i in 2..MAX_QUEUED_REPORTS {
            n.request(i % 2 == 0, 100.1, 200).unwrap();
        }
        let before = n.clone();
        assert!(n.request(true, 100.2, 300).is_err());
        assert_eq!(n, before);
        assert!(n.request(true, f32::NAN, 300).is_err());
        assert_eq!(n, before);
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

/// What one participant in free roam runs: both driving participants, or
/// during an owned event data acquisition plus the event driving-style
/// participant on the race's logic once that exists.
fn kinds(in_event: bool, race: Option<u16>) -> Vec<(Which, Option<u16>)> {
    let mut kinds = vec![(Which::Acquisition, None)];
    match (in_event, race) {
        (false, _) => kinds.push((Which::Scoring, None)),
        (true, Some(ghost)) => kinds.push((Which::Event, Some(ghost))),
        (true, None) => {}
    }
    kinds
}

/// Kinds to retire (running but not wanted, or on another entity) and kinds
/// to start (wanted but not running), in participant and kind order.
#[allow(clippy::type_complexity)]
fn plan(
    desired: &BTreeMap<u16, Vec<(Which, Option<u16>)>>,
    running: &BTreeMap<u16, Vec<(Which, u16)>>,
) -> (Vec<(u16, Which)>, Vec<(u16, Which, Option<u16>)>) {
    let same = |(which, ghost): (Which, Option<u16>), (kind, entity): (Which, u16)| {
        which == kind && ghost.is_none_or(|g| g == entity)
    };
    let mut retiring = Vec::new();
    for (&participant, entities) in running {
        for &entity in entities {
            if !desired
                .get(&participant)
                .is_some_and(|kinds| kinds.iter().any(|&k| same(k, entity)))
            {
                retiring.push((participant, entity.0));
            }
        }
    }
    let mut starting = Vec::new();
    for (&participant, wanted) in desired {
        for &(which, ghost) in wanted {
            if !running
                .get(&participant)
                .is_some_and(|entities| entities.iter().any(|&e| same((which, ghost), e)))
            {
                starting.push((participant, which, ghost));
            }
        }
    }
    (retiring, starting)
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    #[test]
    fn an_event_keeps_acquisition_and_swaps_the_driving_style_participant() {
        let free = BTreeMap::from([(4, kinds(false, None))]);
        // Free roam starts acquisition, then the free-roam participant.
        assert_eq!(
            plan(&free, &BTreeMap::new()),
            (
                vec![],
                vec![(4, Which::Acquisition, None), (4, Which::Scoring, None)]
            )
        );
        let roaming = BTreeMap::from([(4, vec![(Which::Acquisition, 10), (Which::Scoring, 11)])]);
        assert_eq!(plan(&free, &roaming), (vec![], vec![]));
        // The event start retires only the free-roam participant (E994: 1354
        // goes, 1355 stays).
        let waiting = BTreeMap::from([(4, kinds(true, None))]);
        assert_eq!(
            plan(&waiting, &roaming),
            (vec![(4, Which::Scoring)], vec![])
        );
        // The race start runs the event participant on the race's logic.
        let acquiring = BTreeMap::from([(4, vec![(Which::Acquisition, 10)])]);
        let racing = BTreeMap::from([(4, kinds(true, Some(20)))]);
        assert_eq!(
            plan(&racing, &acquiring),
            (vec![], vec![(4, Which::Event, Some(20))])
        );
        // A new race logic (restart) replaces the session; the results retire it.
        let raced = BTreeMap::from([(4, vec![(Which::Acquisition, 10), (Which::Event, 20)])]);
        assert_eq!(plan(&racing, &raced), (vec![], vec![]));
        let again = BTreeMap::from([(4, kinds(true, Some(21)))]);
        assert_eq!(
            plan(&again, &raced),
            (vec![(4, Which::Event)], vec![(4, Which::Event, Some(21))])
        );
        assert_eq!(plan(&waiting, &raced), (vec![(4, Which::Event)], vec![]));
        // The free-roam return starts the free-roam participant again; leaving
        // retires everything.
        assert_eq!(
            plan(&free, &acquiring),
            (vec![], vec![(4, Which::Scoring, None)])
        );
        assert_eq!(
            plan(&BTreeMap::new(), &raced),
            (vec![(4, Which::Acquisition), (4, Which::Event)], vec![])
        );
        // Participants are independent.
        let both = BTreeMap::from([(4, kinds(true, Some(20))), (5, kinds(false, None))]);
        let mixed = BTreeMap::from([
            (4, vec![(Which::Acquisition, 10), (Which::Event, 20)]),
            (5, vec![(Which::Acquisition, 30), (Which::Scoring, 31)]),
        ]);
        assert_eq!(plan(&both, &mixed), (vec![], vec![]));
    }
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

/// Every other replicated car with decodable chassis physics, for the draft
/// query. Other participants' cars are replicated into this world, including
/// client-simulated traffic. The proximity filters (drift train, speed match)
/// never read these: they count participant-set members only (E1130).
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
        participants: Vec::new(),
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
        self.scoring_failed = true;
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
        vehicle.airborne = Some(physics.controls.is_airborne());
        vehicle.nos = nitrous.map(Nitrous::sample);
        vehicle.burnout = nfs_world::scoring::burnout::Controls::from_wire(&physics.controls, None)
            .map_err(fail("burnout controls"))?;
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
            // The drift and high-speed participant sets hold participants
            // whose own scoring sets that property: other players and AI
            // racers, never traffic (E1130). This host scores only its own
            // player and its rivals run no scoring, so no other participant
            // is a member.
            participants: Vec::new(),
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
            for entity in &live.entities {
                let which = entity.which;
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
                    .program(which)?
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
            let (fires, skipped) = remote_fires(content.bus_entities(which)?, ghost, emission);
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

    /// Retire one participant kind with the root disable event, which banks
    /// every category. Free-roam kinds delete their entity; the event deletes
    /// its own with its other logics, so only pending changes are dropped.
    #[allow(clippy::too_many_arguments)]
    fn retire(
        &self,
        content: &Content,
        players: &mut Players,
        state: &mut State,
        participant: u16,
        which: Which,
        now_ms: u64,
        collected: &mut Collected,
    ) -> Result<(), replication::Error> {
        let live = state
            .live
            .get_mut(&participant)
            .ok_or(replication::Error::Shape)?;
        let position = live
            .entities
            .iter()
            .position(|e| e.which == which)
            .ok_or(replication::Error::Shape)?;
        let entity = live.entities.remove(position);
        if live.entities.is_empty() {
            let car = live.car;
            state.live.remove(&participant);
            state.nitrous.remove(&car);
        }
        let root = content
            .program(which)?
            .nodes
            .iter()
            .position(|n| n.class == "ParticipantLogicEntityData")
            .ok_or(replication::Error::Shape)? as u16;
        let mut session = entity.session;
        let output = session
            .event(root, ROOT_DISABLED, &idle_inputs(self.persona, now_ms))
            .map_err(fail("root disable"))?;
        collected
            .awards
            .extend(output.awards.into_iter().map(|award| (participant, award)));
        state.dirty.remove(&entity.ghost);
        if !which.is_event() {
            players.remove_entities(&[entity.ghost])?;
            state.dirty_deleted.push(entity.ghost);
        }
        tracing::info!(
            participant,
            ?which,
            ghost = entity.ghost,
            "owned scoring participant retired"
        );
        Ok(())
    }

    /// Open a participant's scoring on its car, seeding the car's nitrous fuel.
    fn open(
        &self,
        content: &Content,
        players: &Players,
        state: &mut State,
        target: Target,
        now_ms: u64,
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
                    last_ms: now_ms as f64,
                    clock: None,
                },
            );
        }
        state.live.insert(
            participant,
            Live {
                car,
                entities: Vec::with_capacity(MAX_ENTITIES),
                last_ms: now_ms,
            },
        );
        Ok(())
    }

    /// Start one participant kind. Free-roam kinds create their own entity,
    /// whose creation carries the startup publications; the event kind runs
    /// on the entity the race created, and its startup publications follow
    /// as an update.
    #[allow(clippy::too_many_arguments)]
    fn start(
        &self,
        content: &Content,
        players: &mut Players,
        state: &mut State,
        participant: u16,
        which: Which,
        event_ghost: Option<u16>,
        now_ms: u64,
        collected: &mut Collected,
    ) -> Result<(), replication::Error> {
        let car = state
            .live
            .get(&participant)
            .ok_or(replication::Error::Shape)?
            .car;
        let inputs = self.scoring_inputs(content, players, car, now_ms, state.nitrous.get(&car))?;
        let (session, output) = content
            .start(which, &inputs)
            .map_err(fail("session start"))?;
        let (ghost, output) = match (which, event_ghost) {
            (Which::Event | Which::EventFor(_), Some(ghost)) => (ghost, output),
            (Which::Event | Which::EventFor(_), None) => return Err(replication::Error::Shape),
            (_, _) => {
                let creation = content
                    .creation(which, &session, participant, players)
                    .map_err(fail("creation record"))?;
                let record = players
                    .spawn_entities(vec![creation], content.creation_content())?
                    .remove(0);
                let ghost = record.id;
                merge_record(&mut state.dirty, record)?;
                (
                    ghost,
                    Output {
                        synced: Vec::new(),
                        ..output
                    },
                )
            }
        };
        let reports = content.report_bindings(which, players, ghost)?;
        Self::collect_output(
            content,
            players,
            state,
            Scope {
                participant,
                which,
                ghost,
            },
            output,
            collected,
        )?;
        let live = state
            .live
            .get_mut(&participant)
            .ok_or(replication::Error::Shape)?;
        if live.entities.len() >= MAX_ENTITIES {
            return Err(replication::Error::Bound);
        }
        live.entities.push(Entity {
            which,
            ghost,
            session,
            reports,
        });
        tracing::info!(
            participant,
            car,
            ?which,
            ghost,
            "owned scoring participant started"
        );
        Ok(())
    }

    /// The participant properties the participant's event driving-style
    /// session has written so far (empty without one).
    pub(super) fn event_scoring_properties(
        &self,
        participant: u16,
    ) -> BTreeMap<nfs_content::scoring::participant_properties::Property, i32> {
        let (Some(content), Some(live)) = (
            self.scoring.as_deref(),
            self.scoring_state.live.get(&participant),
        ) else {
            return BTreeMap::new();
        };
        live.entities
            .iter()
            .find(|e| e.which.is_event())
            .map(
                |e| match content.event_properties_for(e.which, &e.session) {
                    Ok(values) => values,
                    Err(error) => {
                        tracing::warn!(participant, ?error, "event scoring properties unreadable");
                        BTreeMap::new()
                    }
                },
            )
            .unwrap_or_default()
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
        if !self.scoring_state.held.is_empty() {
            if now_ms.saturating_sub(self.scoring_state.held_since_ms) < HOLD_LIMIT_MS {
                let state = &self.scoring_state;
                if state.dirty_deleted.is_empty()
                    && state.dirty.values().all(|record| record.initial.is_none())
                {
                    return;
                }
            } else {
                tracing::warn!(
                    batches = ?self.scoring_state.held,
                    "credit commit outstanding; held scoring output released"
                );
                self.scoring_state.held.clear();
                self.release_held(now_ms);
                return;
            }
        }
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

    /// Flush the held records with the committed REP fields, then the held
    /// calls, as one output.
    fn release_held(&mut self, now_ms: u64) {
        let state = &mut self.scoring_state;
        if !state.dirty.is_empty() || !state.dirty_deleted.is_empty() {
            let records = std::mem::take(&mut state.dirty).into_values().collect();
            let deleted = std::mem::take(&mut state.dirty_deleted);
            state.last_flush_ms = now_ms;
            self.pending.push_back(packed(records, deleted));
        }
        let held = std::mem::take(&mut self.scoring_state.held_rpcs);
        tracing::info!(
            calls = held.len(),
            "held scoring output released with the commit"
        );
        self.pending_rpcs.extend(held);
    }

    /// A banked credit committed, failed or was dropped: once no commit is
    /// outstanding, its held output leaves.
    pub(in crate::net) fn release_award(&mut self, batch_id: u64) {
        if self.scoring_state.held.remove(&batch_id) && self.scoring_state.held.is_empty() {
            let now_ms = self
                .scoring_state
                .last_flush_ms
                .max(self.scoring_state.held_since_ms);
            self.release_held(now_ms);
        }
    }

    pub(super) fn desired_scoring(
        &self,
        has_event: bool,
    ) -> BTreeMap<u16, Vec<(Which, Option<u16>)>> {
        let mut desired = BTreeMap::new();
        let crashing: BTreeSet<_> = self
            .crashing_event_participants()
            .filter(|p| self.event_suspended.contains(p))
            .collect();
        for participant in self
            .participants
            .in_free_roam()
            .into_iter()
            .chain(crashing.iter().copied())
        {
            if !self.free_roam_presented.contains(&participant) && !crashing.contains(&participant)
            {
                continue;
            }
            let race = has_event
                .then(|| self.event_driving_style(participant))
                .flatten();
            desired.insert(
                participant,
                kinds(self.event_suspended.contains(&participant), race),
            );
        }
        // Prologue drives (official E1026 122.317, 572.121, 756.224 and
        // 891.324 s): data acquisition lives with each drive's presentation,
        // the free-roam driving-style participant only in the return drive
        // (891.423 s, retired with the drive at 1119.136 s).
        let drive = self
            .logic
            .as_deref()
            .map(crate::garage_logic::GarageLogic::arrived_captures)
            .unwrap_or_default();
        if !drive.is_empty() {
            for (participant, chapter) in self.participants.intro_chapters() {
                let live = self
                    .logic_ghosts
                    .get(&participant)
                    .is_some_and(|ghosts| drive.iter().all(|c| ghosts.contains_key(c)));
                if live && !desired.contains_key(&participant) {
                    let returning = chapter == nfs_world::participants::intro::Chapter::ReturnDrive;
                    desired.insert(participant, kinds(!returning, None));
                }
            }
        }
        desired
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
        let mut desired = self.desired_scoring(content.has_event());
        for (&participant, kinds) in &mut desired {
            for (which, _) in kinds {
                if which.is_event() {
                    let id = self
                        .event_state
                        .event_id(participant)
                        .ok_or(replication::Error::UnknownObject)?;
                    *which = content
                        .event_kind(id)
                        .ok_or(replication::Error::Unsupported)?;
                }
            }
        }
        let running: BTreeMap<u16, Vec<(Which, u16)>> = self
            .scoring_state
            .live
            .iter()
            .map(|(&p, live)| {
                (
                    p,
                    live.entities.iter().map(|e| (e.which, e.ghost)).collect(),
                )
            })
            .collect();
        let (retiring, mut starting) = plan(&desired, &running);
        // A participant without scoring opens on its ready car; without one it
        // waits.
        let mut opening: Vec<Target> = Vec::new();
        for participant in desired.keys() {
            if running.contains_key(participant) {
                continue;
            }
            match self
                .world_cars
                .iter()
                .find(|(_, (owner, ready))| owner == participant && *ready)
            {
                Some((&car, _)) => opening.push(Target {
                    participant: *participant,
                    car,
                }),
                None => starting.retain(|(p, _, _)| p != participant),
            }
        }
        let idle = retiring.is_empty()
            && starting.is_empty()
            && self.scoring_state.reports.is_empty()
            && self.scoring_state.reported.is_empty();
        if idle {
            self.flush_scoring(now_ms, false);
            return Ok(());
        }
        let mut state = self.scoring_state.clone();
        let mut players = self.players.clone();
        let mut collected = Collected::default();
        for (participant, which) in retiring {
            self.retire(
                &content,
                &mut players,
                &mut state,
                participant,
                which,
                now_ms,
                &mut collected,
            )?;
        }
        for target in opening {
            self.open(&content, &players, &mut state, target, now_ms)?;
        }
        for (participant, which, ghost) in starting {
            self.start(
                &content,
                &mut players,
                &mut state,
                participant,
                which,
                ghost,
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
                // A report for a participant kind retired since is dropped.
                let Some(entity) = live.entity_mut(queued.which) else {
                    continue;
                };
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
            // Preserve transitions even when start and release arrive between
            // scheduler passes. Component notifications deliver their snapshots
            // without advancing unrelated queries, timers or fuel.
            let transitions = state
                .nitrous
                .get_mut(&car)
                .map(|n| std::mem::take(&mut n.transitions))
                .unwrap_or_default();
            let kinds: Vec<Which> = state.live[&participant]
                .entities
                .iter()
                .map(|e| e.which)
                .collect();
            for sample in transitions {
                if let Some(vehicle) = inputs.vehicle.as_mut() {
                    vehicle.nos = Some(sample);
                }
                for &which in &kinds {
                    let (output, ghost) = {
                        let entity = state
                            .live
                            .get_mut(&participant)
                            .and_then(|live| live.entity_mut(which))
                            .ok_or(replication::Error::Shape)?;
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
                    nitrous
                        .advance_to(inputs.now * 1000.0)
                        .map_err(fail("nitrous fuel"))?;
                    if let Some(vehicle) = inputs.vehicle.as_mut() {
                        vehicle.nos = Some(nitrous.sample());
                    }
                }
                for &which in &kinds {
                    let (output, ghost) = {
                        let entity = state
                            .live
                            .get_mut(&participant)
                            .and_then(|live| live.entity_mut(which))
                            .ok_or(replication::Error::Shape)?;
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
            if state.held.is_empty() {
                state.held_since_ms = now_ms;
            }
            state.held.insert(batch_id);
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
                event: None,
            });
        }
        if collected.replies.len()
            > nfs_world::session::MAX_RPC_QUEUE
                .saturating_sub(self.pending_rpcs.len() + state.held_rpcs.len())
        {
            return Err(replication::Error::Bound);
        }
        let fired = collected
            .replies
            .iter()
            .any(|reply| matches!(reply, HostRpc::Event(_)));
        self.players = players;
        if !state.held.is_empty() {
            // Wait for the commit (released by `release_award`).
            state.held_rpcs.extend(collected.replies);
            self.scoring_state = state;
            self.flush_scoring(now_ms, false);
            return Ok(());
        }
        self.scoring_state = state;
        self.flush_scoring(now_ms, fired);
        self.pending_rpcs.extend(collected.replies);
        Ok(())
    }

    pub(in crate::net) fn take_award(&mut self) -> Option<AwardRequest> {
        self.scoring_state.pending_awards.pop_front()
    }

    /// The award's ScoreKeeper calls on the gameplay scene, by keeper.
    fn keeper_calls(
        &self,
        players: &Players,
        participant: u16,
        totals: nfs_services::reputation::Scores,
        award: nfs_services::reputation::Scores,
    ) -> Result<Vec<HostRpc>, replication::Error> {
        let Some(content) = self.scoring.as_deref() else {
            return Ok(vec![]);
        };
        let keepers = content.keepers();
        if keepers.is_empty() {
            return Ok(vec![]);
        }
        let world = players.objects();
        let scene = world
            .scene(nfs_content::assets::scene_key("levels/genesis01/gameplay"))
            .ok_or(replication::Error::UnknownObject)?;
        let Some(replication::Initial::SubLevel { fields, .. }) =
            world.get(scene).map(|o| &o.initial)
        else {
            return Err(replication::Error::TypeMismatch);
        };
        let value = |scores: &nfs_services::reputation::Scores,
                     category|
         -> Result<u32, replication::Error> {
            use nfs_world::scoring::interpreter::Category;
            let v = match category {
                Category::Total => scores.total,
                Category::Speed => scores.speed,
                Category::Style => scores.style,
                Category::Build => scores.build,
                Category::Crew => scores.crew,
                Category::Outlaw => scores.outlaw,
            };
            u32::try_from(v).map_err(|_| replication::Error::Shape)
        };
        let mut out = Vec::with_capacity(keepers.len());
        for (category, serializer) in keepers {
            let selector = fields
                .get(serializer)
                .and_then(replication::sublevel::Initial::rpc)
                .ok_or(replication::Error::TypeMismatch)?
                .selector;
            out.push(HostRpc::GameMode(crate::event_logic::call(
                world,
                scene,
                selector,
                0,
                Some(participant),
                nfs_world::game_mode::Arguments::ScoreAward {
                    total: value(&totals, category)?,
                    amount: value(&award, category)?,
                },
            )?));
        }
        Ok(out)
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
        // An event completion also paid cash: present the committed balance.
        if request.event.is_some()
            && let Some(bank) = self.bank_notification(credited.loaded.balance().0)?
        {
            if self.pending_rpcs.len() >= nfs_world::session::MAX_RPC_QUEUE {
                return Err(replication::Error::Bound);
            }
            self.pending_rpcs.push_back(bank);
        }
        self.persistent = Some(credited.loaded);
        tracing::info!(
            participant = request.participant,
            batch_id = request.batch_id,
            event = ?request.event.as_ref().map(|e| e.event_id),
            replayed = credited.replayed,
            total = scores.total,
            level = values.get(nfs_services::reputation::Field::Level),
            fields = changed,
            "owned reputation credit committed"
        );
        // Each ScoreKeeper hears the award: the participant's committed total
        // in that keeper and this award's amount (official E1026 911.825 s:
        // six calls with the award event and the REP update).
        let keepers = self.keeper_calls(&players, request.participant, scores, request.credit)?;
        let held = self.scoring_state.held.contains(&request.batch_id);
        if keepers.len()
            > nfs_world::session::MAX_RPC_QUEUE
                .saturating_sub(self.pending_rpcs.len() + self.scoring_state.held_rpcs.len())
        {
            return Err(replication::Error::Bound);
        }
        self.scoring_state.dirty = dirty;
        self.players = players;
        if held {
            self.scoring_state.held_rpcs.extend(keepers);
        } else {
            self.pending_rpcs.extend(keepers);
        }
        if request.event.is_some() {
            self.event_credit_committed(request.participant, request.batch_id);
        }
        self.release_award(request.batch_id);
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
