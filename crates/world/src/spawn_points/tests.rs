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
fn subscriptions_carry_the_token_false_membership_and_padding() {
    let mut model = bound();
    for (participant, value) in [(PLAYER, 1), (PLAYER, 2), (PLAYER, 3)] {
        let n = model.assign(participant, value).unwrap();
        assert_eq!(n.call, Call::Assign(value));
        assert_eq!(model.assigned(participant), Some(value));
    }
    // Selector, serial, method 0, then the u32 token, false membership and
    // four padding bits that complete the byte (11-byte payload).
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
fn arrival_gates_the_prompt_but_preserves_crossings_and_three_lifetimes() {
    let mut model = bound();
    let before = model.clone();
    assert_eq!(
        model.set_presentation_ready(PLAYER, false),
        Err(Error::UnknownObject)
    );
    assert_eq!(model, before);
    for token in 1..=3 {
        model.assign(PLAYER, token).unwrap();
        assert_eq!(model.set_presentation_ready(PLAYER, false), Ok(None));
        let enter = client(SCENE, PLAYER, 0, REQUEST, &with_id(token));
        let leave = client(SCENE, PLAYER, 0, RELEASE, &with_id(token));
        // Loading, leaving the box during rollout and entering it again do
        // not publish a prompt before the driving context is ready.
        for report in [&enter, &leave, &enter] {
            assert_eq!(model.receive(report.span(), |_| true), Ok(Some(None)));
        }
        assert!(matches!(
            model.set_presentation_ready(PLAYER, true),
            Ok(Some(Flag { enabled: true, .. }))
        ));
        assert_eq!(model.set_presentation_ready(PLAYER, true), Ok(None));
        assert!(matches!(
            model.receive(leave.span(), |_| true),
            Ok(Some(Some(Flag { enabled: false, .. })))
        ));
        assert!(matches!(
            model.receive(enter.span(), |_| true),
            Ok(Some(Some(Flag { enabled: true, .. })))
        ));
        assert!(matches!(
            model.clear(PLAYER).unwrap().unwrap().1,
            Some(Flag { enabled: false, .. })
        ));
    }
}

#[test]
fn readiness_uses_current_occupancy_and_does_not_hide_another_ready_player() {
    let mut model = bound();
    for participant in [PLAYER, 41] {
        model.assign(participant, 1).unwrap();
        model.set_presentation_ready(participant, false).unwrap();
    }
    let enter = |p| client(SCENE, p, 0, REQUEST, &with_id(1));
    let leave = |p| client(SCENE, p, 0, RELEASE, &with_id(1));
    model.receive(enter(PLAYER).span(), |_| true).unwrap();
    model.receive(leave(PLAYER).span(), |_| true).unwrap();
    // A car that finished its animation outside must not show a prompt.
    assert_eq!(model.set_presentation_ready(PLAYER, true), Ok(None));
    assert!(matches!(
        model.receive(enter(PLAYER).span(), |_| true),
        Ok(Some(Some(Flag { enabled: true, .. })))
    ));
    assert_eq!(model.receive(enter(41).span(), |_| true), Ok(Some(None)));
    assert_eq!(model.set_presentation_ready(41, true), Ok(None));
    assert_eq!(model.set_presentation_ready(PLAYER, false), Ok(None));
    assert_eq!(model.clear(PLAYER).unwrap().unwrap().1, None);
    assert!(matches!(
        model.receive(leave(41).span(), |_| true),
        Ok(Some(Some(Flag { enabled: false, .. })))
    ));
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
    // The report form names the participant and kind; a repeated release
    // is current but changes no flag, a stale id is not current.
    assert_eq!(
        model.report(release.span(), owns),
        Ok(Some(Report {
            participant: PLAYER,
            release: true,
            current: true,
            flag: None
        }))
    );
    let on = model.report(request.span(), owns).unwrap().unwrap();
    assert_eq!(
        (on.participant, on.release, on.current),
        (PLAYER, false, true)
    );
    assert_eq!(on.flag.map(|f| f.enabled), Some(true));
    model.assign(PLAYER, 2).unwrap();
    assert_eq!(
        model.report(release.span(), owns),
        Ok(Some(Report {
            participant: PLAYER,
            release: true,
            current: false,
            flag: None
        }))
    );
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
fn delayed_previous_spawn_notifications_do_not_block_the_next_assignment() {
    let mut model = bound();
    let owns = |id| id == PLAYER || id == 41;
    model.assign(41, 7).unwrap();
    for value in 1..=3 {
        model.assign(PLAYER, value).unwrap();
        if value > 1 {
            let before = model.clone();
            for method in [REQUEST, RELEASE] {
                let old = client(SCENE, PLAYER, 0, method, &with_id(value - 1));
                assert_eq!(model.receive(old.span(), owns), Ok(Some(None)));
            }
            assert_eq!(model, before);
        }
        let current = client(SCENE, PLAYER, 0, REQUEST, &with_id(value));
        assert!(model.receive(current.span(), owns).is_ok());
        assert!(model.occupants.contains(&PLAYER));
        assert!(!model.occupants.contains(&41));
        let future = client(SCENE, PLAYER, 0, REQUEST, &with_id(value + 1));
        let zero = client(SCENE, PLAYER, 0, RELEASE, &with_id(0));
        let before = model.clone();
        assert_eq!(model.receive(future.span(), owns), Err(Error::Shape));
        assert_eq!(model.receive(zero.span(), owns), Err(Error::Shape));
        assert_eq!(model, before);
        let release = client(SCENE, PLAYER, 0, RELEASE, &with_id(value));
        assert!(model.receive(release.span(), owns).is_ok());
    }
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

#[test]
fn the_garage_clear_calls_method_one_and_drops_the_occupant() {
    let mut model = bound();
    let owns = |_| true;
    assert_eq!(
        SpawnPoints::default().clear(PLAYER),
        Err(Error::Unsupported)
    );
    assert_eq!(model.clear(PLAYER), Ok(None));
    model.assign(PLAYER, 1).unwrap();
    // Released before the clear (official order): no flag change.
    let (n, flag) = model.clear(PLAYER).unwrap().unwrap();
    assert_eq!((n.participant, n.call, flag), (PLAYER, Call::Leave, None));
    assert_eq!(n.endpoint, model.bindings().unwrap().assign);
    host_route(
        &n.encode().unwrap(),
        2,
        |references, selector, method, bits| {
            assert_eq!(references, [SCENE, PLAYER]);
            assert_eq!((selector, method), (0, 1));
            assert_eq!(bits, "00000");
        },
    );
    // The counter and currency continue from the cleared value.
    assert_eq!(model.assigned(PLAYER), Some(1));
    assert_eq!(model.next_value(PLAYER), 2);
    // Still occupying at the clear (no release reported): the flag drops.
    model.assign(PLAYER, 2).unwrap();
    assert!(matches!(
        model.receive(client(SCENE, PLAYER, 0, REQUEST, &with_id(2)).span(), owns),
        Ok(Some(Some(Flag { enabled: true, .. })))
    ));
    let (n, flag) = model.clear(PLAYER).unwrap().unwrap();
    assert_eq!(n.call, Call::Leave);
    assert!(matches!(flag, Some(Flag { enabled: false, .. })));
    assert_eq!(flag.unwrap().endpoint, model.bindings().unwrap().occupied);
    // The next visit's enter raises the flag again.
    model.assign(PLAYER, 3).unwrap();
    assert!(matches!(
        model.receive(client(SCENE, PLAYER, 0, REQUEST, &with_id(3)).span(), owns),
        Ok(Some(Some(Flag { enabled: true, .. })))
    ));
}

#[test]
fn three_lifetimes_clear_and_reassign_with_delayed_callbacks_and_isolation() {
    let mut model = bound();
    let owns = |id| id == PLAYER || id == 41;
    let occupied = model.bindings().unwrap().occupied;
    // Another participant's assignment is untouched by the player's clears.
    model.assign(41, 5).unwrap();
    for value in 1..=3u32 {
        assert_eq!(model.next_value(PLAYER), value);
        let assign = model.assign(PLAYER, value).unwrap();
        assert_eq!(assign.call, Call::Assign(value));
        if value > 1 {
            // A delayed callback with the previous lifetime's id is
            // acknowledged without touching the occupancy.
            let late = client(SCENE, PLAYER, 0, RELEASE, &with_id(value - 1));
            assert_eq!(
                model.report(late.span(), owns),
                Ok(Some(Report {
                    participant: PLAYER,
                    release: true,
                    current: false,
                    flag: None
                }))
            );
        }
        let request = client(SCENE, PLAYER, 0, REQUEST, &with_id(value));
        let release = client(SCENE, PLAYER, 0, RELEASE, &with_id(value));
        let on = model.report(request.span(), owns).unwrap().unwrap();
        assert!(on.current && !on.release);
        assert!(matches!(on.flag, Some(Flag { enabled: true, .. })));
        // Drive-away and re-entry toggle the flag within the lifetime.
        let off = model.report(release.span(), owns).unwrap().unwrap();
        assert!(matches!(off.flag, Some(Flag { enabled: false, .. })));
        let on = model.report(request.span(), owns).unwrap().unwrap();
        assert!(matches!(on.flag, Some(Flag { enabled: true, .. })));
        // Entering the garage clears the assignment while the car still
        // occupies the zone: method 1 and the flag drop.
        let (clear, flag) = model.clear(PLAYER).unwrap().unwrap();
        assert_eq!((clear.participant, clear.call), (PLAYER, Call::Leave));
        assert_eq!(
            flag,
            Some(Flag {
                endpoint: occupied,
                enabled: false
            })
        );
        // A repeated clear changes no occupancy; the counter continues.
        assert_eq!(model.clear(PLAYER), Ok(Some((clear, None))));
        assert_eq!(model.assigned(PLAYER), Some(value));
        assert_eq!(model.next_value(PLAYER), value + 1);
        // The cleared id stays current for late callbacks of this lifetime
        // until the next assignment.
        assert_eq!(
            model.report(release.span(), owns).unwrap().unwrap().flag,
            None
        );
        assert_eq!(model.assigned(41), Some(5));
    }
    // The other participant's zone visit raises the flag on its own id.
    let other = client(SCENE, 41, 0, REQUEST, &with_id(5));
    let on = model.report(other.span(), owns).unwrap().unwrap();
    assert_eq!((on.participant, on.current), (41, true));
    assert!(matches!(on.flag, Some(Flag { enabled: true, .. })));
    // The player's clear does not drop a flag the other participant holds.
    model.assign(PLAYER, 4).unwrap();
    let request = client(SCENE, PLAYER, 0, REQUEST, &with_id(4));
    assert_eq!(
        model.report(request.span(), owns).unwrap().unwrap().flag,
        None
    );
    assert_eq!(model.clear(PLAYER).unwrap().unwrap().1, None);
    assert_eq!(model.assigned(41), Some(5));
}
