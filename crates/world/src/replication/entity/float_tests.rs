// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use creation::{Asset, Catalog, Content, Creation, Prefix};

#[test]
fn float_creation_and_delta_preserve_bits_and_reject_every_truncation() {
    let profile = Profile::new(&[Kind::Root, Kind::FloatProperty, Kind::I32Property]).unwrap();
    for value in [
        None,
        Some(0),
        Some(0x8000_0000),
        Some(0x7fc0_0123),
        Some(0x7f80_0000),
    ] {
        // Independently construct the body: root, optional float, absent int,
        // outer update mask, optional float update, and signed int update.
        let mut wire = BitWriter::new();
        wire.put(1, 16).put(0, 9).put(2, 10).put(5, 13);
        wire.put(u64::from(value.is_some()), 1);
        if let Some(bits) = value {
            wire.put(u64::from(bits), 32);
        }
        wire.put(0, 1)
            .put(0, 1)
            .put(1, 1)
            .put(1, 1)
            .put(0x8000_0000, 32)
            .put(1, 1)
            .put(1, 1)
            .put(0xffff_fffc, 32);
        let decoded = Body::decode(wire.span(), &profile, true).unwrap();
        assert_eq!(decoded.bits, wire.len());
        assert_eq!(
            decoded.body.initial.as_ref().unwrap()[1],
            Initial::FloatProperty(value)
        );
        assert_eq!(
            decoded.body.updates[1],
            Some(Update::FloatProperty(Some(0x8000_0000)))
        );
        assert_eq!(decoded.body.updates[2], Some(Update::I32Property(Some(-4))));
        assert_eq!(decoded.body.encode(&profile).unwrap(), wire);
        for end in 0..wire.len() {
            assert!(Body::decode(wire.span().slice(0, end).unwrap(), &profile, true).is_err());
        }
        let mut wrong = decoded.body;
        wrong.updates[1] = Some(Update::I32Property(Some(1)));
        assert_eq!(wrong.encode(&profile), Err(Error::TypeMismatch));
        wrong.updates[1] = Some(Update::FloatProperty(None));
        wrong.initial.as_mut().unwrap()[1] = Initial::I32Property(None);
        assert_eq!(wrong.encode(&profile), Err(Error::TypeMismatch));
    }
}

#[test]
fn float_state_is_atomic_repeatable_and_isolated() {
    let asset = Asset {
        bundle: 2,
        type_id: 7,
        local_index: 0,
    };
    let profile = Profile::new(&[Kind::Root, Kind::FloatProperty, Kind::BoolProperty]).unwrap();
    let content = Content::new(
        vec![Catalog::new(2, &[(7, 1)]).unwrap()],
        vec![(asset, profile)],
    )
    .unwrap();
    let creation = Creation {
        prefix: Prefix {
            parent: None,
            blueprint: 3,
            sub_id: 1,
            owner: Some(5),
            asset,
        },
        body: Body {
            initial: Some(vec![
                Initial::Root {
                    value: 1,
                    rpc: Rpc {
                        selector: 0,
                        serial: Serial::new(1).unwrap(),
                    },
                    reference: 5,
                },
                Initial::FloatProperty(Some(0x7fc0_0123)),
                Initial::BoolProperty(None),
            ]),
            updates: vec![None, None, None],
        },
    };
    let mut first = state::State::new(creation.clone(), &content, |_| true).unwrap();
    let other = state::State::new(creation, &content, |_| true).unwrap();
    let before = first.clone();
    assert_eq!(
        first.apply(vec![
            None,
            Some(Update::FloatProperty(Some(0))),
            Some(Update::I32Property(None))
        ]),
        Err(Error::TypeMismatch)
    );
    assert_eq!(first, before);
    let delta = vec![None, Some(Update::FloatProperty(Some(0x8000_0000))), None];
    first.apply(delta.clone()).unwrap();
    let saved = first.snapshot();
    first.apply(delta).unwrap();
    first
        .apply(vec![None, Some(Update::FloatProperty(None)), None])
        .unwrap();
    assert_eq!(first.snapshot(), saved);
    assert_eq!(
        saved.body.initial.as_ref().unwrap()[1],
        Initial::FloatProperty(Some(0x8000_0000))
    );
    assert_eq!(
        other.snapshot().body.initial.as_ref().unwrap()[1],
        Initial::FloatProperty(Some(0x7fc0_0123))
    );
    let wire = saved.encode(&content).unwrap();
    assert_eq!(
        Creation::decode(wire.span(), &content).unwrap().creation,
        saved
    );
}
