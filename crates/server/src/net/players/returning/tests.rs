// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    bits::BitWriter,
    participants::{Lifecycle, Stage},
    replication::{
        entity::creation::{Asset, Catalog, Content},
        spline, sublevel,
    },
    sequences::{Component, Definition, Layout, Sequences},
};

fn request(scene: u16, participant: u16, selector: u16, method: u32) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(selector.into(), 9)
        .put(method.into(), 32)
        .align();
    let mut out = BitWriter::new();
    out.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(scene.into(), 13)
        .put(participant.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    out
}

fn transport_with_snapshot(players: &Players) -> nfs_world::session::Host {
    use nfs_world::{
        application::{Fixed, Policy},
        session::{Host, Ids},
        transport::Codec,
    };
    let key = nfs_world::crypto::session_key(
        "11111111-2222-3333-4444-555555555555",
        "66666666-7777-8888-9999-aaaaaaaaaaaa",
    )
    .unwrap();
    let mut host = Host::new(
        Codec::new(key, [5; 64]).unwrap(),
        Ids {
            client: 1,
            host: 2,
            host_index: 0,
        },
        Policy::default(),
        Box::new(Fixed::new([1, 2, 3])),
    )
    .unwrap();
    for section in split_scenes(
        players.objects().snapshot(),
        nfs_world::application::OUTBOUND_FRAME_BITS,
    )
    .unwrap()
    {
        host.queue_replication(section).unwrap();
    }
    host
}

#[test]
fn return_handlers_publish_valid_frames_restore_loading_and_allow_three_exits() {
    let (mut population, mut players, vehicles, layout, items, _) =
        crate::vehicle_content::population::tests::fixture();
    let content = GarageContent {
        vehicles,
        layout: crate::vehicle_content::population::tests::world_layout(&layout),
    };
    let inventory = crate::vehicle_content::population::tests::inventory(items);
    let persona = 101;
    let player = players
        .create(
            HOST_SELECTOR as u8,
            persona,
            Request {
                name: b"RoundTrip".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players
        .join(HOST_SELECTOR as u8, persona, player)
        .unwrap()
        .unwrap()
        .id;
    players
        .create_actor(HOST_SELECTOR as u8, persona, participant)
        .unwrap();
    players.enable_player_manager().unwrap();
    players.sync_player_views().unwrap();
    let owner = Owner {
        connection: HOST_SELECTOR as u8,
        persona,
        participant,
    };
    let car_owner = || crate::vehicle_content::population::Owner {
        connection: owner.connection,
        persona,
        participant,
    };
    let mut startup = vec![sublevel::Kind::RpcReferences; 117];
    startup[0] = sublevel::Kind::Noop;
    let mut garage = vec![sublevel::Kind::Rpc; 28];
    garage[0] = sublevel::Kind::Noop;
    garage[24] = sublevel::Kind::RpcBool;
    let scenes = sublevel::Content::new(vec![
        (5001, sublevel::Profile::new(false, &startup).unwrap()),
        (5002, sublevel::Profile::new(false, &garage).unwrap()),
    ])
    .unwrap();
    let creations = [5001, 5002]
        .into_iter()
        .enumerate()
        .map(|(i, key)| {
            sublevel::ordinary::creation(
                i as u16 + 70,
                key,
                None,
                Serial::new(0).unwrap(),
                &sublevel::ordinary::unpopulated(scenes.profile(key).unwrap()).unwrap(),
                &scenes,
            )
            .unwrap()
        })
        .collect();
    players
        .create_scenes(&[70, 71], creations, &scenes)
        .unwrap();
    let records = players.objects().snapshot();
    let mut lifecycle = Lifecycle::default();
    lifecycle
        .bind(&records, content.layout.item_scene_key, 5001)
        .unwrap();
    lifecycle.bind_exit(&records, 5001, 5002).unwrap();
    lifecycle.bind_return(&records, 5001).unwrap();
    let scene = players.objects().scene(5001).unwrap();
    lifecycle.begin(participant).unwrap();
    lifecycle.set_garage(inventory.slots.values()).unwrap();
    lifecycle
        .receive(request(scene, participant, 10, 3).span(), |p| {
            p == participant
        })
        .unwrap();
    lifecycle.take_garage_bindings();
    lifecycle.finish_persistent(|_| true);
    lifecycle.begin_progression(participant);
    lifecycle.enter_garage_loading(participant);
    let definition = |layout, local_index| Definition {
        presentation: (layout == Layout::GarageEntry).then_some(Component {
            scene_key: 5002,
            index: 13,
        }),
        layout,
        asset: Asset {
            bundle: 30,
            type_id: 42,
            local_index,
        },
        manager: Component {
            scene_key: content.layout.item_scene_key,
            index: 3,
        },
        channel: Component {
            scene_key: 5002,
            index: 12,
        },
        embedded_bus: 2,
    };
    let entity_content = |d: Definition| {
        Content::new(
            vec![Catalog::new(30, &[(42, 3)]).unwrap()],
            vec![(d.asset, d.layout.profile())],
        )
        .unwrap()
    };
    let waiting = definition(Layout::Waiting, 0);
    let mut sequences = Sequences::new(waiting, entity_content(waiting)).unwrap();
    let seq = sequences.start(&mut players, owner).unwrap().unwrap().id;
    sequences
        .poll(nfs_world::sequences::FALLBACK_MS, 1)
        .unwrap();
    sequences
        .complete(
            &players,
            owner,
            Completed {
                sequence: seq,
                participant,
            },
        )
        .unwrap();
    players.remove_entities(&[seq]).unwrap();
    lifecycle.enter_customization(participant);
    let initial = population
        .spawn(
            &mut players,
            car_owner(),
            inventory.clone(),
            &content.vehicles,
            &content.layout,
            [0.; 3],
        )
        .unwrap();
    for b in initial.bindings {
        population
            .acknowledge(&players, car_owner(), b.expected())
            .unwrap();
    }
    let entry = definition(Layout::GarageEntry, 2);
    let mut returns = Arrivals::returning(entry, entity_content(entry), 7500)
        .unwrap()
        .with_spline(spline::Definition {
            asset: Asset {
                bundle: 31,
                type_id: 43,
                local_index: 0,
            },
            catalog: Catalog::new(31, &[(43, 1)]).unwrap(),
            flag: false,
        })
        .unwrap();
    let dummy = Asset {
        bundle: 1,
        type_id: 1,
        local_index: 0,
    };
    let roles = crate::scene_roles::SceneRoles {
        level: b"test".to_vec(),
        gameplay: content.layout.item_scene_key,
        startup: 5001,
        garage: 5002,
        progression: 5003,
        traffic: [0; 10],
        customization_timer: dummy,
        streaming_gate: dummy,
        spawn_points: None,
        free_roam: None,
    };
    let mut ghosts = BTreeMap::new();
    let mut presence = nfs_world::garage::presence::Presence::default();
    let count = players.objects().len();
    for cycle in 0..3 {
        lifecycle
            .receive(request(scene, participant, 96, 0).span(), |_| true)
            .unwrap();
        lifecycle.take_exited();
        let car = population
            .spawn_world(
                &mut players,
                car_owner(),
                &inventory,
                &content.vehicles,
                &content.layout,
            )
            .unwrap()
            .records[0]
            .id;
        players
            .bind_player_vehicle(owner.connection, persona, participant, Some(car))
            .unwrap();
        lifecycle
            .receive(request(scene, participant, 76, 3).span(), |_| true)
            .unwrap();
        lifecycle
            .receive(request(scene, participant, 80, 0).span(), |_| true)
            .unwrap();
        lifecycle.take_arriving();
        lifecycle.arrived(participant);
        // The first request shows the prompt and starts nothing; the repeated
        // request (PageDown) begins the return.
        assert_eq!(
            lifecycle
                .receive(request(scene, participant, 100, 0).span(), |_| true)
                .unwrap(),
            Outcome::Advanced(Vec::new())
        );
        assert_eq!(lifecycle.take_prompted(), [participant]);
        assert!(lifecycle.take_returning().is_empty());
        let Outcome::Advanced(chain) = lifecycle
            .receive(request(scene, participant, 100, 0).span(), |_| true)
            .unwrap()
        else {
            panic!()
        };
        lifecycle.take_returning();
        returns.poll(cycle * 8000).unwrap();
        // Exercise the production queue's reference validation, not only wire
        // encoding: a slot binding must follow its replacement car's creation.
        let mut transport = transport_with_snapshot(&players);
        let entries = PlayerListener::begin_return(
            &mut players,
            &mut returns,
            &mut population,
            &inventory,
            &content,
            None,
            &mut ghosts,
            persona,
            participant,
            chain.into_iter().map(HostRpc::Participant).collect(),
        )
        .unwrap();
        for (section, rpcs) in &entries {
            transport
                .queue_replication_after(section.clone(), rpcs.clone())
                .unwrap();
            assert!(
                nfs_world::session::replication_frame(section, rpcs, 0)
                    .is_ok_and(|wire| wire.len() <= nfs_world::application::OUTBOUND_FRAME_BITS)
            );
            for rpc in rpcs {
                if let HostRpc::Vehicle(b) = rpc {
                    population
                        .acknowledge(&players, car_owner(), b.expected())
                        .unwrap();
                }
            }
        }
        let stops = returns.poll(cycle * 8000 + 7500).unwrap();
        assert_eq!(stops.len(), 1);
        // The timed Stop must pass the same endpoint validation as creation.
        for stop in &stops {
            transport
                .queue_replication_after(empty_section(), vec![HostRpc::SequenceStop(*stop)])
                .unwrap();
        }
        let completion = Completed {
            sequence: returns.ghost(participant).unwrap(),
            participant,
        }
        .encode()
        .unwrap();
        let ((section, rpcs), removed) = PlayerListener::finish_return_call(
            completion.span(),
            &mut returns,
            &mut players,
            &mut lifecycle,
            &mut population,
            &inventory,
            &content,
            &mut sequences,
            None,
            None,
            &mut ghosts,
            &mut presence,
            Some(&roles),
            persona,
        )
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(removed, car);
        transport
            .queue_replication_after(section.clone(), rpcs.clone())
            .unwrap();
        assert!(
            nfs_world::session::replication_frame(&section, &rpcs, 0)
                .is_ok_and(|wire| wire.len() <= nfs_world::application::OUTBOUND_FRAME_BITS)
        );
        assert_eq!(
            transport.queue_rpc(HostRpc::SequenceStop(stops[0])),
            Err(nfs_world::session::Error::Replication(
                replication::Error::UnknownObject
            ))
        );
        assert_eq!(lifecycle.stage(participant), Some(Stage::LoadingGarage));
        assert_eq!(players.objects().len(), count);
        let seq = sequences.start(&mut players, owner).unwrap().unwrap().id;
        sequences
            .poll((cycle + 2) * nfs_world::sequences::FALLBACK_MS, 1)
            .unwrap();
        sequences
            .complete(
                &players,
                owner,
                Completed {
                    sequence: seq,
                    participant,
                },
            )
            .unwrap();
        players.remove_entities(&[seq]).unwrap();
        lifecycle.enter_customization(participant);
    }
}
