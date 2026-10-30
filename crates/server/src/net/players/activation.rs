// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

/// Longest wait for a held Alive (the event lane releases it after its
/// subscriptions, official E912 1.29 s).
const HELD_ALIVE_LIMIT_MS: u64 = 5_000;
/// Longest wait of a released participant's presentation for its world
/// memberships (official 0.2-2.1 s after Alive, E912 and E1026).
const PRESENTATION_LIMIT_MS: u64 = 3_000;

/// Whether a held Alive goes now: at once without an event lane (nothing
/// will subscribe the participant), after [`HELD_ALIVE_LIMIT_MS`] with one.
fn held_alive_due(lane: bool, held_ms: u64, now_ms: u64) -> bool {
    !lane || now_ms >= held_ms.saturating_add(HELD_ALIVE_LIMIT_MS)
}
/// Whether a participant's free-roam presentation goes now: at once unless
/// its Alive was held, then once its memberships are out or after
/// [`PRESENTATION_LIMIT_MS`].
fn presentation_ready(released_ms: Option<u64>, settled: bool, now_ms: u64) -> bool {
    released_ms.is_none_or(|t| settled || now_ms >= t.saturating_add(PRESENTATION_LIMIT_MS))
}

impl PlayerListener {
    /// A held Alive goes at once without an event lane, and after
    /// [`HELD_ALIVE_LIMIT_MS`] in any case.
    pub(super) fn release_held_alive(&mut self) {
        let lane = self.events.is_some() && !self.event_state.failed;
        let due: Vec<u16> = self
            .held_alive
            .iter()
            .filter(|(_, (_, held))| held_alive_due(lane, *held, self.world_ms))
            .map(|(&p, _)| p)
            .collect();
        for participant in due {
            if lane {
                tracing::warn!(
                    participant,
                    "held Alive released without the event subscriptions"
                );
            }
            let (rpc, _) = self.held_alive.remove(&participant).expect("listed");
            self.pending_rpcs.push_back(rpc);
            self.alive_released_ms.insert(participant, self.world_ms);
        }
    }
    /// A released participant presents once its world memberships are out
    /// (official E912: last membership 77.537 s, presentation 78.037 s), or
    /// after [`PRESENTATION_LIMIT_MS`]; any other entry presents at once.
    fn presentation_due(&self, participant: u16) -> bool {
        let released = self.alive_released_ms.get(&participant).copied();
        if released.is_none() {
            return true;
        }
        let world = self.players.objects();
        let settled = self.prologue.as_ref().is_none_or(|content| {
            self.world_volumes
                .settled(participant, &content.volumes, |site| {
                    world
                        .scene_endpoint(site.scene_key, site.serializer)
                        .is_some()
                })
        });
        presentation_ready(released, settled, self.world_ms)
    }
    /// Membership follows the current owned lifecycle. Keep it behind all
    /// completion creations/binding refreshes and retain it under backpressure.
    pub(super) fn advance_world_activation(&mut self) -> Result<(), replication::Error> {
        if !self.pending_entries.is_empty()
            || !self.pending.is_empty()
            || !self.pending_binding_refreshes.is_empty()
        {
            return Ok(());
        }
        self.release_held_alive();
        // Deactivation currently retires all configured volumes at return
        // start. Separate later sublevel retirement is not modeled here.
        let active: BTreeSet<_> = self.participants.in_free_roam().into_iter().collect();
        let targets: BTreeSet<_> = self
            .world_activation
            .as_ref()
            .into_iter()
            .flat_map(|a| a.participants())
            .chain(active.iter().copied())
            .collect();
        for participant in targets {
            // A held Alive keeps the whole activation for a later pass; the
            // world activation itself goes with Alive (official E912
            // 76.538 s), only the presentation waits for the memberships.
            if self.held_alive.contains_key(&participant) {
                continue;
            }
            // The garage volume subscription survives a crash. Only its
            // presentation is suspended until the driving context returns.
            let crashing = self.participants.stage(participant)
                == Some(nfs_world::participants::Stage::Crashing);
            // The intro keeps its assignment through the return cinematic,
            // whose garage-zone report needs it; the garage entry clears it.
            let clearing = !active.contains(&participant)
                && !crashing
                && self.participants.intro_chapter(participant).is_none()
                && self.spawn_points.assigned(participant).is_some();
            let capacity = nfs_world::session::MAX_RPC_QUEUE
                .saturating_sub(self.pending_rpcs.len())
                .saturating_sub(if clearing { 2 } else { 0 });
            // Scripted intro driving shares world membership, but owns its
            // presentation through the authored story sequences. Its objects
            // are not the ordinary garage-exit presentation retained here.
            let presenting = active.contains(&participant)
                && self.participants.intro_chapter(participant).is_none()
                && !self.free_roam_presented.contains(&participant)
                && self.presentation_due(participant);
            // AliveState and the driving context must have reached the client
            // before the garage-zone notification can trigger its prompt.
            // Prepare this on a clone so queue pressure retains the gate.
            let mut spawn_points = self.spawn_points.clone();
            let spawn_presence =
                if (presenting || crashing) && spawn_points.assigned(participant).is_some() {
                    spawn_points.set_presentation_ready(participant, !crashing)?
                } else {
                    None
                };
            if presenting && self.free_roam_presented.len() >= MAX_WORLD_CARS {
                return Err(replication::Error::Bound);
            }
            let events = if presenting {
                Self::presentation_events(
                    self.logic.as_deref(),
                    crate::garage_logic::Which::FreeRoam,
                    &self.players,
                    &self.logic_ghosts,
                    self.persona,
                    participant,
                )?
            } else {
                Vec::new()
            };
            let presentation_count = events.len() + usize::from(spawn_presence.is_some());
            if presentation_count > capacity {
                break;
            }
            let replies = match self.world_activation.as_mut() {
                Some(activation) => activation.set(
                    participant,
                    active.contains(&participant),
                    capacity - presentation_count,
                    |id| {
                        self.players
                            .owns_participant(HOST_SELECTOR as u8, self.persona, id)
                    },
                )?,
                None => Vec::new(),
            };
            if self.world_activation.as_ref().is_some_and(|a| {
                a.participants().any(|id| id == participant) != active.contains(&participant)
            }) {
                // Activation could not reserve its complete output. Do not send
                // the presentation release or consume its once-only marker.
                continue;
            }
            if !replies.is_empty() {
                tracing::info!(
                    participant,
                    active = active.contains(&participant),
                    notifications = replies.len(),
                    "owned world participant activation queued"
                );
                if clearing {
                    // Clear the spawn subscription before the leave batch.
                    let clear = self.spawn_clear(participant)?;
                    self.pending_rpcs.extend(clear);
                }
                self.pending_rpcs.extend(
                    replies
                        .into_iter()
                        .map(nfs_world::participants::HostRpc::WorldActivation),
                );
            }
            if presenting {
                self.pending_rpcs.extend(events);
                self.free_roam_presented.insert(participant);
                self.alive_released_ms.remove(&participant);
            }
            if presenting || crashing {
                self.pending_rpcs
                    .extend(spawn_presence.map(nfs_world::participants::HostRpc::SpawnOccupied));
                self.spawn_points = spawn_points;
            }
        }
        self.free_roam_presented.retain(|p| active.contains(p));
        self.alive_released_ms.retain(|p, _| active.contains(p));
        Ok(())
    }
    /// The spawn-assignment clear for a participant leaving free roam, with
    /// the occupied-flag change it implies; empty without an assignment.
    pub(super) fn spawn_clear(
        &mut self,
        participant: u16,
    ) -> Result<Vec<nfs_world::participants::HostRpc>, replication::Error> {
        if self.spawn_points.assigned(participant).is_none() {
            return Ok(Vec::new());
        }
        let Some((notification, flag)) = self.spawn_points.clear(participant)? else {
            return Ok(Vec::new());
        };
        tracing::info!(
            participant,
            flag = ?flag,
            "owned spawn assignment cleared for the garage"
        );
        let mut rpcs = vec![nfs_world::participants::HostRpc::Participant(notification)];
        rpcs.extend(flag.map(nfs_world::participants::HostRpc::SpawnOccupied));
        Ok(rpcs)
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;
    use nfs_world::participants::{Call, Endpoint, HostRpc, Notification};

    #[test]
    fn a_held_alive_waits_for_the_event_lane_within_a_bound() {
        assert!(held_alive_due(false, 100, 100), "no lane: at once");
        assert!(!held_alive_due(true, 100, 100 + HELD_ALIVE_LIMIT_MS - 1));
        assert!(held_alive_due(true, 100, 100 + HELD_ALIVE_LIMIT_MS));
        let mut listener = PlayerListener::new(101);
        let alive = HostRpc::Participant(Notification {
            endpoint: Endpoint {
                scene: 2,
                selector: 1,
                serial: nfs_protocol::world::rpc::Serial::new(0).unwrap(),
            },
            participant: 7,
            call: Call::Enter,
        });
        listener.held_alive.insert(7, (alive, 40));
        listener.world_ms = 90;
        listener.release_held_alive();
        assert!(listener.held_alive.is_empty());
        assert_eq!(listener.pending_rpcs.iter().collect::<Vec<_>>(), [&alive]);
        assert_eq!(listener.alive_released_ms.get(&7), Some(&90));
    }

    #[test]
    fn a_released_participant_presents_after_its_memberships_or_the_bound() {
        assert!(presentation_ready(None, false, 0), "not held: at once");
        assert!(!presentation_ready(Some(1_000), false, 1_000));
        assert!(presentation_ready(Some(1_000), true, 1_000));
        assert!(!presentation_ready(
            Some(1_000),
            false,
            1_000 + PRESENTATION_LIMIT_MS - 1
        ));
        assert!(presentation_ready(
            Some(1_000),
            false,
            1_000 + PRESENTATION_LIMIT_MS
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_protocol::world::rpc::Serial;
    use nfs_world::{
        bits::BitWriter,
        participants::{Bindings, Outcome},
        replication::sublevel::{Content, Kind, Profile, ordinary},
        world_activation::{Activation, Catalog, Site},
    };

    #[test]
    fn activation_follows_complete_lifecycle_and_waits_for_presentation_and_capacity() {
        let mut startup = vec![Kind::RpcReferences; 117];
        startup[0] = Kind::Noop;
        let mut garage = vec![Kind::Rpc; 28];
        garage[0] = Kind::Noop;
        let profiles = Content::new(vec![
            (
                501,
                nfs_world::replication::sublevel::gameplay::profile().unwrap(),
            ),
            (502, Profile::new(false, &startup).unwrap()),
            (503, Profile::new(false, &garage).unwrap()),
            (
                504,
                Profile::new(false, &[Kind::Noop, Kind::Rpc, Kind::RpcBool]).unwrap(),
            ),
        ])
        .unwrap();
        let mut listener = PlayerListener::new(101);
        let creations = profiles
            .profiles()
            .enumerate()
            .map(|(i, (key, p))| {
                ordinary::creation(
                    (i + 1) as u16,
                    key,
                    None,
                    Serial::new(0).unwrap(),
                    &ordinary::unpopulated(p).unwrap(),
                    &profiles,
                )
                .unwrap()
            })
            .collect();
        let records = listener
            .players
            .create_scenes(&[1, 2, 3, 4], creations, &profiles)
            .unwrap();
        listener.spawn_points.bind(&records, 504).unwrap();
        listener.participants.bind(&records, 501, 502).unwrap();
        listener.participants.bind_exit(&records, 502, 503).unwrap();
        listener.participants.bind_return(&records, 502).unwrap();
        let site = |serializer| Site {
            scene_key: 501,
            serializer,
        };
        listener.world_activation = Some(
            Activation::bind(
                &records,
                &Catalog {
                    volumes: vec![site(5)],
                    presence: site(19),
                    streamed: site(47),
                    location: site(28),
                },
            )
            .unwrap(),
        );
        let player = listener
            .players
            .create(
                HOST_SELECTOR as u8,
                101,
                Request {
                    name: b"Driver".to_vec(),
                    flag: false,
                    slot: 0,
                },
            )
            .unwrap()
            .unwrap()
            .id;
        let participant = listener
            .players
            .join(HOST_SELECTOR as u8, 101, player)
            .unwrap()
            .unwrap()
            .id;
        let assign = listener.spawn_points.assign(participant, 1).unwrap();
        listener
            .spawn_points
            .set_presentation_ready(participant, false)
            .unwrap();
        let crossing = |method: u32| {
            let mut payload = BitWriter::new();
            payload
                .put(assign.endpoint.selector.into(), 9)
                .put(method.into(), 32)
                .put(1.0f64.to_bits(), 64)
                .put(1, 32)
                .align();
            let mut crossing = BitWriter::new();
            crossing
                .put(0, 32)
                .put(0, 32)
                .put(2, 8)
                .put(assign.endpoint.scene.into(), 13)
                .put(participant.into(), 13)
                .put(payload.bytes().len() as u64, 9)
                .put_span(payload.span());
            crossing
        };
        assert_eq!(
            listener
                .spawn_points
                .receive(crossing(nfs_world::spawn_points::REQUEST).span(), |_| true),
            Ok(Some(None))
        );
        let mut presentation = crate::garage_logic::tests::presentation_json();
        presentation["free_roam"]["events"][0]["target"] =
            serde_json::json!({"scene_content_key":501});
        listener.logic = Some(std::sync::Arc::new(
            crate::garage_logic::GarageLogic::from_json(&presentation).unwrap(),
        ));
        let bindings = Bindings::from_records(&records, 501, 502).unwrap().unwrap();
        let request = |endpoint: nfs_world::participants::Endpoint, method: u64| {
            let mut payload = BitWriter::new();
            payload
                .put(endpoint.selector.into(), 9)
                .put(method, 32)
                .align();
            let mut body = BitWriter::new();
            body.put(0, 32)
                .put(0, 32)
                .put(2, 8)
                .put(endpoint.scene.into(), 13)
                .put(participant.into(), 13)
                .put(payload.bytes().len() as u64, 9)
                .put_span(payload.span());
            body
        };
        listener.participants.begin(participant).unwrap();
        listener.participants.set_garage([None; 5]).unwrap();
        listener
            .participants
            .receive(request(bindings.inventory, 3).span(), |_| true)
            .unwrap();
        listener.participants.take_garage_bindings();
        listener.participants.finish_persistent(|_| true);
        assert!(listener.participants.begin_progression(participant));
        listener.participants.enter_garage_loading(participant);
        assert!(matches!(
            listener.participants.enter_customization(participant),
            Outcome::Advanced(_)
        ));
        listener.advance_world_activation().unwrap();
        assert!(listener.pending_rpcs.is_empty());
        let exit = listener.participants.exit_bindings().unwrap();
        for (e, m) in [
            (exit.exit_request, 0),
            (exit.state_77, 3),
            (exit.ready_request, 0),
        ] {
            assert!(matches!(
                listener
                    .participants
                    .receive(request(e, m).span(), |_| true)
                    .unwrap(),
                Outcome::Advanced(_)
            ));
            listener.advance_world_activation().unwrap();
            assert!(listener.pending_rpcs.is_empty());
        }
        assert!(matches!(
            listener.participants.arrived(participant),
            Outcome::Advanced(_)
        ));
        let section = Section {
            float_bits: None,
            flag: false,
            deleted: vec![],
            setup: None,
            records: vec![],
        };
        listener
            .pending_entries
            .push_back((section.clone(), vec![]));
        listener.advance_world_activation().unwrap();
        assert!(listener.pending_rpcs.is_empty());
        listener.pending_entries.clear();
        listener.pending.push_back(section);
        listener.advance_world_activation().unwrap();
        assert!(listener.pending_rpcs.is_empty());
        listener.pending.clear();
        listener
            .pending_binding_refreshes
            .push_back((100, participant));
        listener.advance_world_activation().unwrap();
        assert!(listener.pending_rpcs.is_empty());
        listener.pending_binding_refreshes.clear();
        let occupied =
            nfs_world::participants::HostRpc::Participant(nfs_world::participants::Notification {
                endpoint: exit.state_2,
                participant,
                call: nfs_world::participants::Call::Enter,
            });
        listener
            .pending_rpcs
            .resize(nfs_world::session::MAX_RPC_QUEUE - 4, occupied);
        let before = listener.world_activation.clone();
        let before_spawn = listener.spawn_points.clone();
        listener.advance_world_activation().unwrap();
        assert_eq!(listener.world_activation, before);
        assert_eq!(listener.spawn_points, before_spawn);
        assert!(listener.free_roam_presented.is_empty());
        listener.pending_rpcs.clear();
        listener.advance_world_activation().unwrap();
        assert_eq!(listener.pending_rpcs.len(), 5);
        assert!(
            listener
                .pending_rpcs
                .iter()
                .take(3)
                .all(|r| matches!(r, nfs_world::participants::HostRpc::WorldActivation(_)))
        );
        assert!(
            matches!(listener.pending_rpcs.get(3),Some(nfs_world::participants::HostRpc::Event(e)) if e.event == 17)
        );
        assert!(
            matches!(listener.pending_rpcs.back(),Some(nfs_world::participants::HostRpc::SpawnOccupied(f)) if f.enabled)
        );
        listener.advance_world_activation().unwrap();
        assert_eq!(listener.pending_rpcs.len(), 5);
        listener.pending_rpcs.clear();
        listener.advance_world_activation().unwrap();
        assert!(listener.pending_rpcs.is_empty());
        assert_eq!(
            listener
                .world_activation
                .as_ref()
                .unwrap()
                .participants()
                .collect::<Vec<_>>(),
            vec![participant]
        );
        // A crash temporarily removes the driving context, but is not a
        // garage return. Keep the volume subscription and token through three
        // recoveries, including occupancy changes while presentation is hidden.
        for _ in 0..3 {
            listener.participants.crashed(participant).unwrap();
            let before = listener.spawn_points.clone();
            listener.pending_rpcs = std::iter::repeat_n(
                nfs_world::participants::HostRpc::Participant(assign),
                nfs_world::session::MAX_RPC_QUEUE - 3,
            )
            .collect();
            listener.advance_world_activation().unwrap();
            assert_eq!(listener.spawn_points, before);
            listener.pending_rpcs.clear();
            listener.advance_world_activation().unwrap();
            assert_eq!(listener.spawn_points.assigned(participant), Some(1));
            assert!(!listener.pending_rpcs.iter().any(|r| matches!(
                r, nfs_world::participants::HostRpc::Participant(n)
                    if n.call == nfs_world::participants::Call::Leave
            )));
            assert!(matches!(
                listener.pending_rpcs.back(),
                Some(nfs_world::participants::HostRpc::SpawnOccupied(f)) if !f.enabled
            ));
            listener.pending_rpcs.clear();
            // Repeated callbacks are still accepted under the same token;
            // their notification remains hidden until driving is restored.
            for method in [
                nfs_world::spawn_points::RELEASE,
                nfs_world::spawn_points::REQUEST,
            ] {
                assert_eq!(
                    listener
                        .spawn_points
                        .receive(crossing(method).span(), |_| true),
                    Ok(Some(None))
                );
            }
            listener.participants.recovered(participant).unwrap();
            listener.advance_world_activation().unwrap();
            assert!(matches!(
                listener.pending_rpcs.back(),
                Some(nfs_world::participants::HostRpc::SpawnOccupied(f)) if f.enabled
            ));
            listener.pending_rpcs.clear();
        }
        let returning = nfs_world::participants::ReturnBindings::from_records(&records, 502)
            .unwrap()
            .unwrap();
        for _ in 0..2 {
            assert!(matches!(
                listener
                    .participants
                    .receive(request(returning.request, 0).span(), |_| true),
                Ok(Outcome::Advanced(_))
            ));
        }
        listener.advance_world_activation().unwrap();
        assert!(listener.pending_rpcs.iter().any(|r| matches!(
            r, nfs_world::participants::HostRpc::Participant(n)
                if n.call == nfs_world::participants::Call::Leave && n.endpoint == assign.endpoint
        )));
        listener.pending_rpcs.clear();
        listener.advance_world_activation().unwrap();
        assert!(listener.pending_rpcs.is_empty());
    }
}
