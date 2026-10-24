// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded, per-connection map membership. No start-volume subscriptions or
//! prompts are created for game modes whose playable lifecycle is not owned yet.
use super::*;
use nfs_content::events::discovery::Endpoint as Site;

const BATCH: usize = 16;

#[derive(Clone, Debug, Default)]
pub(in crate::net::players) struct State {
    enabled: BTreeSet<Site>,
}
impl State {
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
    pub(super) fn advance_event_discovery(&mut self) -> Result<(), replication::Error> {
        // Race/presentation output gets priority. A full queue defers work without
        // marking membership published or refusing incoming client frames.
        if !self.pending_entries.is_empty() {
            return Ok(());
        }
        let budget = nfs_world::session::MAX_RPC_QUEUE
            .saturating_sub(self.pending_rpcs.len())
            .saturating_sub(BATCH);
        let present = !self.event_state.failed
            && (self
                .participants
                .in_free_roam()
                .iter()
                .any(|p| self.free_roam_presented.contains(p))
                || self.crashing_event_participants().next().is_some()
                || self.event_state.restart_resuming().next().is_some());
        let empty = BTreeSet::new();
        let wanted = self
            .events
            .as_ref()
            .filter(|_| present)
            .map_or(&empty, |c| c.discovery_notifications());
        let changes = self.event_discovery.plan(wanted, budget);
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
