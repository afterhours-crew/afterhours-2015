// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::net::items::tests::{deliver_car_reply, enable_cars, item_bindings, transfer};
use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    items::{PurchaseReply, PurchaseRequest, PurchaseSelection, SellReply, SellRequest},
    replication::sublevel,
};

fn setup(listener: &mut PlayerListener, player: u16, car: u16) -> u16 {
    let roles = listener.roles.as_mut().unwrap();
    roles.bank = Some(crate::scene_roles::Bank {
        scene: roles.garage,
        serializer: 1,
    });
    let gameplay = roles.gameplay;
    let startup = roles.startup;
    let profile = sublevel::Profile::new(
        false,
        &std::iter::once(sublevel::Kind::Noop)
            .chain(std::iter::repeat_n(sublevel::Kind::RpcReferences, 114))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let content = sublevel::Content::new(vec![(startup, profile)]).unwrap();
    let creation = sublevel::ordinary::creation(
        88,
        startup,
        None,
        Serial::new(0).unwrap(),
        &sublevel::ordinary::unpopulated(content.profile(startup).unwrap()).unwrap(),
        &content,
    )
    .unwrap();
    listener
        .players
        .create_scenes(&[88], vec![creation], &content)
        .unwrap();
    let records = listener.players.objects().snapshot();
    listener
        .participants
        .bind(&records, gameplay, startup)
        .unwrap();
    let participant = listener
        .players
        .participant_for_player(HOST_SELECTOR as u8, listener.persona, player)
        .unwrap();
    let bindings = nfs_world::participants::Bindings::from_records(&records, gameplay, startup)
        .unwrap()
        .unwrap();
    listener.participants.begin(participant).unwrap();
    listener
        .participants
        .set_garage(listener.inventory.as_ref().unwrap().slots.values())
        .unwrap();
    let mut payload = BitWriter::new();
    payload
        .put(bindings.inventory.selector.into(), 9)
        .put(3, 32)
        .align();
    let mut ack = BitWriter::new();
    ack.put(0, 64)
        .put(2, 8)
        .put(bindings.inventory.scene.into(), 13)
        .put(participant.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    listener
        .participants
        .receive(ack.span(), |p| p == participant)
        .unwrap();
    listener.participants.take_garage_bindings();
    listener
        .participants
        .finish_persistent(|p| p == participant);
    listener.participants.begin_progression(participant);
    listener.participants.enter_garage_loading(participant);
    assert!(matches!(
        listener.participants.enter_customization(participant),
        nfs_world::participants::Outcome::Advanced(_)
    ));
    let scene = listener
        .players
        .objects()
        .scene(listener.vehicles.as_ref().unwrap().layout.scene_key)
        .unwrap();
    let replication::Initial::SubLevel { fields, .. } =
        &listener.players.objects().get(scene).unwrap().initial
    else {
        panic!()
    };
    let rpc = fields[1].rpc().unwrap();
    listener
        .population
        .as_mut()
        .unwrap()
        .acknowledge(
            &listener.players,
            Owner {
                connection: HOST_SELECTOR as u8,
                persona: listener.persona,
                participant,
            },
            nfs_world::garage::vehicle::Loaded {
                scene,
                selector: rpc.selector,
                participant,
                vehicle: car,
            },
        )
        .unwrap();
    participant
}
fn ack_cars(listener: &mut PlayerListener, participant: u16) {
    for rpc in std::mem::take(&mut listener.pending_rpcs) {
        if let nfs_world::participants::HostRpc::Vehicle(binding) = rpc {
            listener
                .population
                .as_mut()
                .unwrap()
                .acknowledge(
                    &listener.players,
                    Owner {
                        connection: HOST_SELECTOR as u8,
                        persona: listener.persona,
                        participant,
                    },
                    binding.expected(),
                )
                .unwrap();
        }
    }
    listener.pending.clear();
    listener.pending_content.clear();
}
fn command(
    listener: &mut PlayerListener,
    scene: u16,
    player: u16,
    method: u32,
    slot: u32,
    item: Option<u64>,
) {
    let mut payload = BitWriter::new();
    payload
        .put(5, 9)
        .put(method.into(), 32)
        .put(slot.into(), 32);
    if let Some(item) = item {
        payload.put(item & 0xffffffff, 32).put(item >> 32, 32);
    }
    payload.align();
    let mut body = BitWriter::new();
    body.put(0, 64)
        .put(2, 8)
        .put(scene.into(), 13)
        .put(player.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    let mut pending = listener.garage_commands.clone();
    assert!(
        listener
            .stage_garage(&listener.players, body.span(), &mut pending)
            .unwrap()
    );
    listener.garage_commands = pending;
}

#[tokio::test]
async fn buy_switch_customize_and_sell_keep_durable_and_visible_cars_in_sync() {
    let (mut listener, mut jobs, repo, account, scene, player, car) = fixture();
    let participant = setup(&mut listener, player, car);
    enable_cars(&mut jobs, &listener.inventory.as_ref().unwrap().items);
    assert!(listener.garage_ready());
    let buy = PurchaseRequest {
        sequence: 2,
        owner: 0,
        selection: PurchaseSelection::Definitions(vec![[1; 16]]),
    }
    .encode()
    .unwrap();
    transfer(&mut listener, 0, b"Items", buy);
    jobs.start(&mut listener, 10, Timestamp(10));
    assert!(jobs.completed(&mut listener).await);
    assert_eq!(listener.inventory.as_ref().unwrap().slots.values()[1], None);
    let bytes = deliver_car_reply(&mut jobs);
    let purchased = PurchaseReply::decode(&bytes, item_bindings(&jobs))
        .unwrap()
        .purchased
        .unwrap();
    let second = purchased
        .items
        .values()
        .find(|i| matches!(i.derived, Derived::RaceVehicle { .. }))
        .unwrap()
        .id;
    jobs.start(&mut listener, 11, Timestamp(11));
    assert_eq!(
        listener.inventory.as_ref().unwrap().slots.values()[1],
        Some(second)
    );
    assert_eq!(
        nfs_services::bank::Balance::from_snapshot(&repo.snapshot(account).unwrap())
            .unwrap()
            .0,
        40
    );
    ack_cars(&mut listener, participant);
    command(&mut listener, scene, player, 3, 0, Some(second));
    jobs.start(&mut listener, 12, Timestamp(12));
    assert!(jobs.completed(&mut listener).await);
    command(&mut listener, scene, player, 2, 0, None);
    jobs.start(&mut listener, 13, Timestamp(13));
    assert!(jobs.completed(&mut listener).await);
    ack_cars(&mut listener, participant);
    assert_eq!(
        listener.inventory.as_ref().unwrap().slots.values()[0],
        Some(second)
    );
    accept(
        &mut listener,
        &appearance::car_finish_wire(scene, player, Some(b"SECOND"), 0.),
    )
    .unwrap();
    let edit = listener.equipment.committed.front().unwrap();
    assert_eq!(edit.car, second);
    assert_ne!(edit.vehicle, car);
    jobs.start(&mut listener, 14, Timestamp(14));
    assert!(jobs.completed(&mut listener).await);
    let _ = crate::net::items::tests::deliver_notification(&mut jobs);
    command(&mut listener, scene, player, 2, 0, None);
    jobs.start(&mut listener, 15, Timestamp(15));
    assert!(jobs.completed(&mut listener).await);
    ack_cars(&mut listener, participant);
    let before = listener.inventory.as_ref().unwrap().items.clone();
    assert_ne!(before.items[&1].derived, before.items[&second].derived);
    transfer(
        &mut listener,
        0,
        b"Items",
        SellRequest {
            sequence: 3,
            items: vec![second],
        }
        .encode()
        .unwrap(),
    );
    jobs.start(&mut listener, 16, Timestamp(16));
    assert!(jobs.completed(&mut listener).await);
    let bytes = deliver_car_reply(&mut jobs);
    assert!(
        SellReply::decode(&bytes, item_bindings(&jobs))
            .unwrap()
            .sold
            .unwrap()
            .items
            .contains_key(&second)
    );
    jobs.start(&mut listener, 17, Timestamp(17));
    assert_eq!(
        listener.inventory.as_ref().unwrap().slots.values(),
        [Some(1), None, None, None, None]
    );
    assert_eq!(
        nfs_services::bank::Balance::from_snapshot(&repo.snapshot(account).unwrap())
            .unwrap()
            .0,
        55
    );
    assert_eq!(
        listener.inventory.as_ref().unwrap().items.items[&1],
        before.items[&1]
    );
    command(&mut listener, scene, player, 4, 0, None);
    jobs.start(&mut listener, 18, Timestamp(18));
    assert!(jobs.completed(&mut listener).await);
}
