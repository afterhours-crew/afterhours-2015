// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_protocol::world::rpc::Serial;
fn endpoint(selector: u16) -> Endpoint {
    Endpoint {
        scene: 6,
        selector,
        serial: Serial::new(12).unwrap(),
    }
}
fn fixture() -> (Owner, Bindings) {
    (
        Owner {
            connection: 4,
            persona: 17,
            participant: 9,
        },
        Bindings {
            before: endpoint(3),
            playing: endpoint(4),
            after: endpoint(5),
            video: Video {
                bundle: 7,
                index: 4,
                count: 8,
            },
        },
    )
}
fn completed(participant: u16, padding: u8) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(4, 9).put(3, 32).put(padding.into(), 7);
    let mut out = BitWriter::new();
    out.put(0, 32)
        .put(0, 32)
        .put(2, 8)
        .put(6, 13)
        .put(participant.into(), 13)
        .put(6, 9)
        .put_span(payload.span());
    out
}
#[test]
fn playback_wire_is_a_bounded_registry_reference() {
    let (owner, b) = fixture();
    let wire = Play {
        endpoint: b.playing,
        participant: owner.participant,
        video: b.video,
    }
    .encode()
    .unwrap();
    let e = Envelope::decode(
        wire.span(),
        Limits {
            max_input_bits: 4096,
            max_references: 2,
            max_payload_bytes: 256,
        },
    )
    .unwrap();
    let r = e.route(RouteProfile::ClientReceive).unwrap();
    assert_eq!(e.references(), [6, 9]);
    assert_eq!(r.method_index(), 2);
    assert_eq!(r.arguments().read_u32(0, 2).unwrap(), 2);
    assert_eq!(r.arguments().read_u32(2, 11).unwrap(), 7);
    // Inclusive native range [0,count] reserves an extra bit at powers of two.
    assert_eq!(r.arguments().read_u32(13, 4).unwrap(), 4);
    assert_eq!(r.arguments().len(), 21);
    for video in [
        Video {
            bundle: 0,
            ..b.video
        },
        Video {
            bundle: 2003,
            ..b.video
        },
        Video {
            count: 0,
            ..b.video
        },
        Video {
            index: 8,
            ..b.video
        },
    ] {
        assert!(
            Play {
                endpoint: b.playing,
                participant: owner.participant,
                video
            }
            .encode()
            .is_err()
        );
    }
    assert!(
        Play {
            endpoint: endpoint(512),
            participant: owner.participant,
            video: b.video
        }
        .encode()
        .is_err()
    );
}
#[test]
fn acknowledgements_require_owner_and_phase_and_retries_are_inert() {
    let (owner, b) = fixture();
    let (mut active, begin) = Active::start(owner, b).unwrap();
    assert_eq!(begin.len(), 1);
    let original = active.clone();
    assert!(active.receive(completed(9, 127).span(), owner).is_err());
    assert_eq!(active, original);
    assert_eq!(active.before_completed(owner).unwrap().len(), 3);
    assert!(active.before_completed(owner).unwrap().is_empty());
    let playing = active.clone();
    assert!(active.receive(completed(8, 0).span(), owner).is_err());
    assert!(
        active
            .receive(
                completed(9, 0).span(),
                Owner {
                    connection: 5,
                    ..owner
                }
            )
            .is_err()
    );
    assert!(active.after_completed(owner).is_err());
    assert_eq!(active, playing);
    for bits in 0..completed(9, 127).len() {
        assert!(
            active
                .receive(completed(9, 127).span().slice(0, bits).unwrap(), owner)
                .is_err()
        );
        assert_eq!(active, playing);
    }
    assert_eq!(
        active
            .receive(completed(9, 127).span(), owner)
            .unwrap()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(active.phase(), Phase::After);
    assert!(
        active
            .receive(completed(9, 0).span(), owner)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert_eq!(active.after_completed(owner).unwrap().len(), 1);
    assert!(active.after_completed(owner).unwrap().is_empty());
    assert_eq!(active.phase(), Phase::Finished);
}
#[test]
fn independent_movies_do_not_advance_each_other() {
    let (owner, b) = fixture();
    let other = Owner {
        participant: 10,
        persona: 18,
        ..owner
    };
    let (mut a, _) = Active::start(owner, b).unwrap();
    let (mut c, _) = Active::start(other, b).unwrap();
    a.before_completed(owner).unwrap();
    c.before_completed(other).unwrap();
    a.receive(completed(9, 0).span(), owner).unwrap();
    assert!(c.receive(completed(9, 0).span(), other).is_err());
    assert_eq!(c.phase(), Phase::Playing);
    assert!(
        Active::start(
            owner,
            Bindings {
                after: b.playing,
                ..b
            }
        )
        .is_err()
    );
}
