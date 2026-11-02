// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded, per-connection map membership. No start-volume subscriptions or
//! prompts are created for game modes whose playable lifecycle is not owned yet.
use super::*;
use crate::event_logic::availability;
use nfs_content::events::discovery::Endpoint as Site;
use nfs_content::events::discovery::membership::Fact;
use std::sync::Arc;

const BATCH: usize = 16;

#[derive(Clone, Debug, Default)]
pub(in crate::net::players) struct State {
    enabled: BTreeSet<Site>,
    wanted: BTreeSet<Site>,
    eligible: BTreeMap<u64, availability::Participant>,
    model: Option<Model>,
    pub(in crate::net::players) failures: usize,
}
#[derive(Clone, Debug)]
struct Model {
    content: Arc<availability::Content>,
    runtime: availability::State,
    saved: Option<Saved>,
}
#[derive(Clone, Debug)]
struct Saved {
    loaded: Arc<crate::persistent::Loaded>,
    progression: Arc<crate::progression::Content>,
    facts: BTreeMap<Fact, bool>,
}
impl State {
    fn clear(&mut self) {
        self.wanted.clear();
        self.eligible.clear();
    }
    fn project(
        &mut self,
        content: &Arc<availability::Content>,
        progression: &Arc<crate::progression::Content>,
        loaded: &Arc<crate::persistent::Loaded>,
        roster: &BTreeSet<u64>,
        scene: Option<&crate::prologue::membership::Session>,
    ) -> Result<(), String> {
        if self
            .model
            .as_ref()
            .is_none_or(|m| !Arc::ptr_eq(&m.content, content))
        {
            self.model = Some(Model {
                content: content.clone(),
                runtime: availability::State::new(content.clone())
                    .map_err(|error| format!("map owner construction: {error:?}"))?,
                saved: None,
            });
        }
        let model = self.model.as_mut().ok_or("map owner missing")?;
        if model.saved.as_ref().is_none_or(|s| {
            !Arc::ptr_eq(&s.loaded, loaded) || !Arc::ptr_eq(&s.progression, progression)
        }) {
            model.saved = Some(Saved {
                facts: content
                    .saved_facts(progression, loaded)
                    .map_err(|error| error.to_string())?,
                loaded: loaded.clone(),
                progression: progression.clone(),
            });
        }
        let saved = model.saved.as_ref().ok_or("saved projection missing")?;
        let mut missing = None;
        let result = model
            .runtime
            .update(roster, &[], |participant, fact| {
                let value = (|| match fact {
                    Fact::Player(owner) if owner == content.player_manager() => {
                        Some(roster.contains(&participant))
                    }
                    Fact::States(sites) => {
                        let participant = u16::try_from(participant).ok()?;
                        let scene = scene?;
                        // Validate every endpoint, even when an earlier state is true.
                        sites.iter().try_fold(false, |any, site| {
                            Some(scene.contains_at(*site, participant).ok()? || any)
                        })
                    }
                    _ => saved.facts.get(fact).copied(),
                })();
                if value.is_none() && missing.is_none() {
                    missing = Some(fact.clone());
                }
                value
            })
            .map_err(|error| format!("map membership: {error:?}; missing={missing:?}"))?;
        self.wanted = result
            .values()
            .flat_map(|p| p.notifications.iter().copied())
            .collect();
        self.eligible = result;
        Ok(())
    }
    fn plan(&self, wanted: &BTreeSet<Site>, budget: usize) -> Vec<(Site, bool)> {
        self.enabled
            .difference(wanted)
            .map(|s| (*s, false))
            .chain(wanted.difference(&self.enabled).map(|s| (*s, true)))
            .take(budget.min(BATCH))
            .collect()
    }
    fn commit(&mut self, changes: &[(Site, bool)]) {
        for &(site, enabled) in changes {
            if enabled {
                self.enabled.insert(site);
            } else {
                self.enabled.remove(&site);
            }
        }
    }
}

impl PlayerListener {
    pub(super) fn event_available(&self, content: &event_logic::Content, participant: u16) -> bool {
        self.events
            .as_ref()
            .and_then(|c| c.availability())
            .is_none()
            || self
                .event_discovery
                .eligible
                .get(&u64::from(participant))
                .is_some_and(|p| p.eligible.contains(&content.event().game_mode_id))
    }
    pub(super) fn map_owns_notification(
        &self,
        content: &event_logic::Content,
        serializer: usize,
    ) -> bool {
        self.events
            .as_ref()
            .filter(|c| c.availability().is_some())
            .is_some_and(|c| {
                c.discovery_notifications().contains(&Site {
                    scene_key: content.event().scene_content_key,
                    serializer,
                })
            })
    }
    fn availability_failure(&mut self, producer: &'static str, error: impl std::fmt::Debug) {
        self.event_discovery.clear();
        self.event_discovery.failures = self.event_discovery.failures.saturating_add(1);
        if super::super::refusal::logged(self.event_discovery.failures) {
            tracing::warn!(
                connection = self.unsupported.connection(),
                producer,
                ?error,
                failures = self.event_discovery.failures,
                "event eligibility unavailable; starts disabled, movement remains eligible"
            );
        }
    }
    pub(super) fn advance_event_discovery(&mut self) -> Result<(), replication::Error> {
        let present = !self.event_state.failed
            && (self
                .participants
                .in_free_roam()
                .iter()
                .any(|p| self.free_roam_presented.contains(p))
                || self.crashing_event_participants().next().is_some()
                || self.event_state.restart_resuming().next().is_some());
        if let Some(catalog) = self.events.clone().filter(|_| present) {
            if let Some(content) = catalog.availability() {
                let result = match (&self.progression, &self.persistent) {
                    (Some(progression), Some(loaded)) => {
                        let roster = self
                            .players
                            .player_participants()
                            .filter(|&p| {
                                self.players
                                    .owns_participant(HOST_SELECTOR as u8, self.persona, p)
                            })
                            .map(u64::from)
                            .collect();
                        self.event_discovery.project(
                            content,
                            progression,
                            loaded,
                            &roster,
                            self.scene_membership.as_ref(),
                        )
                    }
                    _ => Err("committed save or progression content missing".into()),
                };
                if let Err(error) = result {
                    self.availability_failure("saved and scene membership", error);
                }
            } else {
                self.event_discovery.wanted = catalog.discovery_notifications().clone();
            }
        } else {
            self.event_discovery.clear();
        }
        // Refresh eligibility even under backpressure, but defer publication.
        if !self.pending_entries.is_empty() {
            return Ok(());
        }
        let budget = nfs_world::session::MAX_RPC_QUEUE
            .saturating_sub(self.pending_rpcs.len())
            .saturating_sub(BATCH);
        if let Err(error) = self.publish_event_discovery(budget) {
            self.availability_failure("map notification publication", error);
        }
        Ok(())
    }
    fn publish_event_discovery(&mut self, budget: usize) -> Result<(), replication::Error> {
        let changes = self
            .event_discovery
            .plan(&self.event_discovery.wanted, budget);
        let world = self.players.objects();
        let mut calls = Vec::new();
        for &(site, enabled) in &changes {
            let Some((scene, selector)) = world.scene_endpoint(site.scene_key, site.serializer)
            else {
                // Deleted scenes no longer need an explicit removal call.
                if !enabled {
                    continue;
                }
                return Err(replication::Error::UnknownObject);
            };
            calls.push(HostRpc::GameMode(event_logic::call(
                world,
                scene,
                selector,
                u32::from(!enabled),
                None,
                Arguments::None,
            )?));
        }
        self.pending_rpcs.extend(calls);
        self.event_discovery.commit(&changes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sites(n: usize) -> BTreeSet<Site> {
        (0..n)
            .map(|serializer| Site {
                scene_key: 10,
                serializer,
            })
            .collect()
    }
    #[test]
    fn bounded_membership_reconciles_restarts_and_repeated_ticks() {
        let mut state = State::default();
        let desired = sites(100);
        assert!(state.plan(&desired, 0).is_empty());
        while state.enabled != desired {
            let changes = state.plan(&desired, BATCH);
            assert!(!changes.is_empty() && changes.len() <= BATCH);
            state.commit(&changes);
        }
        assert!(state.plan(&desired, BATCH).is_empty());
        let empty = BTreeSet::new();
        let removed = state.plan(&empty, BATCH);
        assert!(removed.iter().all(|(_, on)| !on));
        state.commit(&removed);
        // A restart reversed while removal is still in progress: restore only
        // the memberships actually removed, without duplicating the remainder.
        assert_eq!(
            state.plan(&desired, BATCH),
            removed.iter().map(|(s, _)| (*s, true)).collect::<Vec<_>>()
        );
        while !state.enabled.is_empty() {
            state.commit(&state.plan(&empty, BATCH));
        }
        assert!(state.plan(&empty, BATCH).is_empty());
    }
    #[test]
    fn uncommitted_batches_and_other_connections_do_not_change_membership() {
        let state = State::default();
        let target = sites(20);
        let changes = state.plan(&target, BATCH);
        assert_eq!(state.plan(&target, BATCH), changes);
        let mut other = State::default();
        other.commit(&changes);
        assert_ne!(other.plan(&target, BATCH), changes);
        assert_eq!(state.plan(&target, BATCH), changes);
    }
    #[test]
    fn disabling_discovery_defers_for_queued_output_then_removes_membership() {
        use crate::event_logic::tests as fixture;
        let (players, _, _) = fixture::world();
        let mut listener = PlayerListener::new(fixture::PERSONA);
        listener.players = players;
        listener.event_discovery.enabled.insert(Site {
            scene_key: fixture::RACE,
            serializer: 6,
        });
        listener
            .pending_entries
            .push_back((returning::empty_section(), Vec::new()));
        listener.advance_event_discovery().unwrap();
        assert_eq!(listener.event_discovery.enabled.len(), 1);
        assert!(listener.pending_rpcs.is_empty());
        listener.pending_entries.clear();
        listener.advance_event_discovery().unwrap();
        assert!(listener.event_discovery.enabled.is_empty());
        let HostRpc::GameMode(call) = listener.pending_rpcs.pop_front().unwrap() else {
            panic!("notification")
        };
        assert_eq!(
            (call.target, call.selector),
            listener
                .players
                .objects()
                .scene_endpoint(fixture::RACE, 6)
                .unwrap()
        );
        assert_eq!(call.method, 1);
        assert_eq!(call.reference, None);
        listener.advance_event_discovery().unwrap();
        assert!(listener.pending_rpcs.is_empty());
        // Unrelated recurring output must not starve map updates. Conversely,
        // reserve queue space for race/presentation calls near the queue bound.
        listener.event_discovery.enabled.insert(Site {
            scene_key: fixture::RACE,
            serializer: 6,
        });
        listener.pending_rpcs.push_back(HostRpc::GameMode(call));
        listener.advance_event_discovery().unwrap();
        assert!(listener.event_discovery.enabled.is_empty());
        assert_eq!(listener.pending_rpcs.len(), 2);
        listener.pending_rpcs =
            std::iter::repeat_n(HostRpc::GameMode(call), nfs_world::session::MAX_RPC_QUEUE)
                .collect();
        listener.event_discovery.enabled.insert(Site {
            scene_key: fixture::RACE,
            serializer: 6,
        });
        listener.advance_event_discovery().unwrap();
        assert_eq!(listener.event_discovery.enabled.len(), 1);
        assert_eq!(
            listener.pending_rpcs.len(),
            nfs_world::session::MAX_RPC_QUEUE
        );
    }
}
