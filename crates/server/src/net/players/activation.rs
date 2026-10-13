// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

impl PlayerListener {
    /// Membership follows the current owned lifecycle. Keep it behind all
    /// completion creations/binding refreshes and retain it under backpressure.
    pub(super) fn advance_world_activation(&mut self) -> Result<(), replication::Error> {
        if !self.pending_entries.is_empty()
            || !self.pending.is_empty()
            || !self.pending_binding_refreshes.is_empty()
        {
            return Ok(());
        }
        // The official host deactivates in two parts: streamed-leave, presence
        // off and 33 volumes at the return start (E742 seg01 168,129, seg03
        // 281,710), the remaining 100 world-sublevel volumes after the
        // finishing chain with the assignment clear (175,920; 289,326). The
        // whole batch leaves at the return start here; the late part is a
        // content split still to be modeled (E849).
        let active: BTreeSet<_> = self.participants.in_free_roam().into_iter().collect();
        let targets: BTreeSet<_> = self
            .world_activation
            .as_ref()
            .into_iter()
            .flat_map(|a| a.participants())
            .chain(active.iter().copied())
            .collect();
        for participant in targets {
            // A deactivated participant with a spawn assignment also gets the
            // assignment clear and at most one occupied-flag change.
            let clearing =
                !active.contains(&participant) && self.spawn_points.assigned(participant).is_some();
            let capacity = nfs_world::session::MAX_RPC_QUEUE
                .saturating_sub(self.pending_rpcs.len())
                .saturating_sub(if clearing { 2 } else { 0 });
            let presenting =
                active.contains(&participant) && !self.free_roam_presented.contains(&participant);
            // AliveState and the driving context must have reached the client
            // before the garage-zone notification can trigger its prompt.
            // Prepare this on a clone so queue pressure retains the gate.
            let mut spawn_points = self.spawn_points.clone();
            let spawn_presence = if presenting && spawn_points.assigned(participant).is_some() {
                spawn_points.set_presentation_ready(participant, true)?
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
            if presenting
                && self
                    .world_activation
                    .as_ref()
                    .is_some_and(|a| !a.participants().any(|id| id == participant))
            {
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
                    // The official host clears the SpawnPoints assignment
                    // ahead of the participant-scoped leave batch (E742).
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
                self.pending_rpcs
                    .extend(spawn_presence.map(nfs_world::participants::HostRpc::SpawnOccupied));
                self.spawn_points = spawn_points;
                self.free_roam_presented.insert(participant);
            }
        }
        self.free_roam_presented.retain(|p| active.contains(p));
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
        let mut payload = BitWriter::new();
        payload
            .put(assign.endpoint.selector.into(), 9)
            .put(nfs_world::spawn_points::REQUEST.into(), 32)
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
        assert_eq!(
            listener.spawn_points.receive(crossing.span(), |_| true),
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
    }
}
