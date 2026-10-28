// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The intro participant's world volume memberships (career start, E1194).
//! The official host adds the participant to the event start volumes and
//! switches the start areas' map notifications on as each drive starts (leave
//! `opening`: 122317, 572121, 756224 ms); entering `loading_garage` (298018,
//! 1127014 ms) or `relocating` (747725 ms) removes the memberships, and a
//! relocation also switches the notifications off, as the selection exit does
//! (562319 ms). The activity memberships follow `alive` in the same frame,
//! all 30 at every one of its nine transitions, from the first drive
//! (123619 ms) through the garage request (290318 ms) and the movies to the
//! final return (1119119 ms). Spot volumes are joined and left in the first
//! drive's burst only. Starting a drive reserves the entire ordered handoff
//! with the owned mode transaction; the session's FIFO spreads publication
//! over reply-sized frames. Other transitions observe accepted notifications
//! (`rpc_sent`, `entry_sent`) and reconcile their membership changes in batches.
use super::*;
use crate::prologue::volumes::WorldVolumes;
use nfs_content::events::discovery::Endpoint as Site;
use nfs_world::{
    game_mode::Arguments,
    participants::{Call, Notification},
};

/// Calls published per world loop pass (one reply group).
const BATCH: usize = 15;
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Pulse {
    #[default]
    Never,
    Joining,
    Leaving,
    Done,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Wanted {
    events: bool,
    activities: bool,
    notifications: bool,
    spots: Pulse,
    /// A participant after the intro: the spots stay with the activities
    /// (official E912: joined in the Alive Enter burst, no pulse).
    normal: bool,
}

/// One membership (`Some` participant) or map notification (`None`) change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::net::players) struct Change {
    pub participant: Option<u16>,
    pub site: Site,
    pub join: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::net::players) struct State {
    wanted: BTreeMap<u16, Wanted>,
    // Projection of output reserved in the listener's FIFO, not a client ack.
    // A driving transaction commits this together with its complete output.
    published: BTreeSet<(Option<u16>, Site)>,
}

impl State {
    /// Whether the participant's membership of `site` is published.
    pub fn member(&self, participant: u16, site: Site) -> bool {
        self.published.contains(&(Some(participant), site))
    }
    /// Whether a participant after the intro is in free roam with all of
    /// its wanted memberships published (official E912: its presentation
    /// follows them).
    pub fn settled(
        &self,
        participant: u16,
        volumes: &WorldVolumes,
        resolvable: impl Fn(Site) -> bool,
    ) -> bool {
        self.wanted
            .get(&participant)
            .is_some_and(|w| w.normal && w.activities)
            && self
                .desired(volumes)
                .into_iter()
                .filter(|&(who, site)| who == Some(participant) && resolvable(site))
                .all(|key| self.published.contains(&key))
    }
    /// Update what participants should be in from notifications the session
    /// has just accepted, in send order; `following` are the calls queued
    /// after `calls`. A participant after the intro follows Alive with the
    /// activity and spot memberships only (official E912 76.538-77.537 s, in
    /// the Alive Enter frame and the next ones; the release at the Alive
    /// Leave is inferred from the intro). Its event start volumes belong to
    /// the owned events.
    pub fn observe<'a>(
        &mut self,
        participants: &Lifecycle,
        calls: &'a [HostRpc],
        following: impl Iterator<Item = &'a HostRpc> + Clone,
    ) {
        let (Some(intro), Some(lifecycle)) = (
            participants.intro_bindings(),
            participants.lifecycle_bindings(),
        ) else {
            return;
        };
        for (index, call) in calls.iter().enumerate() {
            let HostRpc::Participant(Notification {
                endpoint,
                participant,
                call: kind,
            }) = *call
            else {
                continue;
            };
            let enter = matches!(kind, Call::Enter);
            let leave = matches!(kind, Call::Leave);
            if participants.intro_chapter(participant).is_none() {
                if endpoint == intro.alive
                    && (enter || leave)
                    && participants.stage(participant).is_some()
                {
                    let wanted = self.wanted.entry(participant).or_default();
                    wanted.normal = true;
                    wanted.activities = enter;
                }
                continue;
            }
            let wanted = self.wanted.entry(participant).or_default();
            if endpoint == intro.opening && leave {
                wanted.events = true;
                wanted.notifications = true;
            } else if endpoint == intro.alive {
                wanted.activities = enter;
                if enter && wanted.spots == Pulse::Never {
                    wanted.spots = Pulse::Joining;
                }
            } else if endpoint == lifecycle.loading_garage && enter {
                wanted.events = false;
            } else if endpoint == intro.relocating && enter {
                wanted.events = false;
                wanted.notifications = false;
            } else if endpoint == lifecycle.composite && leave {
                // Toward the first race, not into customization (1247625 ms).
                let next =
                    calls[index + 1..]
                        .iter()
                        .chain(following.clone())
                        .find_map(|c| match c {
                            HostRpc::Participant(n)
                                if n.participant == participant && n.call == Call::Enter =>
                            {
                                Some(n.endpoint)
                            }
                            _ => None,
                        });
                if next != Some(lifecycle.customization) {
                    wanted.notifications = false;
                }
            }
        }
    }

    /// Stage the subscriptions that must precede Alive. The caller works on a
    /// clone and reserves the complete drive transaction before committing it.
    pub(super) fn before_alive(
        &mut self,
        volumes: &WorldVolumes,
        players: &Players,
        participant: u16,
    ) -> Result<Vec<HostRpc>, replication::Error> {
        let wanted = self.wanted.entry(participant).or_default();
        wanted.events = true;
        wanted.notifications = true;
        let mut changes = Vec::new();
        let mut seen = BTreeSet::new();
        for event in &volumes.event_order {
            for &site in &event.notifications {
                let key = (None, site);
                if !self.published.contains(&key) && seen.insert(key) {
                    changes.push(Change {
                        participant: None,
                        site,
                        join: true,
                    });
                }
            }
            let key = (Some(participant), event.volume);
            if !self.published.contains(&key) && seen.insert(key) {
                changes.push(Change {
                    participant: Some(participant),
                    site: event.volume,
                    join: true,
                });
            }
        }
        // A partial catalog must not silently release the downstream state.
        if volumes
            .event_order
            .iter()
            .map(|e| e.volume)
            .collect::<BTreeSet<_>>()
            != volumes.events
            || volumes
                .event_order
                .iter()
                .flat_map(|e| e.notifications.iter().copied())
                .collect::<BTreeSet<_>>()
                != volumes.notifications
        {
            return Err(replication::Error::Shape);
        }
        self.reserve(players, changes)
    }

    /// Driving subscriptions follow Alive but precede the drive-mode callback.
    pub(super) fn after_alive(
        &mut self,
        volumes: &WorldVolumes,
        players: &Players,
        participant: u16,
    ) -> Result<Vec<HostRpc>, replication::Error> {
        let wanted = self.wanted.entry(participant).or_default();
        wanted.activities = true;
        let pulse = matches!(wanted.spots, Pulse::Never | Pulse::Joining);
        if pulse {
            wanted.spots = Pulse::Leaving;
        }
        let expected: BTreeSet<_> = volumes.activities.union(&volumes.spots).copied().collect();
        if volumes
            .driving_order
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            != expected
            || volumes.driving_order.len() != expected.len()
        {
            return Err(replication::Error::Shape);
        }
        let changes = volumes
            .driving_order
            .iter()
            .copied()
            .filter(|site| {
                volumes.activities.contains(site) || pulse && volumes.spots.contains(site)
            })
            .filter(|&site| !self.published.contains(&(Some(participant), site)))
            .map(|site| Change {
                participant: Some(participant),
                site,
                join: true,
            })
            .collect();
        self.reserve(players, changes)
    }

    /// The once-only spot pulse leaves after the drive callback and before the
    /// canceller callback. Repeated chapters retain their completed pulse.
    pub(super) fn after_drive(
        &mut self,
        volumes: &WorldVolumes,
        players: &Players,
        participant: u16,
    ) -> Result<Vec<HostRpc>, replication::Error> {
        if self
            .wanted
            .get(&participant)
            .is_none_or(|w| w.spots != Pulse::Leaving)
        {
            return Ok(Vec::new());
        }
        let changes = volumes
            .driving_order
            .iter()
            .copied()
            .filter(|site| volumes.spots.contains(site))
            .filter(|&site| self.published.contains(&(Some(participant), site)))
            .map(|site| Change {
                participant: Some(participant),
                site,
                join: false,
            })
            .collect();
        let calls = self.reserve(players, changes)?;
        self.wanted
            .get_mut(&participant)
            .expect("existing owner")
            .spots = Pulse::Done;
        Ok(calls)
    }

    fn reserve(
        &mut self,
        players: &Players,
        changes: Vec<Change>,
    ) -> Result<Vec<HostRpc>, replication::Error> {
        if changes.len() > nfs_world::session::MAX_RPC_QUEUE {
            return Err(replication::Error::Bound);
        }
        let calls = changes
            .iter()
            .map(|change| encode_change(players, change))
            .collect::<Result<Vec<_>, _>>()?;
        for change in changes {
            let key = (change.participant, change.site);
            if change.join {
                self.published.insert(key);
            } else {
                self.published.remove(&key);
            }
        }
        Ok(calls)
    }

    /// Drop memberships the world no longer holds, without removal calls:
    /// participants that left (their memberships went with their objects) and
    /// endpoints whose scene is gone.
    pub fn forget(&mut self, gone: impl Fn(u16) -> bool, resolvable: impl Fn(Site) -> bool) {
        self.wanted.retain(|&participant, _| !gone(participant));
        self.published
            .retain(|&(participant, site)| !participant.is_some_and(&gone) && resolvable(site));
    }

    fn desired(&self, volumes: &WorldVolumes) -> BTreeSet<(Option<u16>, Site)> {
        let mut desired = BTreeSet::new();
        for (&participant, wanted) in &self.wanted {
            let who = Some(participant);
            if wanted.events {
                desired.extend(volumes.events.iter().map(|&s| (who, s)));
            }
            if wanted.activities {
                desired.extend(volumes.activities.iter().map(|&s| (who, s)));
            }
            if wanted.spots == Pulse::Joining || (wanted.normal && wanted.activities) {
                desired.extend(volumes.spots.iter().map(|&s| (who, s)));
            }
            if wanted.notifications {
                desired.extend(volumes.notifications.iter().map(|&s| (None, s)));
            }
        }
        desired
    }

    /// The next changes toward what is wanted, at most `budget` (and one
    /// batch): removals first, then joins in the authored driving order
    /// (official E912 76.538 s: the narrative locations in the Alive Enter
    /// frame, the other families after), the rest after them. Endpoints the
    /// world cannot resolve are left out.
    pub fn plan(
        &self,
        volumes: &WorldVolumes,
        budget: usize,
        resolvable: impl Fn(Site) -> bool,
    ) -> Vec<Change> {
        let desired: BTreeSet<_> = self
            .desired(volumes)
            .into_iter()
            .filter(|&(_, site)| resolvable(site))
            .collect();
        let change = |&(participant, site): &(Option<u16>, Site), join| Change {
            participant,
            site,
            join,
        };
        let rank: BTreeMap<Site, usize> = volumes
            .driving_order
            .iter()
            .enumerate()
            .map(|(index, &site)| (site, index))
            .collect();
        let mut joins: Vec<_> = desired.difference(&self.published).collect();
        joins.sort_by_key(|(_, site)| rank.get(site).copied().unwrap_or(usize::MAX));
        self.published
            .difference(&desired)
            .map(|item| change(item, false))
            .chain(joins.into_iter().map(|item| change(item, true)))
            .take(budget.min(BATCH))
            .collect()
    }

    /// Record published changes and move each spot pulse along once its
    /// joins (then its leaves) are all out.
    pub fn commit(
        &mut self,
        volumes: &WorldVolumes,
        changes: &[Change],
        resolvable: impl Fn(Site) -> bool,
    ) {
        for change in changes {
            if change.join {
                self.published.insert((change.participant, change.site));
            } else {
                self.published.remove(&(change.participant, change.site));
            }
        }
        for (&participant, wanted) in &mut self.wanted {
            let mut spots = volumes.spots.iter().filter(|&&s| resolvable(s));
            match wanted.spots {
                Pulse::Joining
                    if spots.all(|&s| self.published.contains(&(Some(participant), s))) =>
                {
                    wanted.spots = Pulse::Leaving;
                }
                Pulse::Leaving
                    if !spots.any(|&s| self.published.contains(&(Some(participant), s))) =>
                {
                    wanted.spots = Pulse::Done;
                }
                _ => {}
            }
        }
    }
}

fn encode_change(players: &Players, change: &Change) -> Result<HostRpc, replication::Error> {
    let world = players.objects();
    let (scene, selector) = world
        .scene_endpoint(change.site.scene_key, change.site.serializer)
        .ok_or(replication::Error::UnknownObject)?;
    let arguments = if change.join && change.participant.is_some() {
        Arguments::VolumeEntry {
            value: 0,
            flag: false,
        }
    } else {
        Arguments::None
    };
    Ok(HostRpc::GameMode(crate::event_logic::call(
        world,
        scene,
        selector,
        u32::from(!change.join),
        change.participant,
        arguments,
    )?))
}

impl PlayerListener {
    /// The session accepted the front queued RPC. Every notification passes
    /// here once, whichever producer queued it.
    pub(in crate::net) fn rpc_sent(&mut self) {
        let Some(sent) = self.pending_rpcs.pop_front() else {
            return;
        };
        self.world_volumes
            .observe(&self.participants, &[sent], self.pending_rpcs.iter());
    }

    /// The session accepted the front transition entry with its calls.
    pub(in crate::net) fn entry_sent(&mut self) {
        let Some((_, calls)) = self.pending_entries.pop_front() else {
            return;
        };
        self.world_volumes
            .observe(&self.participants, &calls, std::iter::empty());
    }

    /// Publish the next batch of intro world volume changes. Transition
    /// entries go first; a full RPC queue defers the batch.
    pub(in crate::net::players) fn advance_world_volumes(
        &mut self,
        now_ms: u64,
    ) -> Result<(), replication::Error> {
        if !self.pending_entries.is_empty() {
            return Ok(());
        }
        // Starting a drive reserves its whole causal publication in
        // advance_prologue. Do not race that transaction with partial sweeps.
        if self
            .participants
            .intro_participants()
            .any(|p| self.participants.stage(p) == Some(Stage::IntroStartingDrive))
        {
            return Ok(());
        }
        let Some(content) = self.prologue.clone() else {
            return Ok(());
        };
        let world = self.players.objects();
        self.world_volumes.forget(
            |participant| world.get(participant).is_none(),
            |site| {
                world
                    .scene_endpoint(site.scene_key, site.serializer)
                    .is_some()
            },
        );
        let budget = nfs_world::session::MAX_RPC_QUEUE
            .saturating_sub(self.pending_rpcs.len())
            .saturating_sub(BATCH);
        let resolvable = |site: Site| {
            world
                .scene_endpoint(site.scene_key, site.serializer)
                .is_some()
        };
        let changes = self
            .world_volumes
            .plan(&content.volumes, budget, resolvable);
        if changes.is_empty() {
            return Ok(());
        }
        let mut calls = Vec::with_capacity(changes.len());
        for change in &changes {
            calls.push(encode_change(&self.players, change)?);
        }
        let count = |member: bool, join: bool| {
            changes
                .iter()
                .filter(|c| c.participant.is_some() == member && c.join == join)
                .count()
        };
        tracing::info!(
            now_ms,
            joins = count(true, true),
            leaves = count(true, false),
            shown = count(false, true),
            hidden = count(false, false),
            "world volume batch"
        );
        self.pending_rpcs.extend(calls);
        let world = self.players.objects();
        self.world_volumes
            .commit(&content.volumes, &changes, |site| {
                world
                    .scene_endpoint(site.scene_key, site.serializer)
                    .is_some()
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_protocol::world::rpc::{Envelope, Limits, RouteProfile};
    use std::sync::Arc;

    fn site(serializer: usize) -> Site {
        Site {
            scene_key: 909,
            serializer,
        }
    }

    fn ordered_volumes() -> WorldVolumes {
        use crate::prologue::volumes::EventVolume;
        WorldVolumes {
            events: (1..=20).map(site).collect(),
            activities: [site(70)].into(),
            spots: [site(71), site(72)].into(),
            notifications: (31..=50).map(site).collect(),
            event_order: (1..=20)
                .rev()
                .map(|i| EventVolume {
                    volume: site(i),
                    notifications: vec![site(i + 30)],
                })
                .collect(),
            driving_order: vec![site(71), site(70), site(72)],
            cop_patrols: BTreeSet::new(),
        }
    }

    fn decoded(call: HostRpc) -> (Vec<u16>, u16, u32) {
        let wire = call.encode().unwrap();
        let envelope = Envelope::decode(
            wire.span(),
            Limits {
                max_input_bits: 512,
                max_references: 2,
                max_payload_bytes: 16,
            },
        )
        .unwrap();
        let route = envelope.route(RouteProfile::ClientReceive).unwrap();
        (
            envelope.references().to_vec(),
            route.selector(),
            route.method_index(),
        )
    }

    fn at_drive_boundary() -> (PlayerListener, u16, HostRpc) {
        let (mut listener, participant, _) = super::super::tests::setup();
        let intro = listener.participants.intro_bindings().unwrap();
        assert!(matches!(
            listener.participants.intro_vehicle_ready(participant),
            Outcome::Advanced(_)
        ));
        for (endpoint, method) in [(intro.teleport, 3), (intro.render_ready, 0)] {
            let mut payload = nfs_world::bits::BitWriter::new();
            payload
                .put(endpoint.selector.into(), 9)
                .put(method, 32)
                .align();
            let mut request = nfs_world::bits::BitWriter::new();
            request
                .put(0, 32)
                .put(0, 32)
                .put(2, 8)
                .put(endpoint.scene.into(), 13)
                .put(participant.into(), 13)
                .put(payload.bytes().len() as u64, 9)
                .put_span(payload.span());
            assert!(matches!(
                listener
                    .participants
                    .receive_intro(request.span(), |_| true)
                    .unwrap(),
                Some(Outcome::Advanced(_))
            ));
        }
        let Outcome::Advanced(leave) = listener.participants.intro_opening_complete(participant)
        else {
            panic!()
        };
        Arc::make_mut(listener.prologue.as_mut().unwrap()).volumes = ordered_volumes();
        (listener, participant, HostRpc::Participant(leave[0]))
    }

    #[test]
    fn drive_publication_reservation_rolls_back_under_pressure_then_queues_once() {
        let (mut listener, participant, leave) = at_drive_boundary();
        let mut host = crate::net::players::test_wire::session_host();
        for section in split_scenes(
            listener.players.objects().snapshot(),
            nfs_world::application::OUTBOUND_FRAME_BITS - 96,
        )
        .unwrap()
        {
            host.queue_replication(section).unwrap();
        }
        listener
            .pending_rpcs
            .resize(nfs_world::session::MAX_RPC_QUEUE - 1, leave);
        let before = listener.players.objects().snapshot();
        let volumes = listener.world_volumes.clone();
        listener.advance_prologue().unwrap();
        assert_eq!(listener.players.objects().snapshot(), before);
        assert_eq!(listener.world_volumes, volumes);
        assert!(listener.intro_modes.is_empty());
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroStartingDrive)
        );
        assert!(listener.pending.is_empty());
        // Relieve listener pressure through actual session acceptance. The
        // host now has only one RPC slot, exercising partial admission too.
        crate::net::players::test_wire::flush(&mut listener, &mut host).unwrap();
        assert!(listener.pending_rpcs.is_empty());
        listener.advance_prologue().unwrap();
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::FreeRoam)
        );
        let calls: Vec<_> = listener.pending_rpcs.iter().copied().collect();
        let alive = calls.iter().position(|c| matches!(c, HostRpc::Participant(n)
            if n.endpoint == listener.participants.intro_bindings().unwrap().alive && n.call == Call::Enter)).unwrap();
        assert_eq!(
            alive, 40,
            "twenty notification/membership pairs precede Alive"
        );
        let (drive, canceller, _) = &listener.intro_modes[&participant];
        let joined = |ghost| {
            calls
                .iter()
                .position(|c| {
                    matches!(c, HostRpc::GameMode(g)
            if g.method == 4 && g.reference == Some(ghost))
                })
                .unwrap()
        };
        assert!(alive + 3 < joined(drive.ghost));
        assert!(joined(drive.ghost) + 2 < joined(canceller.ghost));
        let created: BTreeSet<_> = listener
            .pending
            .iter()
            .flat_map(|s| s.records.iter().map(|r| r.id))
            .collect();
        assert!(created.contains(&drive.ghost) && created.contains(&canceller.ghost));
        crate::net::players::test_wire::flush(&mut listener, &mut host).unwrap();
        assert_eq!(
            listener.pending_rpcs.iter().copied().collect::<Vec<_>>(),
            calls[1..]
        );
        let retained = listener.pending_rpcs.clone();
        let objects = listener.players.objects().snapshot();
        let volumes = listener.world_volumes.clone();
        listener.advance_prologue().unwrap();
        crate::net::players::test_wire::flush(&mut listener, &mut host).unwrap();
        assert_eq!(listener.pending_rpcs, retained);
        assert_eq!(listener.players.objects().snapshot(), objects);
        assert_eq!(listener.world_volumes, volumes);
    }

    #[test]
    fn an_unresolvable_required_subscription_does_not_release_alive_or_create_modes() {
        let (mut listener, participant, _) = at_drive_boundary();
        let content = Arc::make_mut(listener.prologue.as_mut().unwrap());
        content.volumes.events.insert(site(500));
        content
            .volumes
            .event_order
            .push(crate::prologue::volumes::EventVolume {
                volume: site(500),
                notifications: vec![],
            });
        let before = listener.players.objects().snapshot();
        let volumes = listener.world_volumes.clone();
        assert_eq!(
            listener.advance_prologue(),
            Err(replication::Error::UnknownObject)
        );
        assert_eq!(listener.players.objects().snapshot(), before);
        assert_eq!(listener.world_volumes, volumes);
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroStartingDrive)
        );
        assert!(listener.pending.is_empty() && listener.pending_rpcs.is_empty());
        assert!(listener.intro_modes.is_empty());
    }

    #[test]
    fn staged_handoff_preserves_pairs_and_pulse_order_and_shared_notification_ownership() {
        let (mut listener, participant, _) = super::super::tests::setup();
        let volumes = ordered_volumes();
        let mut state = State::default();
        let before = state
            .before_alive(&volumes, &listener.players, participant)
            .unwrap();
        let after = state
            .after_alive(&volumes, &listener.players, participant)
            .unwrap();
        let leaves = state
            .after_drive(&volumes, &listener.players, participant)
            .unwrap();
        let (scene, _) = listener.players.objects().scene_endpoint(909, 1).unwrap();
        let expected: Vec<_> = (1..=20)
            .rev()
            .flat_map(|i| {
                [
                    (vec![scene], (i + 29) as u16, 0),
                    (vec![scene, participant], (i - 1) as u16, 0),
                ]
            })
            .collect();
        assert_eq!(
            before.iter().copied().map(decoded).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            after.iter().copied().map(decoded).collect::<Vec<_>>(),
            [70, 69, 71].map(|s| (vec![scene, participant], s, 0))
        );
        assert_eq!(
            leaves.iter().copied().map(decoded).collect::<Vec<_>>(),
            [70, 71].map(|s| (vec![scene, participant], s, 1))
        );
        let member_calls: Vec<_> = after
            .iter()
            .map(|c| match c {
                HostRpc::GameMode(call) => *call,
                _ => panic!("membership expected"),
            })
            .collect();
        assert_eq!(counts(&member_calls, participant), [3, 0, 0, 0]);

        // Real session admission, with the fixture's actual creation bindings:
        // no clearing queues or pretending an RPC has been accepted.
        let mut host = crate::net::players::test_wire::session_host();
        for section in split_scenes(
            listener.players.objects().snapshot(),
            nfs_world::application::OUTBOUND_FRAME_BITS - 96,
        )
        .unwrap()
        {
            host.queue_replication(section).unwrap();
        }
        listener
            .pending_rpcs
            .extend(before.iter().chain(&after).chain(&leaves).copied());
        crate::net::players::test_wire::flush(&mut listener, &mut host).unwrap();
        assert!(listener.pending_rpcs.is_empty());

        // A repeated chapter emits neither another spot pulse nor duplicate
        // subscriptions. A second owner still needs its own subscriptions;
        // scene-wide notifications remain shared until the last owner leaves.
        assert!(
            state
                .before_alive(&volumes, &listener.players, participant)
                .unwrap()
                .is_empty()
        );
        assert!(
            state
                .after_alive(&volumes, &listener.players, participant)
                .unwrap()
                .is_empty()
        );
        assert!(
            state
                .after_drive(&volumes, &listener.players, participant)
                .unwrap()
                .is_empty()
        );
        let player = listener
            .players
            .create(
                HOST_SELECTOR as u8,
                listener.persona + 1,
                Request {
                    name: b"Second owner".to_vec(),
                    flag: false,
                    slot: 1,
                },
            )
            .unwrap()
            .unwrap();
        let other = listener
            .players
            .join(HOST_SELECTOR as u8, listener.persona + 1, player.id)
            .unwrap()
            .unwrap()
            .id;
        let calls = state
            .before_alive(&volumes, &listener.players, other)
            .unwrap();
        assert_eq!(calls.len(), 20);
        assert!(calls.into_iter().all(|c| decoded(c).0 == [scene, other]));
        state.wanted.get_mut(&participant).unwrap().notifications = false;
        assert!(
            state
                .plan(&volumes, BATCH, |_| true)
                .iter()
                .all(|c| c.participant.is_some())
        );
    }

    /// Queue the notifications and send them the way the world loop does.
    fn observe(listener: &mut PlayerListener, calls: Vec<HostRpc>) {
        listener.pending_rpcs.extend(calls);
        while !listener.pending_rpcs.is_empty() {
            listener.rpc_sent();
        }
    }

    /// Every batch the world loop would publish until nothing changes.
    fn drain(listener: &mut PlayerListener) -> Vec<nfs_world::game_mode::Call> {
        let mut all = Vec::new();
        loop {
            let before = listener.pending_rpcs.len();
            listener.advance_world_volumes(0).unwrap();
            let added: Vec<_> = listener.pending_rpcs.drain(before..).collect();
            if added.is_empty() {
                return all;
            }
            assert!(added.len() <= BATCH, "one reply group per pass");
            all.extend(added.into_iter().map(|call| match call {
                HostRpc::GameMode(call) => call,
                other => panic!("unexpected {other:?}"),
            }));
        }
    }

    /// (participant joins, participant leaves, notifications on, notifications off)
    fn counts(calls: &[nfs_world::game_mode::Call], participant: u16) -> [usize; 4] {
        let count = |reference: Option<u16>, method: u32| {
            calls
                .iter()
                .filter(|c| c.reference == reference && c.method == method)
                .count()
        };
        for call in calls {
            let wire = call.encode().unwrap();
            let envelope = Envelope::decode(
                wire.span(),
                Limits {
                    max_input_bits: 512,
                    max_references: 2,
                    max_payload_bytes: 16,
                },
            )
            .unwrap();
            let route = envelope.route(RouteProfile::ClientReceive).unwrap();
            let arguments = route.arguments();
            if call.reference.is_some() && call.method == 0 {
                // The client reads a full word and then one boolean. Check
                // those wire fields rather than the producer's enum value.
                assert_eq!(envelope.references(), &[call.target, participant]);
                assert_eq!(arguments.len(), 37);
                assert_eq!(arguments.read_u32(0, 32).unwrap(), 0);
                assert_eq!(arguments.read_u32(32, 1).unwrap(), 0);
                assert_eq!(arguments.read_u32(33, 4).unwrap(), 0);
            } else {
                assert_eq!(arguments.len(), 5);
                assert_eq!(arguments.read_u32(0, 5).unwrap(), 0);
            }
        }
        [
            count(Some(participant), 0),
            count(Some(participant), 1),
            count(None, 0),
            count(None, 1),
        ]
    }

    #[test]
    fn intro_transitions_publish_world_volume_memberships_in_batches() {
        let (mut listener, participant, _) = super::super::tests::setup();
        let mut content = (*listener.prologue.clone().unwrap()).clone();
        content.volumes = WorldVolumes {
            events: (1..=20).map(site).chain([site(500)]).collect(),
            activities: (21..=25).map(site).collect(),
            spots: (26..=30).map(site).collect(),
            notifications: (31..=33).map(site).collect(),
            ..WorldVolumes::default()
        };
        listener.prologue = Some(Arc::new(content));
        let intro = listener.participants.intro_bindings().unwrap();
        let life = listener.participants.lifecycle_bindings().unwrap();
        let note = |endpoint, participant, call| {
            HostRpc::Participant(Notification {
                endpoint,
                participant,
                call,
            })
        };
        let drive = vec![
            note(intro.opening, participant, Call::Leave),
            note(intro.alive, participant, Call::Enter),
        ];
        // The drive starts: events and activities join (the unresolvable
        // serializer 500 is left out), the spots join and leave once, and the
        // start areas' notifications switch on.
        observe(&mut listener, drive.clone());
        let calls = drain(&mut listener);
        assert_eq!(counts(&calls, participant), [30, 5, 3, 0]);
        let (scene, _) = listener.players.objects().scene_endpoint(909, 1).unwrap();
        assert!(calls.iter().all(|c| c.target == scene));
        // A batch waits while transition entries are queued.
        listener
            .pending_entries
            .push_back((returning::empty_section(), Vec::new()));
        observe(
            &mut listener,
            vec![note(intro.alive, participant, Call::Leave)],
        );
        listener.advance_world_volumes(0).unwrap();
        assert!(listener.pending_rpcs.is_empty());
        listener.pending_entries.clear();
        // The garage request leaves the activities, the garage loading the
        // events; the map stays on.
        assert_eq!(counts(&drain(&mut listener), participant), [0, 5, 0, 0]);
        observe(
            &mut listener,
            vec![note(life.loading_garage, participant, Call::Enter)],
        );
        assert_eq!(counts(&drain(&mut listener), participant), [0, 20, 0, 0]);
        // The next drive joins again, without a second spot pulse.
        observe(&mut listener, drive.clone());
        assert_eq!(counts(&drain(&mut listener), participant), [25, 0, 0, 0]);
        // A relocation leaves both and switches the map off.
        observe(
            &mut listener,
            vec![
                note(intro.alive, participant, Call::Leave),
                note(intro.relocating, participant, Call::Enter),
            ],
        );
        assert_eq!(counts(&drain(&mut listener), participant), [0, 25, 0, 3]);
        // Toward a race the composite exit switches the map off; into
        // customization it does not.
        observe(
            &mut listener,
            vec![note(intro.opening, participant, Call::Leave)],
        );
        assert_eq!(counts(&drain(&mut listener), participant), [20, 0, 3, 0]);
        observe(
            &mut listener,
            vec![
                note(life.composite, participant, Call::Leave),
                note(life.customization, participant, Call::Enter),
            ],
        );
        assert!(drain(&mut listener).is_empty());
        observe(
            &mut listener,
            vec![
                note(life.composite, participant, Call::Leave),
                note(life.initial_teleport, participant, Call::Enter),
            ],
        );
        assert_eq!(counts(&drain(&mut listener), participant), [0, 0, 0, 3]);
        // Another participant's identical notifications change nothing.
        observe(
            &mut listener,
            drive
                .iter()
                .map(|c| match *c {
                    HostRpc::Participant(n) => note(n.endpoint, participant + 1, n.call),
                    other => other,
                })
                .collect(),
        );
        assert!(drain(&mut listener).is_empty());
    }

    #[test]
    fn departed_participants_are_forgotten_without_leave_calls() {
        let volumes = WorldVolumes {
            events: BTreeSet::from([site(1)]),
            notifications: BTreeSet::from([site(2)]),
            ..WorldVolumes::default()
        };
        let mut state = State::default();
        for participant in [5, 6] {
            state.wanted.insert(
                participant,
                Wanted {
                    events: true,
                    notifications: true,
                    ..Wanted::default()
                },
            );
        }
        let changes = state.plan(&volumes, BATCH, |_| true);
        state.commit(&volumes, &changes, |_| true);
        assert_eq!(state.published.len(), 3);
        // Nothing is sent for the departed participant; the map stays on.
        state.forget(|participant| participant == 5, |_| true);
        assert!(state.plan(&volumes, BATCH, |_| true).is_empty());
        state.forget(|participant| participant == 6, |_| true);
        assert_eq!(
            state.plan(&volumes, BATCH, |_| true),
            vec![Change {
                participant: None,
                site: site(2),
                join: false,
            }]
        );
    }

    #[test]
    fn a_participant_after_the_intro_holds_activities_and_spots_while_alive() {
        // E912: after a normal garage exit the narrative, mission and spot
        // volumes join in the Alive Enter burst and stay (no spot pulse).
        let volumes = WorldVolumes {
            events: BTreeSet::from([site(1)]),
            activities: BTreeSet::from([site(70)]),
            spots: BTreeSet::from([site(71)]),
            driving_order: vec![site(71), site(70)],
            ..WorldVolumes::default()
        };
        let mut state = State::default();
        let alive = |activities| Wanted {
            normal: true,
            activities,
            ..Wanted::default()
        };
        state.wanted.insert(5, alive(true));
        let changes = state.plan(&volumes, BATCH, |_| true);
        let joined: BTreeSet<_> = changes
            .iter()
            .map(|c| (c.participant, c.site, c.join))
            .collect();
        assert_eq!(
            joined,
            BTreeSet::from([(Some(5), site(70), true), (Some(5), site(71), true)]),
            "no event volume: the owned events subscribe those"
        );
        assert_eq!(
            changes.iter().map(|c| c.site).collect::<Vec<_>>(),
            [site(71), site(70)],
            "joins follow the driving order, not the endpoint order"
        );
        // The budget takes the front of that order.
        assert_eq!(state.plan(&volumes, 1, |_| true)[0].site, site(71));
        assert!(!state.settled(5, &volumes, |_| true), "joins outstanding");
        state.commit(&volumes, &changes, |_| true);
        assert!(
            state.plan(&volumes, BATCH, |_| true).is_empty(),
            "spots stay"
        );
        assert!(state.settled(5, &volumes, |_| true));
        assert!(!state.settled(6, &volumes, |_| true), "no Alive seen");
        state.wanted.insert(5, alive(false));
        let left = state.plan(&volumes, BATCH, |_| true);
        assert_eq!(left.len(), 2);
        assert!(left.iter().all(|c| !c.join));
    }

    #[test]
    fn memberships_of_a_vanished_scene_are_forgotten_without_removal_calls() {
        let volumes = WorldVolumes {
            events: BTreeSet::from([site(1), site(2)]),
            ..WorldVolumes::default()
        };
        let mut state = State::default();
        state.wanted.insert(
            5,
            Wanted {
                events: true,
                ..Wanted::default()
            },
        );
        let changes = state.plan(&volumes, BATCH, |_| true);
        state.commit(&volumes, &changes, |_| true);
        assert_eq!(state.published.len(), 2);
        // Serializer 1's scene is gone: no call can name it, so none is planned.
        let resolvable = |s: Site| s != site(1);
        state.forget(|_| false, resolvable);
        assert_eq!(state.published, BTreeSet::from([(Some(5), site(2))]));
        assert!(state.plan(&volumes, BATCH, resolvable).is_empty());
    }

    #[test]
    fn the_world_tick_drops_an_unresolvable_membership_instead_of_failing() {
        let (mut listener, participant, _) = super::super::tests::setup();
        listener
            .world_volumes
            .published
            .insert((Some(participant), site(500)));
        listener.advance_world_volumes(0).unwrap();
        assert!(listener.pending_rpcs.is_empty());
        assert!(listener.world_volumes.published.is_empty());
    }

    /// A client crossing report `[scene, participant]` m2/m3 a103: the f64
    /// clock high word first, the token, seven pad bits.
    fn crossing(
        scene: u16,
        selector: u16,
        participant: u16,
        method: u32,
        token: u32,
    ) -> nfs_world::bits::BitWriter {
        let bits = 125.5f64.to_bits();
        let mut payload = nfs_world::bits::BitWriter::new();
        payload
            .put(selector.into(), 9)
            .put(method.into(), 32)
            .put(bits >> 32, 32)
            .put(bits & 0xffff_ffff, 32)
            .put(token.into(), 32)
            .put(0, 7)
            .align();
        let mut body = nfs_world::bits::BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(scene.into(), 13)
            .put(participant.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body
    }

    #[test]
    fn assigned_volume_reports_are_consumed_late_ones_dropped_and_bad_tokens_refused() {
        use super::super::tests::send;
        let (mut listener, participant, _) = at_drive_boundary();
        listener.advance_prologue().unwrap();
        let activity = site(70);
        assert!(listener.world_volumes.member(participant, activity));
        let (scene, selector) = listener.players.objects().scene_endpoint(909, 70).unwrap();
        let unsupported = listener.unsupported_rpcs;
        let entered = crossing(scene, selector, participant, 2, 0);
        assert!(send(&mut listener, 1, 47, entered.span()));
        assert_eq!(
            listener.activity_inside.get(&(participant, activity)),
            Some(&true)
        );
        // A repeat changes nothing; the exit is recorded.
        assert!(send(&mut listener, 2, 47, entered.span()));
        let left = crossing(scene, selector, participant, 3, 0);
        assert!(send(&mut listener, 3, 47, left.span()));
        assert_eq!(
            listener.activity_inside.get(&(participant, activity)),
            Some(&false)
        );
        assert_eq!(listener.unsupported_rpcs, unsupported);
        // A token the host never assigned is malformed.
        let wrong = crossing(scene, selector, participant, 2, 1);
        assert!(!send(&mut listener, 4, 47, wrong.span()));
        // After the membership left, a report is late and dropped.
        listener
            .world_volumes
            .published
            .remove(&(Some(participant), activity));
        let before = listener.activity_inside.clone();
        assert!(send(&mut listener, 4, 47, entered.span()));
        assert_eq!(listener.activity_inside, before);
        assert_eq!(listener.unsupported_rpcs, unsupported);
    }

    #[test]
    fn cop_patrol_requests_are_consumed_without_an_answer() {
        use super::super::tests::send;
        let (mut listener, participant, _) = at_drive_boundary();
        Arc::make_mut(listener.prologue.as_mut().unwrap())
            .volumes
            .cop_patrols
            .insert(site(80));
        let (scene, selector) = listener.players.objects().scene_endpoint(909, 80).unwrap();
        let mut payload = nfs_world::bits::BitWriter::new();
        payload
            .put(selector.into(), 9)
            .put(0, 32)
            .put(0b1010101, 7)
            .align();
        let mut body = nfs_world::bits::BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(scene.into(), 13)
            .put(participant.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        let unsupported = listener.unsupported_rpcs;
        let rpcs = listener.pending_rpcs.len();
        assert!(send(&mut listener, 1, 47, body.span()));
        assert_eq!(listener.unsupported_rpcs, unsupported);
        assert_eq!(listener.pending_rpcs.len(), rpcs);
    }
}
