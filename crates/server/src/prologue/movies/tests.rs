// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    bits::BitWriter,
    replication::{Initial, sublevel},
};
fn fixture() -> (Players, Content, Owner, Owner) {
    let (mut players, authored, a, b) = crate::sequence_content::authored::tests::fixture();
    let scenes = sublevel::Content::new(vec![(
        88,
        sublevel::Profile::new(
            false,
            &[
                sublevel::Kind::Noop,
                sublevel::Kind::Rpc,
                sublevel::Kind::Rpc,
                sublevel::Kind::Rpc,
            ],
        )
        .unwrap(),
    )])
    .unwrap();
    players
        .create_scenes(
            &[100],
            vec![sublevel::passive::creation(100, 88, Serial::new(10).unwrap(), &scenes).unwrap()],
            &scenes,
        )
        .unwrap();
    let sequence = |state| movies::Sequence {
        blueprint: "sequence".into(),
        reference: nfs_content::scoring::events::Identity {
            asset: "a".into(),
            index: 1,
            instances: vec![],
        },
        state: (88, state),
    };
    let data = movies::Movie {
        objective: 99,
        blueprint: "movie".into(),
        video: fmv::Video {
            bundle: 4,
            index: 1,
            count: 3,
        },
        state: (88, 2),
        before: sequence(1),
        after: sequence(3),
    };
    (
        players,
        Content {
            data,
            before: authored.clone(),
            after: authored,
        },
        a,
        b,
    )
}
fn request(scene: u16, participant: u16) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(1, 9).put(3, 32).put(127, 7);
    let mut out = BitWriter::new();
    out.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(scene.into(), 13)
        .put(participant.into(), 13)
        .put(6, 9)
        .put_span(payload.span());
    out
}
#[test]
fn movie_runs_both_owned_sequences_and_waits_for_native_completion() {
    let (mut players, content, a, b) = fixture();
    let scene = players.objects().scene(88).unwrap();
    let (mut movie, out) = Active::start(&content, &mut players, a).unwrap();
    assert_eq!(out.records.len(), 1);
    assert_eq!(out.calls.len(), 1);
    let before = movie.sequence().unwrap();
    let done = Completed {
        sequence: before,
        participant: a.participant,
    };
    let snapshot = players.objects().snapshot();
    assert!(movie.completed(&mut players, b, done).is_err());
    assert_eq!(players.objects().snapshot(), snapshot);
    assert_eq!(movie.phase(), fmv::Phase::Before);
    let out = movie.completed(&mut players, a, done).unwrap();
    assert_eq!(out.deleted, [before]);
    assert_eq!(out.calls.len(), 3);
    assert!(matches!(out.calls[2], HostRpc::Movie(_)));
    assert!(
        movie
            .completed(&mut players, a, done)
            .unwrap()
            .calls
            .is_empty()
    );
    let out = movie
        .receive(
            &content,
            &mut players,
            a,
            request(scene, a.participant).span(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(out.records.len(), 1);
    assert_eq!(out.calls.len(), 2);
    let after = movie.sequence().unwrap();
    assert_ne!(before, after);
    let snapshot = players.objects().snapshot();
    assert!(movie.completed(&mut players, a, done).is_err());
    assert_eq!(players.objects().snapshot(), snapshot);
    assert!(
        movie
            .receive(
                &content,
                &mut players,
                a,
                request(scene, a.participant).span()
            )
            .unwrap()
            .unwrap()
            .records
            .is_empty()
    );
    let out = movie
        .completed(
            &mut players,
            a,
            Completed {
                sequence: after,
                participant: a.participant,
            },
        )
        .unwrap();
    assert!(out.finished);
    assert_eq!(out.deleted, [after]);
    assert_eq!(movie.phase(), fmv::Phase::Finished);
    for call in out.calls {
        call.encode().unwrap();
    }
}
#[test]
fn failed_after_sequence_construction_preserves_playing_state_and_allocations() {
    let (mut players, content, a, _) = fixture();
    let scene = players.objects().scene(88).unwrap();
    let (mut movie, _) = Active::start(&content, &mut players, a).unwrap();
    movie
        .completed(
            &mut players,
            a,
            Completed {
                sequence: movie.sequence().unwrap(),
                participant: a.participant,
            },
        )
        .unwrap();
    let parent=players.objects().iter().find_map(|(id,o)|matches!(&o.initial,Initial::Entity{fields,..} if matches!(fields.first(),Some(nfs_world::replication::entity::Initial::Root{reference,..}) if *reference==a.participant)).then_some(id)).unwrap();
    players.remove_entities(&[parent]).unwrap();
    let snapshot = players.objects().snapshot();
    assert!(
        movie
            .receive_with(
                &content,
                &mut players,
                a,
                request(scene, a.participant).span(),
                |staged| Ok(vec![
                    staged
                        .create_actor(a.connection, a.persona, a.participant)?
                        .unwrap()
                ]),
            )
            .is_err()
    );
    assert_eq!(movie.phase(), fmv::Phase::Playing);
    assert_eq!(players.objects().snapshot(), snapshot);
}
