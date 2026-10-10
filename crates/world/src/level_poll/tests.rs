// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    bits::BitWriter,
    replication::{Update, sublevel::Rpc},
};
use nfs_protocol::world::rpc::Serial;

const KEY: u32 = 1_286_940_107;
const SCENE: u16 = 2;
const PLAYER: u16 = 40;

fn level_root(id: u16, field: sublevel::Initial) -> Record {
    let mut fields = vec![sublevel::Initial::Empty; 60];
    fields[FIELD] = field;
    let kinds = vec![sublevel::Kind::Noop; fields.len()];
    Record {
        id,
        initial: Some(Initial::SubLevel {
            prefix: sublevel::Prefix {
                level_id: 1,
                content_key: KEY,
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
fn poll_field(selector: u16) -> sublevel::Initial {
    sublevel::Initial::Rpc(Rpc {
        selector,
        serial: Serial::new(28).unwrap(),
    })
}
fn bound() -> LevelPoll {
    let mut model = LevelPoll::default();
    assert_eq!(
        model.bind(&[level_root(SCENE, poll_field(49))], KEY),
        Ok(true)
    );
    model
}
fn answer(scene: u16, participant: u16, selector: u16, method: u32, bits: &str) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(selector.into(), 9).put(method.into(), 32);
    for bit in bits.bytes() {
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

#[test]
fn binds_the_level_root_rpc_field_only() {
    let model = bound();
    let e = model.endpoint().unwrap();
    assert_eq!((e.scene, e.selector, e.serial.value()), (SCENE, 49, 28));
    assert_eq!(LevelPoll::default().bind(&[], KEY), Ok(false));
    let mut twice = bound();
    assert_eq!(
        twice.bind(&[level_root(SCENE, poll_field(49))], KEY),
        Err(Error::DuplicateObject)
    );
    assert_eq!(
        LevelPoll::default().bind(
            &[level_root(1, poll_field(49)), level_root(3, poll_field(49))],
            KEY
        ),
        Err(Error::DuplicateObject)
    );
    let plain = level_root(SCENE, sublevel::Initial::Empty);
    assert_eq!(
        LevelPoll::default().bind(&[plain], KEY),
        Err(Error::TypeMismatch)
    );
}

#[test]
fn polls_are_scene_scoped_method_zero_at_the_interval_without_bursts() {
    let mut model = bound();
    let poll = model.due(5_000).unwrap().unwrap();
    assert!(poll.enabled);
    // Scene-scoped, selector 49, method 0, five padding bits.
    let wire = poll.encode().unwrap();
    let envelope = Envelope::decode(
        wire.span(),
        Limits {
            max_input_bits: 4096,
            max_references: 1,
            max_payload_bytes: 7,
        },
    )
    .unwrap();
    let route = envelope.route(RouteProfile::ClientReceive).unwrap();
    assert_eq!(envelope.references(), [SCENE]);
    assert_eq!((route.selector(), route.method_index()), (49, 0));
    assert_eq!(route.arguments().len(), 5);
    assert_eq!(route.arguments().read_u32(0, 5), Ok(0));
    assert_eq!(model.due(5_000), Ok(None));
    assert_eq!(model.due(6_099), Ok(None));
    assert!(model.due(6_100).unwrap().is_some());
    // After a stall, one poll, then the interval restarts from now.
    assert!(model.due(20_000).unwrap().is_some());
    assert_eq!(model.due(21_099), Ok(None));
    assert!(model.due(21_100).unwrap().is_some());
    assert_eq!(LevelPoll::default().due(0), Ok(None));
    let mut end = bound();
    assert_eq!(end.due(u64::MAX), Err(Error::Bound));
    assert_eq!(end, bound());
}

#[test]
fn answers_are_recorded_per_participant_and_padding_is_ignored() {
    let mut model = bound();
    let owns = |id| id == PLAYER || id == 41;
    assert_eq!(
        model.receive(answer(SCENE, PLAYER, 49, ANSWER, "1000000").span(), owns),
        Ok(Some(true))
    );
    assert_eq!(
        model.receive(answer(SCENE, 41, 49, ANSWER, "0101101").span(), owns),
        Ok(Some(false))
    );
    assert_eq!(model.answer(PLAYER), Some(true));
    assert_eq!(model.answer(41), Some(false));
    assert_eq!(
        model.receive(answer(SCENE, PLAYER, 49, ANSWER, "1000101").span(), owns),
        Ok(Some(true))
    );
}

#[test]
fn other_calls_foreign_participants_and_bad_shapes_do_not_mutate() {
    let mut model = bound();
    let owns = |id| id == PLAYER;
    let before = model.clone();
    for other in [
        answer(SCENE + 1, PLAYER, 49, ANSWER, "1000000"),
        answer(SCENE, PLAYER, 48, ANSWER, "1000000"),
        answer(SCENE, PLAYER, 49, 0, "1000000"),
    ] {
        assert_eq!(model.receive(other.span(), owns), Ok(None));
    }
    for (body, error) in [
        (
            answer(SCENE, 42, 49, ANSWER, "1000000"),
            Error::UnknownObject,
        ),
        (answer(SCENE, PLAYER, 49, ANSWER, "10000001"), Error::Shape),
    ] {
        assert_eq!(model.receive(body.span(), owns), Err(error));
    }
    assert_eq!(model, before);
    assert_eq!(
        LevelPoll::default().receive(answer(SCENE, PLAYER, 49, ANSWER, "1000000").span(), owns),
        Ok(None)
    );
    let mut full = bound();
    for participant in 1..=128 {
        full.receive(
            answer(SCENE, participant, 49, ANSWER, "1000000").span(),
            |_| true,
        )
        .unwrap();
    }
    let before = full.clone();
    assert_eq!(
        full.receive(answer(SCENE, 129, 49, ANSWER, "1000000").span(), |_| true),
        Err(Error::Bound)
    );
    assert_eq!(full, before);
}
