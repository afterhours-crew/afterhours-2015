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
        let Some(activation) = &mut self.world_activation else {
            return Ok(());
        };
        let active: BTreeSet<_> = self.participants.in_free_roam().into_iter().collect();
        let targets: BTreeSet<_> = activation
            .participants()
            .chain(active.iter().copied())
            .collect();
        for participant in targets {
            let capacity =
                nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len());
            let replies =
                activation.set(participant, active.contains(&participant), capacity, |id| {
                    self.players
                        .owns_participant(HOST_SELECTOR as u8, self.persona, id)
                })?;
            if !replies.is_empty() {
                tracing::info!(
                    participant,
                    active = active.contains(&participant),
                    notifications = replies.len(),
                    "owned world participant activation queued"
                );
                self.pending_rpcs.extend(
                    replies
                        .into_iter()
                        .map(nfs_world::participants::HostRpc::WorldActivation),
                );
            }
        }
        Ok(())
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
            .create_scenes(&[1, 2, 3], creations, &profiles)
            .unwrap();
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
            .resize(nfs_world::session::MAX_RPC_QUEUE - 2, occupied);
        let before = listener.world_activation.clone();
        listener.advance_world_activation().unwrap();
        assert_eq!(listener.world_activation, before);
        listener.pending_rpcs.clear();
        listener.advance_world_activation().unwrap();
        assert_eq!(listener.pending_rpcs.len(), 3);
        assert!(
            listener
                .pending_rpcs
                .iter()
                .all(|r| matches!(r, nfs_world::participants::HostRpc::WorldActivation(_)))
        );
        listener.advance_world_activation().unwrap();
        assert_eq!(listener.pending_rpcs.len(), 3);
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
