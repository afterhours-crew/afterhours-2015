// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::super::player_view;
use super::*;
impl Players {
    /// Allocate one view per current player. The serial is owned by this world;
    /// repeated synchronization retains the same view and never resets its seat.
    pub fn sync_player_views(&mut self) -> Result<Vec<Record>, Error> {
        let missing = self
            .objects
            .iter()
            .filter_map(|(id, o)| {
                (matches!(o.initial, Initial::Player(_)) && !self.views.contains_key(&id))
                    .then_some(id)
            })
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(Vec::new());
        }
        let mut next = self.clone();
        let mut records = Vec::with_capacity(missing.len());
        for player in missing {
            let serial = next.view_serial;
            let record = next.objects.spawn(
                Initial::PlayerView(player_view::Initial { player, serial }),
                Update::PlayerView(player_view::Update {
                    base: Some(player_view::Base {
                        active: true,
                        players: Some(player_view::Players {
                            serial,
                            players: vec![],
                        }),
                        target: Some(None),
                        value8: Some(0),
                        seat: Some(None),
                        flag_b: Some(true),
                        value16: Some(0),
                        value6: Some(0),
                        ..Default::default()
                    }),
                    view: Some(player_view::View {
                        transform: Some(player_view::Transform {
                            rotation: vehicle::Quaternion25::from_quaternion([0., 0., 0., 1.])?,
                            position: vehicle::Vector::from_position([0., -1., 0.], [0.; 3], 5)?,
                            relative: None,
                        }),
                        mode: Some(0),
                        value3: Some(0),
                        vehicle_a: Some(0),
                        vehicle_b: Some((0, None)),
                    }),
                }),
            )?;
            next.views.insert(player, record.id);
            next.view_serial = serial.wrapping_add(1);
            records.push(record);
        }
        *self = next;
        Ok(records)
    }
    pub fn owned_player_view(&self, connection: u8, persona: u64, player: u16) -> Option<u16> {
        self.owns(connection, persona, player)
            .then(|| self.views.get(&player).copied())
            .flatten()
    }
}
pub(super) fn seat_update(vehicle: Option<u16>) -> Update {
    Update::PlayerView(player_view::Update {
        base: Some(player_view::Base {
            seat: Some(vehicle.map(|vehicle| player_view::Seat { vehicle, index: 0 })),
            value8: vehicle.is_none().then_some(0),
            ..Default::default()
        }),
        view: vehicle.is_none().then(|| player_view::View {
            mode: Some(0),
            ..Default::default()
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn player(world: &mut Players, c: u8, p: u64) -> (u16, u16) {
        let player = world
            .create(
                c,
                p,
                Request {
                    name: format!("Driver{c}").into_bytes(),
                    flag: false,
                    slot: 0,
                },
            )
            .unwrap()
            .unwrap()
            .id;
        let participant = world.join(c, p, player).unwrap().unwrap().id;
        world.create_actor(c, p, participant).unwrap();
        (player, participant)
    }
    fn car(world: &mut Players, c: u8, p: u64, participant: u16, item: u64) -> u16 {
        use entity::creation::{Asset, Catalog, Content, Prefix};
        let asset = Asset {
            bundle: 2,
            type_id: 30,
            local_index: 0,
        };
        let content = Content::new(vec![Catalog::new(2, &[(30, 1)]).unwrap()], vec![])
            .unwrap()
            .with_vehicles(vec![(
                asset,
                vehicle::Profile::new(vec![vehicle::Kind::Root {
                    property_owner: false,
                }])
                .unwrap(),
            )])
            .unwrap();
        let root = vehicle::root::State::new(None)
            .unwrap()
            .initial(
                [0.; 3],
                [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                [0.; 3],
                entity::Rpc {
                    selector: 0,
                    serial: nfs_protocol::world::rpc::Serial::new(1).unwrap(),
                },
            )
            .unwrap();
        let creation = vehicle::EntityCreation {
            prefix: Prefix {
                parent: None,
                blueprint: 0,
                sub_id: 1,
                owner: None,
                asset,
            },
            body: vehicle::Body {
                creation: Some(vehicle::Creation {
                    setup: None,
                    connection_id: u16::from(c),
                    fields: vec![vehicle::Initial::Root(Box::new(root))],
                }),
                updates: vec![Some(vehicle::Update::Root(None))],
            },
        };
        world
            .create_vehicle(c, p, participant, item, creation, &content)
            .unwrap()
            .id
    }
    fn current(world: &Players, id: u16) -> player_view::Update {
        let Update::PlayerView(v) = &world.objects.get(id).unwrap().current else {
            panic!()
        };
        v.clone()
    }
    #[test]
    fn views_are_owned_stable_and_do_not_leak_into_another_session() {
        let mut world = Players::default();
        let (a, _) = player(&mut world, 1, 101);
        let (b, _) = player(&mut world, 2, 202);
        let records = world.sync_player_views().unwrap();
        assert_eq!(records.len(), 2);
        let av = world.owned_player_view(1, 101, a).unwrap();
        let bv = world.owned_player_view(2, 202, b).unwrap();
        assert_ne!(av, bv);
        assert_eq!(world.owned_player_view(2, 202, a), None);
        assert_eq!(world.owned_player_view(1, 202, a), None);
        assert!(world.sync_player_views().unwrap().is_empty());
        let init = |r: &Record| match &r.initial {
            Some(Initial::PlayerView(v)) => v.clone(),
            _ => panic!(),
        };
        assert_eq!(init(&records[0]).serial, 0);
        assert_eq!(init(&records[1]).serial, 1);
        for r in records {
            let wire = r.encode().unwrap();
            let d = Record::decode(wire.span(), None).unwrap();
            assert_eq!(d.record, r);
            assert_eq!(d.bits, wire.len());
        }
        let mut fresh = Players::default();
        let (new, _) = player(&mut fresh, 1, 303);
        let fresh_views = fresh.sync_player_views().unwrap();
        assert_eq!(
            init(&fresh_views[0]),
            player_view::Initial {
                player: new,
                serial: 0
            }
        );
        assert!(fresh.owned_player_view(1, 101, new).is_none());
        assert_eq!(world.owned_player_view(1, 101, a), Some(av));
    }
    #[test]
    fn seat_lifecycle_is_atomic_owned_repeat_safe_and_detached_before_delete() {
        let mut world = Players::default();
        let (a, ap) = player(&mut world, 1, 101);
        let (b, bp) = player(&mut world, 2, 202);
        world.enable_player_manager().unwrap();
        world.sync_player_views().unwrap();
        let av = world.owned_player_view(1, 101, a).unwrap();
        let bv = world.owned_player_view(2, 202, b).unwrap();
        let garage = car(&mut world, 1, 101, ap, 10);
        let other = car(&mut world, 2, 202, bp, 20);
        let before = world.objects.snapshot();
        assert_eq!(
            world.bind_player_vehicle(1, 101, ap, Some(other)),
            Err(Error::UnknownObject)
        );
        assert_eq!(
            world.bind_player_vehicle(2, 202, ap, Some(garage)),
            Err(Error::UnknownObject)
        );
        assert_eq!(world.objects.snapshot(), before);
        let records = world.bind_player_vehicle(1, 101, ap, Some(garage)).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records.last().unwrap().id, av);
        assert!(
            world
                .bind_player_vehicle(1, 101, ap, Some(garage))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            current(&world, av).base.unwrap().seat,
            Some(Some(player_view::Seat {
                vehicle: garage,
                index: 0
            }))
        );
        assert_eq!(current(&world, bv).base.unwrap().seat, Some(None));
        assert!(world.remove_vehicle(1, 101, ap, 10).is_err());
        let before = world.objects.snapshot();
        assert!(world.objects.remove(garage).is_err());
        assert_eq!(world.objects.snapshot(), before);
        let records = world.bind_player_vehicle(1, 101, ap, None).unwrap();
        assert_eq!(records.len(), 3);
        let view = current(&world, av);
        assert_eq!(view.base.unwrap().seat, Some(None));
        assert_eq!(view.view.unwrap().mode, Some(0));
        assert_eq!(world.remove_vehicle(1, 101, ap, 10), Ok(garage));
        let world_car = car(&mut world, 1, 101, ap, 10);
        assert_ne!(world_car, garage);
        world
            .bind_player_vehicle(1, 101, ap, Some(world_car))
            .unwrap();
        assert_eq!(
            current(&world, av).base.unwrap().seat,
            Some(Some(player_view::Seat {
                vehicle: world_car,
                index: 0
            }))
        );
        assert!(world.sync_player_views().unwrap().is_empty());
        assert_eq!(world.owned_player_view(1, 101, a), Some(av));
        // A rejected view mutation rolls back the related manager/participant updates.
        world.views.insert(a, bp);
        let before = world.objects.snapshot();
        assert!(world.bind_player_vehicle(1, 101, ap, None).is_err());
        assert_eq!(world.objects.snapshot(), before);
    }
    #[test]
    fn invalid_owner_references_and_allocation_exhaustion_leave_state_unchanged() {
        let mut world = Players::default();
        let (a, _) = player(&mut world, 1, 101);
        world.sync_player_views().unwrap();
        let av = world.owned_player_view(1, 101, a).unwrap();
        let before = world.objects.snapshot();
        assert!(
            world
                .objects
                .spawn(
                    Initial::PlayerView(player_view::Initial {
                        player: 7000,
                        serial: 0
                    }),
                    Update::PlayerView(Default::default())
                )
                .is_err()
        );
        assert!(
            world
                .objects
                .change(
                    av,
                    Update::PlayerView(player_view::Update {
                        base: Some(player_view::Base {
                            players: Some(player_view::Players {
                                serial: 1,
                                players: vec![av]
                            }),
                            ..Default::default()
                        }),
                        view: None
                    })
                )
                .is_err()
        );
        assert!(world.objects.remove(a).is_err());
        assert_eq!(world.objects.snapshot(), before);
        let mut full = Players::default();
        player(&mut full, 1, 101);
        player(&mut full, 2, 202);
        while full.objects.len() < super::super::MAX_OBJECTS - 1 {
            full.objects
                .spawn(
                    Initial::PlayerManager,
                    Update::PlayerManager(Default::default()),
                )
                .unwrap();
        }
        let before = full.objects.snapshot();
        assert!(full.sync_player_views().is_err());
        assert_eq!(full.objects.snapshot(), before);
        assert_eq!(full.view_serial, 0);
        assert!(full.views.is_empty());
    }
}
