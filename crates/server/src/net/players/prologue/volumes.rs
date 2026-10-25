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
//! drive's burst only. The rules see each notification when the session
//! accepts it (`rpc_sent`, `entry_sent`), whichever producer queued it, and
//! record what the participant should be in; the world loop publishes the
//! difference in reply-sized batches once the transition's own entries are
//! out, as the official host spreads it over full frames.
use super::*;
use crate::prologue::volumes::WorldVolumes;
use nfs_content::events::discovery::Endpoint as Site;
use nfs_world::{
    game_mode::Arguments,
    participants::{Call, Notification},
};

/// Calls published per world loop pass (one reply group).
const BATCH: usize = 15;
/// The participant-set argument of every official membership call.
const MEMBERSHIP: u32 = 3;

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
}

/// One membership (`Some` participant) or map notification (`None`) change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::net::players) struct Change {
    pub participant: Option<u16>,
    pub site: Site,
    pub join: bool,
}

#[derive(Clone, Debug, Default)]
pub(in crate::net::players) struct State {
    wanted: BTreeMap<u16, Wanted>,
    published: BTreeSet<(Option<u16>, Site)>,
}

impl State {
    /// Update what intro participants should be in from notifications the
    /// session has just accepted, in send order; `following` are the calls
    /// queued after `calls`. Other participants are not affected.
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
            if participants.intro_chapter(participant).is_none() {
                continue;
            }
            let enter = matches!(kind, Call::Enter);
            let leave = matches!(kind, Call::Leave);
            let wanted = self.wanted.entry(participant).or_default();
            if endpoint == intro.opening && leave {
                wanted.events = true;
                wanted.notifications = true;
                if wanted.spots == Pulse::Never {
                    wanted.spots = Pulse::Joining;
                }
            } else if endpoint == intro.alive {
                wanted.activities = enter;
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
            if wanted.spots == Pulse::Joining {
                desired.extend(volumes.spots.iter().map(|&s| (who, s)));
            }
            if wanted.notifications {
                desired.extend(volumes.notifications.iter().map(|&s| (None, s)));
            }
        }
        desired
    }

    /// The next changes toward what is wanted, at most `budget` (and one
    /// batch): removals first. Endpoints the world cannot resolve are left out.
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
        self.published
            .difference(&desired)
            .map(|item| change(item, false))
            .chain(
                desired
                    .difference(&self.published)
                    .map(|item| change(item, true)),
            )
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
            let (scene, selector) = world
                .scene_endpoint(change.site.scene_key, change.site.serializer)
                .ok_or(replication::Error::UnknownObject)?;
            let arguments = if change.join && change.participant.is_some() {
                Arguments::Word(MEMBERSHIP)
            } else {
                Arguments::None
            };
            calls.push(HostRpc::GameMode(crate::event_logic::call(
                world,
                scene,
                selector,
                u32::from(!change.join),
                change.participant,
                arguments,
            )?));
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
    use std::sync::Arc;

    fn site(serializer: usize) -> Site {
        Site {
            scene_key: 909,
            serializer,
        }
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
            let expected = if call.reference.is_some() && call.method == 0 {
                Arguments::Word(MEMBERSHIP)
            } else {
                Arguments::None
            };
            assert_eq!(call.arguments, expected, "{call:?}");
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
}
