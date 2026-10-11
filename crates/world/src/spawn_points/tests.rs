// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    bits::BitWriter,
    replication::{Update, sublevel::Rpc},
};
use nfs_protocol::world::rpc::Serial;

const KEY: u32 = 2_994_378_277;
const SCENE: u16 = 11;
const PLAYER: u16 = 40;

/// Client request or release arguments: f64 clock, u32 spawn id and seven
/// padding bits, which the client does not zero.
fn client_arguments(clock: f64, id: u32, padding: u8) -> String {
    format!("{:064b}{id:032b}{padding:07b}", clock.to_bits())
}
fn with_id(id: u32) -> String {
    client_arguments(4458.5, id, 0b101_1001)
}
fn rpc(selector: u16, serial: u16) -> Rpc {
    Rpc {
        selector,
        serial: Serial::new(serial).unwrap(),
    }
}
fn record(id: u16, key: u32, fields: Vec<sublevel::Initial>) -> Record {
    let kinds = vec![sublevel::Kind::Noop; fields.len()];
    Record {
        id,
        initial: Some(Initial::SubLevel {
            prefix: sublevel::Prefix {
                level_id: 10,
                content_key: key,
                blueprint: None,
                word_a: 0,
                word_b: 0,
                constructor_word: None,
            },
            fields,
        }),
        update: Update::SubLevel {
            profile: sublevel::Profile::new(false, &kinds).unwrap(),
            fields: vec![None; kinds.len()],
        },
    }
}
/// SpawnPoints field layout: noop, rpc, rpc_bool, rpc ×4.
fn spawn_scene(id: u16) -> Record {
    record(
        id,
        KEY,
        vec![
            sublevel::Initial::Empty,
            sublevel::Initial::Rpc(rpc(0, 25)),
            sublevel::Initial::RpcBool {
                rpc: rpc(1, 25),
                value: false,
            },
            sublevel::Initial::Rpc(rpc(2, 25)),
            sublevel::Initial::Rpc(rpc(3, 25)),
            sublevel::Initial::Rpc(rpc(4, 25)),
            sublevel::Initial::Rpc(rpc(5, 25)),
        ],
    )
}
fn bound() -> SpawnPoints {
    let mut model = SpawnPoints::default();
    assert_eq!(model.bind(&[spawn_scene(SCENE)], KEY), Ok(true));
    model
}
fn client(scene: u16, participant: u16, selector: u16, method: u32, arguments: &str) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(selector.into(), 9).put(method.into(), 32);
    for bit in arguments.bytes() {
        payload.put(u64::from(bit == b'1'), 1);
    }
    payload.align();
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(scene.into(), 13)
        .put(participant.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}
fn host_route(wire: &BitWriter, references: usize, f: impl FnOnce(&[u16], u16, u32, String)) {
    let envelope = Envelope::decode(
        wire.span(),
        Limits {
            max_input_bits: 4096,
            max_references: references,
            max_payload_bytes: 16,
        },
    )
    .unwrap();
    let route = envelope.route(RouteProfile::ClientReceive).unwrap();
    assert_eq!(route.serial(), Serial::new(25));
    let span = route.arguments();
    let bits = (0..span.len())
        .map(|i| {
            if span.read_u32(i, 1).unwrap() == 1 {
                '1'
            } else {
                '0'
            }
        })
        .collect();
    f(
        envelope.references(),
        route.selector(),
        route.method_index(),
        bits,
    );
}

#[test]
fn binds_the_scene_layout_and_rejects_absent_duplicate_or_other_shapes() {
    let model = bound();
    let b = model.bindings().unwrap();
    assert_eq!((b.assign.scene, b.assign.selector), (SCENE, 0));
    assert_eq!((b.occupied.scene, b.occupied.selector), (SCENE, 1));
    assert_eq!(SpawnPoints::default().bind(&[], KEY), Ok(false));
    assert_eq!(
        SpawnPoints::default().bind(&[spawn_scene(1), spawn_scene(2)], KEY),
        Err(Error::DuplicateObject)
    );
    let mut twice = bound();
    assert_eq!(
        twice.bind(&[spawn_scene(SCENE)], KEY),
        Err(Error::DuplicateObject)
    );
    let flat = record(SCENE, KEY, vec![sublevel::Initial::Empty; 3]);
    assert_eq!(
        SpawnPoints::default().bind(&[flat], KEY),
        Err(Error::TypeMismatch)
    );
}

#[test]
fn assignments_carry_the_value_then_padding() {
    let mut model = bound();
    for (participant, value) in [(PLAYER, 1), (PLAYER, 2), (PLAYER, 3)] {
        let n = model.assign(participant, value).unwrap();
        assert_eq!(n.call, Call::Assign(value));
        assert_eq!(model.assigned(participant), Some(value));
    }
    // Selector, serial, method 0, then the u32 id and five padding bits that
    // complete the byte (11-byte payload).
    let wire = model.assign(41, 4).unwrap().encode().unwrap();
    host_route(&wire, 2, |references, selector, method, bits| {
        assert_eq!(references, [SCENE, 41]);
        assert_eq!((selector, method), (0, 0));
        assert_eq!(bits, format!("{:032b}00000", 4));
    });
    assert_eq!(
        SpawnPoints::default().assign(PLAYER, 1),
        Err(Error::Unsupported)
    );
}

#[test]
fn request_and_release_toggle_the_occupied_flag() {
    let mut model = bound();
    model.assign(PLAYER, 1).unwrap();
    let owns = |id| id == PLAYER;
    let request = client(SCENE, PLAYER, 0, REQUEST, &with_id(1));
    let release = client(
        SCENE,
        PLAYER,
        0,
        RELEASE,
        &client_arguments(4466.75, 1, 0b111_0000),
    );
    let on = model.receive(request.span(), owns).unwrap();
    let occupied = model.bindings().unwrap().occupied;
    assert_eq!(
        on,
        Some(Some(Flag {
            endpoint: occupied,
            enabled: true
        }))
    );
    // Scene-scoped flag: selector 1, method 0 (on), five padding bits.
    let wire = on.unwrap().unwrap().encode().unwrap();
    host_route(&wire, 1, |references, selector, method, bits| {
        assert_eq!(references, [SCENE]);
        assert_eq!((selector, method), (1, 0));
        assert_eq!(bits, "00000");
    });
    // A repeated request keeps the flag on without another notification.
    assert_eq!(model.receive(request.span(), owns), Ok(Some(None)));
    assert_eq!(
        model.receive(release.span(), owns),
        Ok(Some(Some(Flag {
            endpoint: occupied,
            enabled: false
        })))
    );
    assert_eq!(model.receive(release.span(), owns), Ok(Some(None)));
}

#[test]
fn other_endpoints_stale_ids_foreign_participants_and_bad_shapes_do_not_mutate() {
    let mut model = bound();
    model.assign(PLAYER, 1).unwrap();
    let owns = |id| id == PLAYER || id == 41;
    let before = model.clone();
    for other in [
        client(SCENE + 1, PLAYER, 0, REQUEST, &with_id(1)),
        client(SCENE, PLAYER, 2, REQUEST, &with_id(1)),
        client(SCENE, PLAYER, 0, 0, &with_id(1)),
    ] {
        assert_eq!(model.receive(other.span(), owns), Ok(None));
    }
    for (body, error) in [
        (client(SCENE, PLAYER, 0, REQUEST, &with_id(2)), Error::Shape),
        (client(SCENE, 41, 0, REQUEST, &with_id(1)), Error::Shape),
        (
            client(SCENE, 42, 0, REQUEST, &with_id(1)),
            Error::UnknownObject,
        ),
        (
            client(SCENE, PLAYER, 0, REQUEST, &with_id(1)[..88]),
            Error::Shape,
        ),
        (
            client(SCENE, PLAYER, 0, REQUEST, &format!("{}0", with_id(1))),
            Error::Shape,
        ),
    ] {
        assert_eq!(model.receive(body.span(), owns), Err(error));
    }
    assert_eq!(model, before);
    assert_eq!(
        SpawnPoints::default().receive(client(SCENE, PLAYER, 0, REQUEST, &with_id(1)).span(), owns),
        Ok(None)
    );
    assert!(matches!(
        model.receive(client(SCENE, PLAYER, 0, REQUEST, &with_id(1)).span(), owns),
        Ok(Some(Some(_)))
    ));
}

#[test]
fn participants_are_isolated_and_assignments_are_bounded() {
    let mut model = bound();
    model.assign(PLAYER, 1).unwrap();
    model.assign(41, 2).unwrap();
    let owns = |_| true;
    assert!(matches!(
        model.receive(client(SCENE, PLAYER, 0, REQUEST, &with_id(1)).span(), owns),
        Ok(Some(Some(_)))
    ));
    assert_eq!(
        model.receive(client(SCENE, 41, 0, REQUEST, &with_id(2)).span(), owns),
        Ok(Some(None))
    );
    assert_eq!(
        model.receive(client(SCENE, PLAYER, 0, RELEASE, &with_id(1)).span(), owns),
        Ok(Some(None))
    );
    assert!(matches!(
        model.receive(client(SCENE, 41, 0, RELEASE, &with_id(2)).span(), owns),
        Ok(Some(Some(Flag { enabled: false, .. })))
    ));
    let mut full = bound();
    for participant in 1..=128 {
        full.assign(participant, 3).unwrap();
    }
    let before = full.clone();
    assert_eq!(full.assign(129, 3), Err(Error::Bound));
    assert_eq!(full, before);
    // An assigned participant may be set again.
    assert_eq!(full.assign(1, 129).unwrap().call, Call::Assign(129));
    assert_eq!(full.assigned(1), Some(129));
}
