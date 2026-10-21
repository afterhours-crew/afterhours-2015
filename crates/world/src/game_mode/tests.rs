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
