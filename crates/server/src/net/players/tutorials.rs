// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tutorial logics of the session's participants (`crate::tutorials`). The
//! gate facts come from the intro modes (a live mode is its objective's
//! active span; its first observation is its launch) and the committed
//! progression (the return objective's completion, at once for a restored
//! one: E912 re-derives the intro finish 5 s after the logic batch).
use super::*;
use nfs_world::participants::{HostRpc, Stage, intro::Chapter};

/// Seconds between driven-distance samples.
const DISTANCE_SAMPLE_SECONDS: f64 = 1.0;
/// Metres per second beyond which a position change is a relocation.
const MAX_SPEED_M_S: f64 = 150.0;

const CHAPTERS: [Chapter; 4] = [
    Chapter::FirstDrive,
    Chapter::FirstRace,
    Chapter::SecondRace,
    Chapter::ReturnDrive,
];

impl PlayerListener {
    fn tutorial_content(&self) -> Option<std::sync::Arc<crate::tutorials::Content>> {
        Some(self.progression.as_ref()?.tutorials.as_ref()?.0.clone())
    }

    /// The stored seen set of the loaded account.
    pub(super) fn stored_tutorials(
        loaded: &crate::persistent::Loaded,
    ) -> Result<BTreeSet<u32>, replication::Error> {
        nfs_services::tutorials::seen(loaded.owned(nfs_services::tutorials::TABLE))
            .map_err(|_| replication::Error::Shape)
    }

    /// The initial notification flush's tutorial values.
    pub(super) fn tutorial_notifications(
        &self,
        players: &Players,
        loaded: &crate::persistent::Loaded,
    ) -> Result<Vec<HostRpc>, replication::Error> {
        let Some(content) = self.tutorial_content() else {
            return Ok(vec![]);
        };
        content.notifications(players.objects(), &Self::stored_tutorials(loaded)?)
    }

    pub(super) fn advance_tutorials(&mut self) -> Result<(), replication::Error> {
        self.advance_chapter_notifications()?;
        let Some(content) = self.tutorial_content() else {
            return Ok(());
        };
        let Some(loaded) = self.persistent.clone() else {
            return Ok(());
        };
        if self.session_backlog >= MAX_PENDING
            || !self.pending_entries.is_empty()
            || self.pending.len() >= MAX_PENDING
        {
            return Ok(());
        }
        let Some(seconds) = self.event_seconds(self.world_ms) else {
            return Ok(());
        };
        if !content.ready(self.players.objects()) {
            return Ok(());
        }
        let stored = Self::stored_tutorials(&loaded)?;
        let restored = match &self.progression {
            Some(progression) => Some(
                progression
                    .catalog
                    .restore(&loaded, progression.settings)
                    .map_err(|_| replication::Error::Shape)?,
            ),
            None => None,
        };
        let objective = |id: u64| {
            restored
                .as_ref()
                .and_then(|r| {
                    r.branch(crate::progression::Branch::Intro)
                        .objectives()
                        .get(&id)
                        .map(|o| (o.active, o.completed))
                })
                .unwrap_or((false, false))
        };
        let prologue = self.prologue.clone();
        let mut players = self.players.clone();
        let mut sessions = self.tutorials.clone();
        let mut samples = self.tutorial_samples.clone();
        let mut commits = Vec::new();
        let mut records = Vec::new();
        let mut deleted = Vec::new();
        let mut calls = Vec::new();
        let owners: Vec<u16> = players.player_participants().collect();
        sessions.retain(|p, _| owners.contains(p));
        for participant in owners {
            // The logic batch follows the progression restore.
            if !self.entered(participant) {
                continue;
            }
            let session = sessions
                .entry(participant)
                .or_insert_with(|| content.session(participant, stored.clone()));
            session.keep_locations(|v| {
                self.world_volumes.member(
                    participant,
                    nfs_content::events::discovery::Endpoint {
                        scene_key: v.scene_key,
                        serializer: v.serializer,
                    },
                )
            });
            let mut held = Vec::new();
            session.objectives(&content, objective, seconds);
            if let Some(prologue) = prologue.as_deref() {
                let running = self.intro_modes.get(&participant);
                for chapter in CHAPTERS {
                    let Ok(mode) = prologue.mode(chapter) else {
                        continue;
                    };
                    if running
                        .is_some_and(|(active, _, c)| *c == chapter && active.objective_live())
                    {
                        session.launched(&content, &mode.data.prefab, seconds);
                    }
                }
                if let Some((active, canceller, chapter)) = running {
                    held.extend(
                        [
                            prologue
                                .mode(*chapter)
                                .ok()
                                .and_then(|m| active.held_categories(m)),
                            prologue
                                .driving
                                .as_ref()
                                .and_then(|pair| canceller.held_categories(&pair.1)),
                        ]
                        .into_iter()
                        .flatten(),
                    );
                }
            }
            // Driven distance: the car's position sampled once a second,
            // as DriverDetailsLogic's distance loop does (Delay #17, 1 s);
            // a jump beyond any car's speed is a teleport, not driving.
            match self
                .world_cars
                .iter()
                .find(|(_, (p, ready))| *p == participant && *ready)
                .and_then(|(&car, _)| {
                    players
                        .vehicle_physics(HOST_SELECTOR as u8, self.persona, car)
                        .ok()?
                        .decode([0.; 3])
                        .ok()
                }) {
                Some(motion) => {
                    let position = motion.position.map(f64::from);
                    match samples.get(&participant).copied() {
                        Some((last, at)) if seconds - at >= DISTANCE_SAMPLE_SECONDS => {
                            let metres = position
                                .iter()
                                .zip(last)
                                .map(|(a, b)| (a - b) * (a - b))
                                .sum::<f64>()
                                .sqrt();
                            if metres <= MAX_SPEED_M_S * (seconds - at) {
                                session.drove(metres);
                            }
                            samples.insert(participant, (position, seconds));
                        }
                        Some(_) => {}
                        None => {
                            samples.insert(participant, (position, seconds));
                        }
                    }
                }
                None => {
                    samples.remove(&participant);
                }
            }
            let out = session.advance(&content, &mut players, seconds, &held)?;
            records.extend(out.records);
            deleted.extend(out.deleted);
            calls.extend(out.calls);
            commits.extend(out.commits.into_iter().map(|key| (participant, key)));
        }
        samples.retain(|p, _| sessions.contains_key(p));
        let mut story = self.story_pending.clone();
        if let Some(progression) = self.progression.clone() {
            for (participant, key) in commits {
                let player_index = players
                    .runtime_index(HOST_SELECTOR as u8, self.persona, participant)
                    .ok_or(replication::Error::UnknownObject)?;
                if story.len() >= MAX_PENDING {
                    return Ok(());
                }
                story.push_back(crate::net::items::story::Request {
                    participant,
                    player_index: u32::from(player_index),
                    progression: progression.clone(),
                    action: crate::net::items::story::Action::Tutorial { key },
                });
            }
        }
        if records.is_empty() && deleted.is_empty() && calls.is_empty() {
            self.tutorials = sessions;
            self.tutorial_samples = samples;
            self.story_pending = story;
            return Ok(());
        }
        let mut sections = split_scenes(records, nfs_world::application::OUTBOUND_FRAME_BITS - 96)?;
        if !deleted.is_empty() {
            sections.push(Section {
                float_bits: None,
                flag: false,
                setup: None,
                records: vec![],
                deleted,
            });
        }
        if calls.len() > nfs_world::session::MAX_RPC_QUEUE {
            return Err(replication::Error::Bound);
        }
        if sections.len() > MAX_PENDING.saturating_sub(self.pending.len())
            || calls.len()
                > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len())
        {
            return Ok(());
        }
        self.players = players;
        self.tutorials = sessions;
        self.tutorial_samples = samples;
        self.story_pending = story;
        self.pending.extend(sections);
        self.pending_rpcs.extend(calls);
        Ok(())
    }

    /// Participants whose logic batch exists (after the progression restore).
    fn entered(&self, participant: u16) -> bool {
        !matches!(
            self.participants.stage(participant),
            None | Some(
                Stage::LoadingInventory
                    | Stage::LoadingPersistentData
                    | Stage::PersistentDataLoaded
                    | Stage::Progressing
            )
        )
    }

    /// Send the chapter values that changed in the committed progression.
    fn advance_chapter_notifications(&mut self) -> Result<(), replication::Error> {
        let (Some(progression), Some(loaded)) = (self.progression.clone(), self.persistent.clone())
        else {
            return Ok(());
        };
        let Some(content) = progression.chapter_notifications.clone() else {
            return Ok(());
        };
        let restored = progression
            .catalog
            .restore(&loaded, progression.settings)
            .map_err(|_| replication::Error::Shape)?;
        let objectives = restored
            .branch(crate::progression::Branch::Intro)
            .objectives();
        let state = |id| {
            objectives
                .get(&id)
                .map_or((false, false), |o| (o.active, o.completed))
        };
        let mut sent = self.chapter_sent.clone();
        let owners: Vec<u16> = self.players.player_participants().collect();
        sent.retain(|p, _| owners.contains(p));
        let mut calls = vec![];
        let mut boxes = vec![];
        for participant in owners {
            if !self.entered(participant) {
                continue;
            }
            let values = sent.entry(participant).or_default();
            calls.extend(content.update(self.players.objects(), state, values)?);
            boxes.extend(content.entry_box(values).map(|on| (participant, on)));
        }
        if calls.len() > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len()) {
            return Ok(());
        }
        self.chapter_sent = sent;
        self.pending_rpcs.extend(calls);
        for (participant, enabled) in boxes {
            self.participants.set_entry_box(participant, enabled);
        }
        Ok(())
    }

    /// A client accept of a live tutorial: queue its durable seen value.
    /// `Ok(false)` when the target is no tutorial. A malformed accept is
    /// logged and dropped (the logic stays live).
    pub(super) fn tutorial_accept(
        &self,
        sessions: &mut BTreeMap<u16, crate::tutorials::Session>,
        players: &Players,
        story_pending: &mut VecDeque<crate::net::items::story::Request>,
        message: &nfs_world::logic::Message,
    ) -> Result<bool, replication::Error> {
        let nfs_world::logic::Message::Reached {
            event,
            target,
            player,
        } = message
        else {
            return Ok(false);
        };
        let (Some(content), Some(progression)) =
            (self.tutorial_content(), self.progression.clone())
        else {
            return Ok(false);
        };
        let Some((&participant, session)) = sessions.iter_mut().find(|(_, s)| s.owns(target.ghost))
        else {
            return Ok(false);
        };
        let key = match session.accept(&content, *event, *target, *player) {
            Ok(Some(key)) => key,
            Ok(None) => return Ok(true),
            Err(error) => {
                tracing::warn!(
                    participant,
                    ghost = target.ghost,
                    entity = target.entity,
                    event,
                    player,
                    ?error,
                    "tutorial accept does not match the logic; dropped"
                );
                return Ok(true);
            }
        };
        let player_index = players
            .runtime_index(HOST_SELECTOR as u8, self.persona, participant)
            .ok_or(replication::Error::UnknownObject)?;
        if story_pending.len() >= MAX_PENDING {
            return Err(replication::Error::Bound);
        }
        tracing::info!(participant, ghost = target.ghost, key, "tutorial accepted");
        story_pending.push_back(crate::net::items::story::Request {
            participant,
            player_index: u32::from(player_index),
            progression,
            action: crate::net::items::story::Action::Tutorial { key },
        });
        Ok(true)
    }

    /// A tutorial's seen value is durable: its logic goes at the next advance.
    pub(super) fn tutorial_committed(
        &mut self,
        participant: u16,
        key: u32,
        loaded: &crate::persistent::Loaded,
    ) -> Result<(), replication::Error> {
        let Some(content) = self.tutorial_content() else {
            return Err(replication::Error::Unsupported);
        };
        if !Self::stored_tutorials(loaded)?.contains(&key) {
            return Err(replication::Error::Shape);
        }
        if let Some(session) = self.tutorials.get_mut(&participant) {
            session.committed(&content, key);
        }
        Ok(())
    }

    /// The seen commit failed: the logic stays live for another accept.
    pub(in crate::net) fn story_failed(&mut self, request: &crate::net::items::story::Request) {
        if let crate::net::items::story::Action::Tutorial { key } = request.action
            && let Some(content) = self.tutorial_content()
            && let Some(session) = self.tutorials.get_mut(&request.participant)
        {
            session.failed(&content, key);
        }
    }
}
