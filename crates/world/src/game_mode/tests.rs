// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn call(method: u32, reference: Option<u16>, arguments: Arguments) -> Call {
    Call {
        target: 1503,
        selector: 7,
        serial: Serial::new(1).unwrap(),
        method,
        reference,
        arguments,
    }
}

/// Decode a host call the way the client reads it.
fn route(wire: &BitWriter) -> (Vec<u16>, u16, u32, Vec<bool>) {
    let e = Envelope::decode(
        wire.span(),
        Limits {
            max_input_bits: 4096,
            max_references: 8,
            max_payload_bytes: 64,
        },
    )
    .unwrap();
    let r = e.route(RouteProfile::ClientReceive).unwrap();
    assert_eq!(r.serial().map(|s| s.value()), Some(1));
    let args = (0..r.arguments().len())
        .map(|i| r.arguments().read_u32(i, 1).unwrap() == 1)
        .collect();
    (
        e.references().to_vec(),
        r.selector(),
        r.method_index(),
        args,
    )
}

fn word(bits: &[bool], at: usize) -> u32 {
    bits[at..at + 32]
        .iter()
        .fold(0, |v, b| (v << 1) | u32::from(*b))
}

#[test]
fn calls_carry_the_client_reader_argument_layouts() {
    let (refs, selector, method, args) =
        route(&call(0, Some(351), Arguments::None).encode().unwrap());
    assert_eq!(
        (refs, selector, method, args.len()),
        (vec![1503, 351], 7, 0, 5)
    );
    let (refs, _, method, args) = route(
        &call(3, Some(351), Arguments::Word(762201586))
            .encode()
            .unwrap(),
    );
    assert_eq!((refs.len(), method, args.len()), (2, 3, 37));
    assert_eq!(word(&args, 0), 762201586);
    let time = 4501.2738f64;
    let (refs, _, _, args) = route(&call(2, None, Arguments::time(time)).encode().unwrap());
    assert_eq!((refs, args.len()), (vec![1503], 69));
    let bits = (u64::from(word(&args, 0)) << 32) | u64::from(word(&args, 32));
    assert_eq!(f64::from_bits(bits), time);
    let progress = Arguments::Progress {
        passed: 21,
        next: 0,
        seconds: 147.5754f32.to_bits(),
        lap: 2,
    };
    let (_, _, method, args) = route(&call(2, Some(351), progress).encode().unwrap());
    assert_eq!((method, args.len()), (2, 133));
    assert_eq!(
        (
            word(&args, 0),
            word(&args, 32),
            f32::from_bits(word(&args, 64)),
            word(&args, 96)
        ),
        (21, 0, 147.5754, 2)
    );
}

#[test]
fn volume_entries_encode_a_word_and_boolean_before_alignment() {
    for value in [0, 1, 3, 0x8000_0000, u32::MAX] {
        for flag in [false, true] {
            let (refs, selector, method, args) = route(
                &call(0, Some(351), Arguments::VolumeEntry { value, flag })
                    .encode()
                    .unwrap(),
            );
            assert_eq!((refs, selector, method), (vec![1503, 351], 7, 0));
            assert_eq!(args.len(), 37);
            assert_eq!(word(&args, 0), value);
            assert_eq!(args[32], flag);
            assert!(args[33..].iter().all(|bit| !bit));
        }
    }
    for (method, reference) in [(1, Some(351)), (0, None)] {
        assert_eq!(
            call(
                method,
                reference,
                Arguments::VolumeEntry {
                    value: 0,
                    flag: false,
                },
            )
            .encode(),
            Err(Error::Shape)
        );
    }
}

#[test]
fn event_positions_include_membership_before_alignment_padding() {
    // A word-only payload has the same byte size, but its zero padding tells
    // the client's collector to remove the racer instead of updating it.
    for position in [0, 1, 6, 255] {
        for active in [false, true] {
            let (refs, _, method, args) = route(
                &call(0, Some(351), Arguments::EventPosition { position, active })
                    .encode()
                    .unwrap(),
            );
            assert_eq!((refs, method, args.len()), (vec![1503, 351], 0, 37));
            assert_eq!(word(&args, 0), position);
            assert_eq!(args[32], active);
            assert!(args[33..].iter().all(|b| !b));
        }
    }
    assert_eq!(
        call(
            0,
            Some(351),
            Arguments::EventPosition {
                position: 256,
                active: true
            },
        )
        .encode(),
        Err(Error::Bound)
    );
}

#[test]
fn calls_refuse_invalid_identities_and_times() {
    for reference in [Some(0), Some(8192)] {
        assert_eq!(
            call(0, reference, Arguments::None).encode(),
            Err(Error::Bound)
        );
    }
    let mut c = call(0, None, Arguments::None);
    c.selector = 512;
    assert_eq!(c.encode(), Err(Error::Bound));
    for t in [f64::NAN, f64::INFINITY, -1.] {
        assert_eq!(
            call(2, None, Arguments::time(t)).encode(),
            Err(Error::Shape)
        );
    }
    let bad = Arguments::Progress {
        passed: 0,
        next: 1,
        seconds: f32::NAN.to_bits(),
        lap: 1,
    };
    assert_eq!(call(2, Some(351), bad).encode(), Err(Error::Shape));
}

fn client(refs: &[u16], selector: u16, method: u32, args: &[(u64, usize)]) -> BitWriter {
    let mut payload = BitWriter::new();
    payload.put(selector.into(), 9).put(method.into(), 32);
    for (v, n) in args {
        payload.put(*v, *n);
    }
    let mut body = BitWriter::new();
    body.put(0, 32).put(0, 32).put(refs.len() as u64, 8);
    for r in refs {
        body.put((*r).into(), 13);
    }
    body.put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}

#[test]
fn leave_requests_are_root_method_zero_with_the_participant() {
    let wire = client(&[1503, 351], 0, 0, &[(0, 7)]);
    assert_eq!(
        Leave::decode(wire.span()),
        Ok(Some(Leave {
            game_mode: 1503,
            participant: 351
        }))
    );
    assert_eq!(
        Leave::decode(client(&[1503, 351], 0, 1, &[(0, 7)]).span()),
        Ok(None)
    );
    assert_eq!(
        Leave::decode(client(&[1503, 351], 3, 0, &[(0, 7)]).span()),
        Ok(None)
    );
    assert_eq!(
        Leave::decode(client(&[1503], 0, 0, &[(0, 7)]).span()),
        Err(Error::Shape)
    );
    for end in 0..wire.len() {
        assert!(Leave::decode(wire.span().slice(0, end).unwrap()).is_err());
    }
}

#[test]
fn request_to_final_is_distinct_from_leave_and_validates_the_same_participant_envelope() {
    for (method, kind) in [(0, RootRequestKind::Leave), (8, RootRequestKind::GoToFinal)] {
        let wire = client(&[1503, 351], 0, method, &[(0, 7)]);
        assert_eq!(
            RootRequest::decode(wire.span()),
            Ok(Some(RootRequest {
                game_mode: 1503,
                participant: 351,
                kind
            }))
        );
        for end in 0..wire.len() {
            assert!(RootRequest::decode(wire.span().slice(0, end).unwrap()).is_err());
        }
        for refs in [&[1503][..], &[1503, 351, 7][..]] {
            assert!(RootRequest::decode(client(refs, 0, method, &[(0, 7)]).span()).is_err());
        }
        assert_eq!(
            RootRequest::decode(client(&[1503, 0], 0, method, &[(0, 7)]).span()),
            Err(Error::Bound)
        );
        assert_eq!(
            RootRequest::decode(client(&[1503, 351], 1, method, &[(0, 7)]).span()),
            Ok(None)
        );
        assert!(RootRequest::decode(client(&[1503, 351], 0, method, &[(0, 15)]).span()).is_err());
    }
    assert_eq!(
        Leave::decode(client(&[1503, 351], 0, 8, &[(0, 7)]).span()),
        Ok(None)
    );
    assert_eq!(
        RootRequest::decode(client(&[1503, 351], 0, 9, &[(0, 7)]).span()),
        Ok(None)
    );
}

#[test]
fn crossings_carry_the_vehicle_side_and_time() {
    let t = 4522.729f64.to_bits();
    let wire = client(
        &[313, 495],
        106,
        2,
        &[(t >> 32, 32), (t & 0xffff_ffff, 32), (93, 7)],
    );
    assert_eq!(
        Crossing::decode(wire.span()),
        Ok(Some(Crossing {
            scene: 313,
            selector: 106,
            vehicle: 495,
            inside: true,
            seconds: 4522.729
        }))
    );
    let out = client(
        &[313, 495],
        106,
        3,
        &[(t >> 32, 32), (t & 0xffff_ffff, 32), (0, 7)],
    );
    assert!(!Crossing::decode(out.span()).unwrap().unwrap().inside);
    let nan = f64::NAN.to_bits();
    let bad = client(
        &[313, 495],
        106,
        2,
        &[(nan >> 32, 32), (nan & 0xffff_ffff, 32), (0, 7)],
    );
    assert_eq!(Crossing::decode(bad.span()), Err(Error::Shape));
    // Other methods and shapes are not crossings.
    assert_eq!(
        Crossing::decode(client(&[313, 495], 106, 0, &[(0, 7)]).span()),
        Ok(None)
    );
    assert_eq!(
        Crossing::decode(
            client(
                &[313],
                106,
                2,
                &[(t >> 32, 32), (t & 0xffff_ffff, 32), (0, 7)]
            )
            .span()
        ),
        Ok(None)
    );
}

/// E1026 911.825 s: gameplay s58 m0 `[240, 454]` with the running total 33
/// and the award 33, then five alignment bits (a69).
#[test]
fn score_award_encodes_total_then_amount_and_needs_the_participant() {
    let call = Call {
        target: 240,
        selector: 58,
        serial: Serial::new(48).unwrap(),
        method: 0,
        reference: Some(454),
        arguments: Arguments::ScoreAward {
            total: 33,
            amount: 33,
        },
    };
    let body = call.encode().unwrap();
    let bytes = body.bytes();
    let span = nfs_protocol::world::BitSpan::new(bytes, 0, bytes.len() * 8).unwrap();
    // Two envelope words, the count, two 13-bit references, the length.
    let payload = 64 + 8 + 26 + 9;
    let args = payload + 9 + 10 + 32;
    assert_eq!(span.read_u32(args, 32).unwrap(), 33);
    assert_eq!(span.read_u32(args + 32, 32).unwrap(), 33);
    // 115 payload bits round up to 15 bytes: 5 alignment bits.
    assert_eq!(span.read_u32(payload - 9, 9).unwrap(), 15);
    for bad in [
        Call { method: 1, ..call },
        Call {
            reference: None,
            ..call
        },
    ] {
        assert!(bad.encode().is_err());
    }
}
