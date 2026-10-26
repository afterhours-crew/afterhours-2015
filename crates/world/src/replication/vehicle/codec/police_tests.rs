// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn wire(bits: &str) -> BitWriter {
    let mut out = BitWriter::new();
    for bit in bits.bytes() {
        out.put(u64::from(bit - b'0'), 1);
    }
    out
}

#[test]
fn police_initializers_keep_the_following_serializer_aligned() {
    let rpc = Rpc {
        selector: 0x155,
        serial: Serial::new(0x2aa).unwrap(),
    };
    // Independently packed native field order: selector9, serial10, then flags.
    for (kind, initial, bits) in [
        (
            Kind::SearchLight,
            Initial::SearchLight {
                rpc: rpc.clone(),
                enabled: true,
            },
            "10101010110101010101",
        ),
        (
            Kind::Siren,
            Initial::Siren {
                rpc: rpc.clone(),
                flags: [false, true],
            },
            "101010101101010101001",
        ),
    ] {
        let body = wire(bits);
        let mut input = body.clone();
        input.put(5, 3);
        let mut r = Reader {
            span: input.span(),
            pos: 0,
        };
        assert_eq!(initial_read(&mut r, &kind).unwrap(), initial);
        assert_eq!(r.pos, bits.len());
        assert_eq!(r.take(3).unwrap(), 5);
        for end in 0..bits.len() {
            let mut r = Reader {
                span: BitSpan::new(body.bytes(), 0, end).unwrap(),
                pos: 0,
            };
            assert_eq!(initial_read(&mut r, &kind), Err(Error::Truncated));
        }
        let mut generated = Writer(BitWriter::new());
        initial_write(&mut generated, &kind, &initial).unwrap();
        assert_eq!(generated.0, body);
    }
}

#[test]
fn police_updates_preserve_absence_false_and_payload_free_trigger() {
    for first in [None, Some(false), Some(true)] {
        for second in [None, Some(false), Some(true)] {
            for trigger in [false, true] {
                let flags = [first, second];
                let mut expected = String::new();
                for flag in flags {
                    expected.push_str(match flag {
                        None => "0",
                        Some(false) => "10",
                        Some(true) => "11",
                    });
                }
                expected.push(if trigger { '1' } else { '0' });
                let expected = wire(&expected);
                let update = Update::Siren { flags, trigger };
                let mut input = expected.clone();
                input.put(0b110, 3);
                let mut r = Reader {
                    span: input.span(),
                    pos: 0,
                };
                assert_eq!(update_read(&mut r, &Kind::Siren).unwrap(), update);
                assert_eq!(r.pos, expected.len());
                assert_eq!(r.take(3).unwrap(), 6);
                for end in 0..expected.len() {
                    let mut r = Reader {
                        span: BitSpan::new(expected.bytes(), 0, end).unwrap(),
                        pos: 0,
                    };
                    assert_eq!(update_read(&mut r, &Kind::Siren), Err(Error::Truncated));
                }
                let mut generated = Writer(BitWriter::new());
                update_write(&mut generated, &Kind::Siren, &update).unwrap();
                assert_eq!(generated.0, expected);
            }
        }
        let expected = wire(match first {
            None => "0",
            Some(false) => "10",
            Some(true) => "11",
        });
        let mut r = Reader {
            span: expected.span(),
            pos: 0,
        };
        assert_eq!(
            update_read(&mut r, &Kind::SearchLight).unwrap(),
            Update::SearchLight(first)
        );
        assert_eq!(r.pos, expected.len());
    }
    let profile = Profile::new(vec![
        Kind::Root {
            property_owner: false,
        },
        Kind::Siren,
        Kind::SearchLight,
        Kind::Bool,
    ])
    .unwrap();
    // Absent root; siren present with false/absent/trigger; light absent; final
    // Boolean present and true. The notification has no following value bit.
    let expected = wire("0110010111");
    let body = Body {
        creation: None,
        updates: vec![
            None,
            Some(Update::Siren {
                flags: [Some(false), None],
                trigger: true,
            }),
            None,
            Some(Update::Bool(Some(true))),
        ],
    };
    assert_eq!(
        Body::decode(expected.span(), &profile, false).unwrap().body,
        body
    );
    assert_eq!(body.encode(&profile).unwrap(), expected);
}

#[test]
fn police_state_retains_flags_without_replaying_notifications() {
    let profile = Profile::new(vec![
        Kind::Root {
            property_owner: false,
        },
        Kind::Siren,
        Kind::SearchLight,
    ])
    .unwrap();
    let rpc = components::fresh_rpcs(&profile);
    let root = Initial::Root(Box::new(RootInitial {
        property_value: None,
        position: Vector::FloatBits([0; 3]),
        rotation: Quaternion25 {
            negative_w: false,
            axes: std::array::from_fn(|_| Axis25 {
                negative: false,
                mantissa: None,
            }),
        },
        flag: false,
        reference: None,
        rpc: rpc[0].clone().unwrap(),
        fine_position: Vector::FloatBits([0; 3]),
        fine_rotation: Quaternion25 {
            negative_w: false,
            axes: std::array::from_fn(|_| Axis25 {
                negative: false,
                mantissa: None,
            }),
        },
        fine_flag: false,
    }));
    let body = Body {
        creation: Some(Creation {
            setup: None,
            connection_id: 0,
            fields: vec![
                root,
                Initial::Siren {
                    rpc: rpc[1].clone().unwrap(),
                    flags: [false; 2],
                },
                Initial::SearchLight {
                    rpc: rpc[2].clone().unwrap(),
                    enabled: false,
                },
            ],
        }),
        updates: vec![
            None,
            Some(Update::Siren {
                flags: [Some(true), Some(false)],
                trigger: true,
            }),
            Some(Update::SearchLight(Some(true))),
        ],
    };
    let mut state = state::State::new(profile, body, |_| false).unwrap();
    assert_eq!(
        state.snapshot().updates[1],
        Some(Update::Siren {
            flags: [Some(true), Some(false)],
            trigger: false
        })
    );
    state
        .apply(
            vec![
                None,
                Some(Update::Siren {
                    flags: [None, Some(true)],
                    trigger: true,
                }),
                Some(Update::SearchLight(Some(false))),
            ],
            |_| false,
        )
        .unwrap();
    assert_eq!(
        state.snapshot().updates[1],
        Some(Update::Siren {
            flags: [Some(true), Some(true)],
            trigger: false
        })
    );
    let before = state.snapshot();
    assert_eq!(
        state.apply(vec![None, Some(Update::Wheel(Some(false))), None], |_| {
            false
        }),
        Err(Error::TypeMismatch)
    );
    assert_eq!(state.snapshot(), before);
    state.apply(vec![None, None, None], |_| false).unwrap();
    assert_eq!(state.snapshot(), before);
    assert_eq!(
        state.snapshot().updates[2],
        Some(Update::SearchLight(Some(false)))
    );
}
