// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use creation::{Asset, Catalog, Content, Creation, Prefix};

fn profile() -> Profile {
    Profile::new(&[
        Kind::Root,
        Kind::RpcBoolUpdate,
        Kind::RpcChaseUpdate,
        Kind::RpcCopFineTracker,
        Kind::RpcCopMapUpdate,
        Kind::RpcTargetVisibilityUpdate,
        Kind::BoolProperty,
    ])
    .unwrap()
}

fn wire(creation: bool) -> BitWriter {
    // Independent bit construction in native reader order, including a following
    // unrelated property. Values exercise sign bits, null and maximum references.
    let mut w = BitWriter::new();
    if creation {
        w.put(1, 16).put(0, 9).put(7, 10).put(0, 13);
        for selector in 1..=5 {
            w.put(selector, 9).put(7, 10);
            if selector == 3 {
                w.put(0x80, 8).put(0x7f, 8).put(0xff, 8).put(2, 8);
                w.put(0xff, 8).put(0x80, 8).put(0x8000, 16);
                w.put(3, 8).put(4, 8).put(0x7fff, 16);
            }
        }
        w.put(1, 1).put(0, 1);
    }
    w.put(1, 1); // Root, no body.
    w.put(1, 1).put(1, 1).put(1, 1); // Audio Boolean.
    w.put(1, 1)
        .put(1, 1)
        .put(0xab, 8)
        .put(1, 1)
        .put(8191, 13)
        .put(1, 1)
        .put(0, 1);
    w.put(1, 1).put(1, 1).put(0xfe, 8); // Fine signed byte.
    w.put(1, 1).put(1, 1).put(2, 6).put(0, 13).put(8191, 13);
    w.put(1, 1)
        .put(1, 1)
        .put(7, 13)
        .put(1, 1)
        .put(2, 16)
        .put(2, 13)
        .put(8191, 13);
    w.put(1, 1).put(1, 1).put(1, 1); // Following ordinary property.
    w
}

#[test]
fn pursuit_fields_have_exact_native_widths_and_signed_values() {
    let profile = profile();
    for creation in [false, true] {
        let wire = wire(creation);
        let decoded = Body::decode(wire.span(), &profile, creation).unwrap();
        assert_eq!(decoded.initial_bits, creation.then_some(241));
        assert_eq!(decoded.bits, if creation { 376 } else { 135 });
        assert_eq!(decoded.bits, wire.len());
        let body = &decoded.body;
        assert_eq!(
            body.updates[2],
            Some(Update::Chase(ChaseUpdate {
                value: Some(0xab),
                participant: Some(8191),
                flag: Some(false)
            }))
        );
        assert_eq!(body.updates[3], Some(Update::CopFineTracker(Some(-2))));
        assert_eq!(body.updates[4], Some(Update::CopMap(Some(vec![0, 8191]))));
        assert_eq!(
            body.updates[5],
            Some(Update::TargetVisibility(TargetVisibilityUpdate {
                participant: Some(7),
                references: Some(vec![2, 8191])
            }))
        );
        assert_eq!(body.updates[6], Some(Update::BoolProperty(Some(true))));
        if let Some(initial) = &body.initial {
            assert_eq!(
                initial[3],
                Initial::RpcCopFineTracker {
                    rpc: Rpc {
                        selector: 3,
                        serial: Serial::new(7).unwrap()
                    },
                    snapshot: FineSnapshot {
                        values: [-128, 127, -1],
                        entries: vec![(-1, -128, -32768), (3, 4, 32767)]
                    }
                }
            );
            for (index, value) in initial[1..6].iter().enumerate() {
                assert_eq!(value.rpc().unwrap().selector, index as u16 + 1);
            }
        }
        assert_eq!(body.encode(&profile).unwrap(), wire);
        for end in 0..wire.len() {
            assert!(
                Body::decode(wire.span().slice(0, end).unwrap(), &profile, creation).is_err(),
                "{creation}/{end}"
            );
        }
        let mut next = wire.clone();
        next.put(0xabc, 12);
        assert_eq!(
            Body::decode(next.span(), &profile, creation).unwrap(),
            decoded
        );
    }
}

#[test]
fn absent_updates_empty_lists_and_false_values_remain_distinct() {
    let profile = profile();
    let mut w = BitWriter::new();
    w.put(0, 1).put(1, 1).put(0, 1); // Root absent, audio unchanged.
    w.put(1, 1).put(0, 1).put(0, 1).put(0, 1); // Three independent unchanged fields.
    w.put(1, 1).put(0, 1);
    w.put(1, 1).put(1, 1).put(0, 6); // Explicit empty map list.
    w.put(1, 1).put(0, 1).put(1, 1).put(0, 16); // Explicit empty target list.
    w.put(1, 1).put(1, 1).put(0, 1); // Present false property.
    let body = Body::decode(w.span(), &profile, false).unwrap().body;
    assert_eq!(body.updates[0], None);
    assert_eq!(body.updates[1], Some(Update::BoolProperty(None)));
    assert_eq!(body.updates[2], Some(Update::Chase(ChaseUpdate::default())));
    assert_eq!(body.updates[3], Some(Update::CopFineTracker(None)));
    assert_eq!(body.updates[4], Some(Update::CopMap(Some(vec![]))));
    assert_eq!(
        body.updates[5],
        Some(Update::TargetVisibility(TargetVisibilityUpdate {
            participant: None,
            references: Some(vec![])
        }))
    );
    assert_eq!(body.updates[6], Some(Update::BoolProperty(Some(false))));
    assert_eq!(body.encode(&profile).unwrap(), w);
}

#[test]
fn fine_and_reference_lists_reject_negative_or_excessive_counts() {
    let profile = Profile::new(&[Kind::Root, Kind::RpcCopFineTracker]).unwrap();
    for count in [128, 255] {
        let mut w = BitWriter::new();
        w.put(0, 48).put(0, 19).put(0, 24).put(count, 8);
        assert_eq!(Body::decode(w.span(), &profile, true), Err(Error::Bound));
    }
    let target = Profile::new(&[Kind::Root, Kind::RpcTargetVisibilityUpdate]).unwrap();
    let mut w = BitWriter::new();
    w.put(0, 1).put(1, 1).put(0, 1).put(1, 1).put(1025, 16);
    assert_eq!(Body::decode(w.span(), &target, false), Err(Error::Bound));
}

#[test]
fn maximal_lists_roundtrip_and_encoding_rejects_overflow_and_wrong_types() {
    let profile = profile();
    let mut body = Body::decode(wire(true).span(), &profile, true)
        .unwrap()
        .body;
    let Initial::RpcCopFineTracker { snapshot, .. } = &mut body.initial.as_mut().unwrap()[3] else {
        panic!()
    };
    snapshot.entries = vec![(-128, 127, -32768); 127];
    body.updates[4] = Some(Update::CopMap(Some(vec![8191; 63])));
    body.updates[5] = Some(Update::TargetVisibility(TargetVisibilityUpdate {
        participant: Some(8191),
        references: Some(vec![8191; MAX_REFERENCES]),
    }));
    let w = body.encode(&profile).unwrap();
    assert_eq!(Body::decode(w.span(), &profile, true).unwrap().body, body);
    let mut invalid = body.clone();
    let Initial::RpcCopFineTracker { snapshot, .. } = &mut invalid.initial.as_mut().unwrap()[3]
    else {
        panic!()
    };
    snapshot.entries.push((0, 0, 0));
    assert_eq!(invalid.encode(&profile), Err(Error::Bound));
    for update in [
        Update::CopMap(Some(vec![0; 64])),
        Update::CopMap(Some(vec![8192])),
    ] {
        let mut invalid = body.clone();
        invalid.updates[4] = Some(update);
        assert!(invalid.encode(&profile).is_err());
    }
    let mut invalid = body.clone();
    invalid.updates[5] = Some(Update::TargetVisibility(TargetVisibilityUpdate {
        participant: None,
        references: Some(vec![0; MAX_REFERENCES + 1]),
    }));
    assert_eq!(invalid.encode(&profile), Err(Error::Bound));
    for index in 1..=5 {
        let mut invalid = body.clone();
        invalid.updates[index] = Some(Update::Noop);
        assert_eq!(invalid.encode(&profile), Err(Error::TypeMismatch));
    }
}

#[test]
fn inspection_support_cannot_silently_become_owned_police_gameplay() {
    for kind in profile()
        .kinds()
        .iter()
        .copied()
        .filter(|k| k.inspection_only())
    {
        let asset = Asset {
            bundle: 1,
            type_id: 1,
            local_index: 0,
        };
        let content = Content::new(
            vec![Catalog::new(1, &[(1, 1)]).unwrap()],
            vec![(asset, Profile::new(&[Kind::Root, kind]).unwrap())],
        )
        .unwrap();
        let creation = Creation {
            prefix: Prefix {
                parent: None,
                blueprint: 0,
                sub_id: 0,
                owner: None,
                asset,
            },
            body: Body {
                initial: None,
                updates: vec![],
            },
        };
        assert_eq!(
            state::State::new(creation, &content, |_| true),
            Err(Error::Unsupported)
        );
    }
}
