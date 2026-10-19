// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::rival_content::population::{Focus, Input};
use nfs_world::application::Random;

impl PlayerListener {
    pub(super) fn rival_glass_owner(
        &self,
        players: &Players,
        vehicle: u16,
        local_slot: u8,
    ) -> Option<crate::glass::Owner> {
        let connection = HOST_SELECTOR as u8;
        let delegated =
            self.rival_population
                .as_ref()?
                .delegated(players, connection, self.persona);
        let ids = players.ai_ids(connection, self.persona, *delegated.get(&vehicle)?)?;
        Some(crate::glass::Owner {
            vehicle,
            participant: ids.participant,
            local_slot,
            runtime_index: players.local_runtime_index(connection, self.persona, local_slot)?,
        })
    }

    /// Entries for the rival map: every active racer, driven by the client or
    /// by the host. Frame records drain before queued rival entries, so the
    /// map is left unchanged (None) until a new participant's creation has
    /// been handed to the host.
    pub(super) fn rival_map(
        &self,
        players: &Players,
    ) -> Result<Option<Vec<crate::rival_content::population::Location>>, replication::Error> {
        if !self.pending_entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            self.rival_population
                .as_ref()
                .map(|r| r.locations(players, HOST_SELECTOR as u8, self.persona))
                .transpose()?
                .unwrap_or_default(),
        ))
    }

    pub(in crate::net) fn advance_rivals(&mut self, now: u64) -> Result<(), replication::Error> {
        if self.ai_busy() {
            return Ok(());
        }
        let Some(rivals) = &self.rival_population else {
            return Ok(());
        };
        let mut rivals = rivals.clone();
        let mut population = self.population.clone().ok_or(replication::Error::Shape)?;
        let previous = population.registrations().clone();
        let mut registrations = previous.clone();
        let focus = if self.rivals_failed {
            vec![]
        } else {
            self.ai_focus()?
                .into_iter()
                .map(|f| Focus {
                    id: f.id,
                    position: f.position,
                })
                .collect()
        };
        let mut players = self.players.clone();
        let changes = rivals.advance(
            &mut players,
            &mut registrations,
            HOST_SELECTOR as u8,
            self.persona,
            Input {
                now,
                focus: &focus,
                random: crate::seeds::OsRandom.next_u32(),
            },
        )?;
        let mut entries: Vec<_> = split_scenes(
            changes.records,
            nfs_world::application::OUTBOUND_FRAME_BITS - 104,
        )?
        .into_iter()
        .map(|mut section| {
            section.setup = Some(Setup::RawEscape([0; 3]));
            (section, Vec::new())
        })
        .collect();
        entries.splice(0..0, super::traffic::ai_departures(&changes.leave));
        if !changes.calls.is_empty() || !changes.deleted.is_empty() {
            let mut section = returning::empty_section();
            section.deleted = changes.deleted;
            entries.push((section, changes.calls));
        }
        if entries.len() > MAX_PENDING.saturating_sub(self.pending_entries.len())
            || changes.messages.len() > MAX_PENDING.saturating_sub(self.pending_content.len())
        {
            return Ok(());
        }
        population.update_registrations(&previous, registrations)?;
        if let Some(ids) = changes.activated {
            tracing::info!(
                participant = ids.participant,
                actor = ids.actor,
                count = rivals.count(),
                "owned roaming rival activated"
            );
        }
        if let Some(ids) = changes.spawned {
            tracing::info!(
                participant = ids.participant,
                actor = ids.actor,
                vehicle = ids.vehicle,
                count = rivals.count(),
                "owned roaming rival car handed to the client"
            );
        }
        self.players = players;
        self.population = Some(population);
        self.rival_population = Some(rivals);
        // The edge drains content before these creation sections, so the client
        // knows every selected part's mesh identity before it constructs the car.
        self.pending_content.extend(changes.messages);
        self.pending_entries.extend(entries);
        Ok(())
    }
    pub(in crate::net) fn disable_rivals(&mut self) -> bool {
        !std::mem::replace(&mut self.rivals_failed, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delegated_callbacks_commit_atomically_with_motion_and_glass() {
        use nfs_world::{
            bits::BitWriter,
            logic::{EntityRef, Message},
            replication::{
                Initial, Update,
                vehicle::{self, chassis, movement::Sample},
            },
        };
        let connection = HOST_SELECTOR as u8;
        let (mut rivals, mut world, mut registrations, _) =
            crate::rival_content::population::tests::fixture(connection);
        let ids = crate::rival_content::population::tests::spawn(
            &mut rivals,
            &mut world,
            &mut registrations,
            connection,
        )
        .spawned
        .unwrap();
        let key = crate::traffic_content::population::RIVAL_KEY_START;
        let Initial::Vehicle { creation, .. } = &world.objects().get(ids.vehicle).unwrap().initial
        else {
            panic!()
        };
        let horn_selector = creation
            .fields
            .iter()
            .find_map(|f| match f {
                vehicle::Initial::Chassis(c) => Some(c.rpc.selector),
                _ => None,
            })
            .unwrap();
        let nos_selector = creation
            .fields
            .iter()
            .find_map(|f| match f {
                vehicle::Initial::Index { rpc, .. } => Some(rpc.selector),
                _ => None,
            })
            .unwrap();
        let wheel_selector = creation
            .fields
            .iter()
            .find_map(|f| match f {
                vehicle::Initial::Wheel { rpc, .. } => Some(rpc.selector),
                _ => None,
            })
            .unwrap();
        // Driving callbacks use the same ordinary component endpoints as owned cars.
        let rpc = |selector: u16, method: u32, value: u64, bits: usize| {
            let mut payload = BitWriter::new();
            payload
                .put(selector.into(), 9)
                .put(method.into(), 32)
                .put(value, bits)
                .put(0, if bits == 1 { 6 } else { 7 });
            let mut body = BitWriter::new();
            body.put(0, 32)
                .put(0, 32)
                .put(1, 8)
                .put(ids.vehicle.into(), 13)
                .put((payload.len() / 8) as u64, 9)
                .put_span(payload.span());
            body
        };
        let before = world.ai_vehicle_physics(connection, 100, key).unwrap();
        let mut motion = before.decode([0.; 3]).unwrap();
        motion.identity = 100f64.to_bits();
        motion.position[0] += 10.;
        motion.velocity = [5., 0., 0.];
        let sample = Sample {
            physics: motion
                .encode([0.; 3], &chassis::Controls::new(4).unwrap())
                .unwrap(),
            flag: false,
        };
        let (header, movement) = sample.encode().unwrap();
        let make_frame = |sequence, active: bool, invalid: bool| {
            let calls = [
                rpc(horn_selector, 0, u64::from(active), 1),
                rpc(nos_selector, u32::from(!active), 10f32.to_bits().into(), 32),
                rpc(wheel_selector, u32::from(!active), 0, 1),
                rpc(
                    horn_selector,
                    2,
                    if invalid { f32::NAN } else { 8.75f32 }.to_bits().into(),
                    32,
                ),
            ];
            let glass = Message::Reached {
                event: 28_404_286,
                target: EntityRef {
                    ghost: ids.vehicle,
                    entity: 2,
                },
                player: 0,
            }
            .encode()
            .unwrap();
            let mut messages: Vec<_> = calls
                .iter()
                .map(|b| frame::Message::Opaque {
                    index: 47,
                    body: b.span(),
                })
                .collect();
            messages.push(frame::Message::Opaque {
                index: 82,
                body: glass.span(),
            });
            let messages = frame::Builder::new().messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(sequence),
                    messages,
                }],
            );
            let mut wire = BitWriter::new();
            wire.put((1 << frame::MESSAGES) | (1 << frame::MOVEMENT), 6);
            messages.append(&mut wire).unwrap();
            wire.put_span(header.span())
                .put(1, 6)
                .put(ids.vehicle.into(), 16)
                .put(0, 4)
                .put(movement.len() as u64, 16)
                .put_span(movement.span())
                .align();
            wire
        };
        let deliver = |listener: &mut PlayerListener, wire: &BitWriter| {
            listener.frame(Queued {
                advertised: None,
                frame: 0,
                data: wire.span(),
            })
        };
        let mut listener = PlayerListener::new(100);
        listener.players = world;
        listener.rival_population = Some(rivals);
        let snapshot = listener.players.objects().snapshot();
        // The final malformed RPC rolls back valid horn, nitrous, glass and motion.
        assert!(!deliver(&mut listener, &make_frame(0, true, true)));
        assert_eq!(listener.players.objects().snapshot(), snapshot);
        assert!(listener.pending.is_empty() && listener.pending_rpcs.is_empty());
        assert!(listener.glass.state(ids.vehicle).is_none());
        let valid = make_frame(0, true, false);
        listener
            .pending
            .resize(MAX_PENDING, returning::empty_section());
        assert!(!deliver(&mut listener, &valid));
        assert_eq!(listener.players.objects().snapshot(), snapshot);
        assert!(listener.glass.state(ids.vehicle).is_none());
        listener.pending.clear();
        assert!(deliver(&mut listener, &valid));
        assert_eq!(
            listener
                .players
                .ai_vehicle_physics(connection, 100, key)
                .unwrap(),
            sample.physics
        );
        assert_eq!(listener.pending_rpcs.len(), 1);
        assert_eq!(listener.unsupported_rpcs, 0);
        let Update::Vehicle { fields, .. } =
            &listener.players.objects().get(ids.vehicle).unwrap().current
        else {
            panic!()
        };
        assert!(
            fields
                .iter()
                .any(|f| matches!(f, Some(vehicle::Update::Chassis(c)) if c.flag == Some(true)))
        );
        assert!(
            fields
                .iter()
                .any(|f| matches!(f, Some(vehicle::Update::Index { index: Some(1), .. })))
        );
        assert!(listener.world_cars.is_empty() && listener.scoring_state.nitrous.is_empty());
        assert_eq!(
            fields
                .iter()
                .filter(|f| matches!(f, Some(vehicle::Update::Wheel(Some(true)))))
                .count(),
            1
        );
        let committed = listener.players.objects().snapshot();
        listener.pending.clear();
        listener.pending_rpcs.clear();
        // A repeated reliable frame must not repeat events or change state.
        assert!(deliver(&mut listener, &valid));
        assert_eq!(listener.players.objects().snapshot(), committed);
        assert!(listener.pending.is_empty() && listener.pending_rpcs.is_empty());
        // Five messages consumed sequence indices 0..4; the next frame turns all off.
        assert!(deliver(&mut listener, &make_frame(5, false, false)));
        let Update::Vehicle { fields, .. } =
            &listener.players.objects().get(ids.vehicle).unwrap().current
        else {
            panic!()
        };
        assert!(
            fields
                .iter()
                .any(|f| matches!(f, Some(vehicle::Update::Chassis(c)) if c.flag == Some(false)))
        );
        assert!(
            fields
                .iter()
                .any(|f| matches!(f, Some(vehicle::Update::Index { index: Some(0), .. })))
        );
        listener
            .players
            .remove_ai_vehicle(connection, 100, key)
            .unwrap();
        assert!(
            !listener
                .delegated_ai(&listener.players)
                .contains_key(&ids.vehicle)
        );
    }

    #[test]
    fn rival_glass_reports_commit_with_movement_and_reliable_delivery() {
        use nfs_world::{
            logic::{EntityRef, Message},
            participants::HostRpc,
            replication::vehicle::{chassis, movement::Sample},
        };
        let connection = HOST_SELECTOR as u8;
        let (mut rivals, mut world, mut registrations, _) =
            crate::rival_content::population::tests::fixture(connection);
        let ids = crate::rival_content::population::tests::spawn(
            &mut rivals,
            &mut world,
            &mut registrations,
            connection,
        )
        .spawned
        .unwrap();
        let key = crate::traffic_content::population::RIVAL_KEY_START;
        let before = world.ai_vehicle_physics(connection, 100, key).unwrap();
        let mut motion = before.decode([0.; 3]).unwrap();
        motion.identity = 100f64.to_bits();
        motion.position[0] += 10.;
        motion.velocity = [5., 0., 0.];
        let sample = Sample {
            physics: motion
                .encode([0.; 3], &chassis::Controls::new(4).unwrap())
                .unwrap(),
            flag: false,
        };
        let (header, body) = sample.encode().unwrap();
        let mut listener = PlayerListener::new(100);
        listener.players = world;
        listener.rival_population = Some(rivals);
        assert!(
            listener
                .rival_glass_owner(&listener.players, ids.vehicle, 1)
                .is_none()
        );
        assert!(
            listener
                .rival_glass_owner(&listener.players, ids.actor, 0)
                .is_none()
        );
        listener.persona = 200;
        assert!(
            listener
                .rival_glass_owner(&listener.players, ids.vehicle, 0)
                .is_none()
        );
        listener.persona = 100;
        let make_frame = |sequence, foreign| {
            let mut events = [28_404_286, 14_703_462, 21_578_436]
                .map(|event| {
                    Message::Reached {
                        event,
                        target: EntityRef {
                            ghost: ids.vehicle,
                            entity: 2,
                        },
                        player: 0,
                    }
                    .encode()
                    .unwrap()
                })
                .to_vec();
            if foreign {
                events.push(
                    Message::Reached {
                        event: 28_404_286,
                        target: EntityRef {
                            ghost: ids.actor,
                            entity: 2,
                        },
                        player: 0,
                    }
                    .encode()
                    .unwrap(),
                );
            }
            let messages = frame::Builder::new().messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(sequence),
                    messages: events
                        .iter()
                        .map(|b| frame::Message::Opaque {
                            index: 82,
                            body: b.span(),
                        })
                        .collect(),
                }],
            );
            let mut wire = nfs_world::bits::BitWriter::new();
            wire.put((1 << frame::MESSAGES) | (1 << frame::MOVEMENT), 6);
            messages.append(&mut wire).unwrap();
            wire.put_span(header.span())
                .put(1, 6)
                .put(ids.vehicle.into(), 16)
                .put(0, 4)
                .put(body.len() as u64, 16)
                .put_span(body.span())
                .align();
            wire
        };
        let deliver = |listener: &mut PlayerListener, wire: &nfs_world::bits::BitWriter| {
            listener.frame(Queued {
                advertised: None,
                frame: 0,
                data: wire.span(),
            })
        };
        // A later invalid event must roll back the earlier valid glass and motion.
        assert!(!deliver(&mut listener, &make_frame(0, true)));
        assert!(listener.glass.state(ids.vehicle).is_none());
        assert_eq!(
            listener
                .players
                .ai_vehicle_physics(connection, 100, key)
                .unwrap(),
            before
        );
        assert!(listener.pending_rpcs.is_empty());
        let valid = make_frame(0, false);
        listener.pending_rpcs.resize(
            nfs_world::session::MAX_RPC_QUEUE,
            HostRpc::Event(nfs_world::logic::Fire {
                event: 7,
                player: Some(3),
                target: EntityRef {
                    ghost: ids.vehicle,
                    entity: 2,
                },
            }),
        );
        assert!(!deliver(&mut listener, &valid));
        assert!(listener.glass.state(ids.vehicle).is_none());
        assert_eq!(
            listener
                .players
                .ai_vehicle_physics(connection, 100, key)
                .unwrap(),
            before
        );
        listener.pending_rpcs.clear();
        assert!(deliver(&mut listener, &valid));
        assert_eq!(
            listener
                .players
                .ai_vehicle_physics(connection, 100, key)
                .unwrap(),
            sample.physics
        );
        assert_eq!(
            listener.glass.state(ids.vehicle),
            Some(&[Some(crate::glass::Condition::BelowThreshold); 3])
        );
        assert_eq!(
            listener
                .pending_rpcs
                .iter()
                .filter_map(|rpc| match rpc {
                    HostRpc::Event(fire) => Some((fire.event, fire.player, fire.target)),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [7, 9, 11].map(|event| (
                event,
                Some(3),
                EntityRef {
                    ghost: ids.vehicle,
                    entity: 2
                }
            ))
        );
        assert!(listener.world_cars.is_empty() && listener.pending_spawns.is_empty());
        listener.pending.clear();
        listener.pending_rpcs.clear();
        assert!(deliver(&mut listener, &valid));
        assert!(listener.pending_rpcs.is_empty());
        // A new report fires again; retirement immediately revokes event authority.
        assert!(deliver(&mut listener, &make_frame(3, false)));
        assert_eq!(listener.pending_rpcs.len(), 3);
        listener
            .players
            .remove_ai_vehicle(connection, 100, key)
            .unwrap();
        assert!(
            listener
                .rival_glass_owner(&listener.players, ids.vehicle, 0)
                .is_none()
        );
    }

    #[test]
    fn mixed_traffic_and_rival_reports_keep_both_delegations_and_echo_clocks() {
        use nfs_world::replication::vehicle::{chassis, movement::Sample};
        let connection = HOST_SELECTOR as u8;
        let (mut rivals, mut world, mut registrations, _) =
            crate::rival_content::population::tests::fixture(connection);
        let rival = crate::rival_content::population::tests::spawn(
            &mut rivals,
            &mut world,
            &mut registrations,
            connection,
        )
        .spawned
        .unwrap();
        let (mut traffic, _, content) =
            crate::traffic_content::population::tests::fixture_with(connection);
        let ambient = crate::traffic_content::population::tests::spawn(
            &mut traffic,
            &mut world,
            &content,
            connection,
        )
        .spawned
        .unwrap();
        let mut listener = PlayerListener::new(100);
        listener.players = world;
        listener.traffic_population = Some(traffic);
        listener.rival_population = Some(rivals);
        let ids = [ambient.vehicle, rival.vehicle];
        assert_eq!(listener.movement_objects(), ids.into_iter().collect());
        let delegated = listener.delegated_ai(&listener.players);
        assert_eq!(delegated[&ids[0]], 1);
        assert_eq!(
            delegated[&ids[1]],
            crate::traffic_content::population::RIVAL_KEY_START
        );
        for (step, now) in [(0, 100), (1, 200)] {
            let samples = ids.map(|id| {
                let mut motion = listener
                    .players
                    .ai_vehicle_physics(connection, 100, delegated[&id])
                    .unwrap()
                    .decode([0.; 3])
                    .unwrap();
                motion.identity = (100. + step as f64 * 0.1).to_bits();
                motion.revision = 3000 + step * 3;
                motion.position[0] += 1.;
                motion.velocity = [5., 0., 0.];
                Sample {
                    physics: motion
                        .encode([0.; 3], &chassis::Controls::new(4).unwrap())
                        .unwrap(),
                    flag: false,
                }
            });
            let encoded = samples.each_ref().map(|sample| sample.encode().unwrap());
            assert_eq!(encoded[0].0, encoded[1].0);
            let movement = frame::Movement {
                header: Some(encoded[0].0.span()),
                records: ids
                    .into_iter()
                    .zip(&encoded)
                    .map(|(id, (_, body))| frame::MovementRecord {
                        id,
                        component: 0,
                        data: body.span(),
                    })
                    .collect(),
            };
            let echoes = listener
                .traffic_reports
                .receive_ai(
                    &mut listener.players,
                    connection,
                    100,
                    &movement,
                    &delegated,
                    now,
                )
                .unwrap();
            assert_eq!(
                echoes
                    .iter()
                    .map(|r| r.id)
                    .filter(|id| ids.contains(id))
                    .collect::<BTreeSet<_>>(),
                ids.into_iter().collect()
            );
            // Only each car's first report releases its actor's held control.
            let actors = [ambient.actor, rival.actor];
            let released: BTreeSet<_> = echoes
                .iter()
                .filter(|r| !ids.contains(&r.id))
                .map(|r| r.id)
                .collect();
            assert_eq!(
                released,
                if step == 0 {
                    actors.into_iter().collect()
                } else {
                    BTreeSet::new()
                }
            );
            for (id, sample) in ids.into_iter().zip(samples) {
                assert_eq!(
                    listener
                        .players
                        .ai_vehicle_physics(connection, 100, delegated[&id])
                        .unwrap(),
                    sample.physics
                );
            }
            assert!(
                listener
                    .traffic_reports
                    .receive_ai(
                        &mut listener.players,
                        connection,
                        100,
                        &movement,
                        &delegated,
                        now,
                    )
                    .unwrap()
                    .is_empty()
            );
        }
    }
    #[test]
    fn backpressure_preserves_rivals_and_disable_retires_in_wire_order() {
        let (mut rivals, mut world, mut registrations, messages) =
            crate::rival_content::population::tests::fixture(HOST_SELECTOR as u8);
        crate::rival_content::population::tests::spawn(
            &mut rivals,
            &mut world,
            &mut registrations,
            HOST_SELECTOR as u8,
        );
        let mut population = Population::new(&messages).unwrap();
        let before = population.registrations().clone();
        population
            .update_registrations(&before, registrations)
            .unwrap();
        let mut listener = PlayerListener::new(100);
        listener.players = world;
        listener.rival_population = Some(rivals);
        listener.population = Some(population);
        assert_eq!(listener.movement_objects().len(), 1);
        listener.session_backlog = 2;
        assert!(listener.disable_rivals());
        assert!(!listener.disable_rivals());
        listener.advance_rivals(1).unwrap();
        assert_eq!(listener.rival_population.as_ref().unwrap().count(), 1);
        assert!(listener.pending_entries.is_empty());
        listener.session_backlog = 0;
        listener.advance_rivals(2).unwrap();
        assert!(listener.movement_objects().is_empty());
        assert_eq!(listener.pending_entries.len(), 3);
        let (section, calls) = &listener.pending_entries[0];
        assert!(section.records.is_empty() && section.deleted.is_empty());
        assert!(
            matches!(calls.as_slice(), [nfs_world::participants::HostRpc::Participant(call), nfs_world::participants::HostRpc::WorldActivation(untrack)] if call.call == nfs_world::participants::Call::Leave && untrack.call == nfs_world::world_activation::Call::StreamedParticipantLeave)
        );
        let (detach, calls) = &listener.pending_entries[1];
        assert!(calls.is_empty() && detach.deleted.is_empty());
        assert_eq!(detach.records.len(), 2);
        let (delete, calls) = &listener.pending_entries[2];
        assert!(calls.is_empty() && delete.records.is_empty());
        assert_eq!(delete.deleted.len(), 4);
    }

    #[test]
    fn rival_map_waits_for_queued_rival_entries() {
        let connection = HOST_SELECTOR as u8;
        let (mut rivals, mut world, mut registrations, _) =
            crate::rival_content::population::tests::fixture(connection);
        let ids = crate::rival_content::population::tests::spawn(
            &mut rivals,
            &mut world,
            &mut registrations,
            connection,
        )
        .spawned
        .unwrap();
        let mut listener = PlayerListener::new(100);
        assert_eq!(listener.rival_map(&world), Ok(Some(vec![])));
        listener.rival_population = Some(rivals);
        // A client frame may be handled before the creation entries drain;
        // frame records would then reach the client ahead of the participant.
        listener
            .pending_entries
            .push_back((returning::empty_section(), Vec::new()));
        assert_eq!(listener.rival_map(&world), Ok(None));
        listener.pending_entries.clear();
        let entries = listener.rival_map(&world).unwrap().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, ids.participant);
    }
}
