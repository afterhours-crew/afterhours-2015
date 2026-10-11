// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::replication::entity::{Initial, Kind, Rpc, Update};
use nfs_protocol::world::rpc::Serial;

fn asset() -> Asset {
    Asset {
        bundle: 3,
        type_id: 12,
        local_index: 0,
    }
}
fn content() -> Content {
    Content::new(
        vec![Catalog::new(3, &[(10, 2), (11, 0), (12, 1)]).unwrap()],
        vec![(asset(), Profile::new(&[Kind::Root]).unwrap())],
    )
    .unwrap()
}
fn prefix() -> Prefix {
    Prefix {
        parent: None,
        blueprint: 2,
        sub_id: 1,
        owner: None,
        asset: asset(),
    }
}
fn creation() -> Creation {
    Creation {
        prefix: prefix(),
        body: Body {
            initial: Some(vec![Initial::Root {
                value: 7,
                rpc: Rpc {
                    selector: 0,
                    serial: Serial::new(2).unwrap(),
                },
                reference: 1,
            }]),
            updates: vec![Some(Update::Noop)],
        },
    }
}

#[test]
fn catalog_flattens_across_zero_counts_and_rejects_reserved_or_invalid_indices() {
    let c = Catalog::new(3, &[(10, 2), (11, 0), (12, 1)]).unwrap();
    assert_eq!(c.width(), 2);
    assert_eq!(c.flatten(asset()), Ok(2));
    assert_eq!(c.resolve(2), Ok(asset()));
    assert_eq!(c.resolve(3), Err(Error::Shape));
    assert_eq!(
        c.flatten(Asset {
            local_index: 1,
            ..asset()
        }),
        Err(Error::Shape)
    );
    assert_eq!(Catalog::new(2003, &[(1, 1)]), Err(Error::Unsupported));
    assert_eq!(Catalog::new(3, &[(2, 1), (1, 1)]), Err(Error::Shape));
    assert_eq!(Catalog::new(3, &[(1, u16::MAX), (2, 1)]), Err(Error::Bound));
    assert_eq!(Catalog::new(3, &[(1, 0)]), Err(Error::Shape));
    assert_eq!(Catalog::new(3, &[(1, 128)]).unwrap().width(), 8);
}

#[test]
fn explicit_prefix_fields_close_with_optional_parent_and_owner() {
    let c = content();
    for (parent, owner, expected) in [
        (None, None, 64),
        (Some(Parent { reference: None }), None, 66),
        (Some(Parent { reference: Some(8) }), Some(9), 92),
    ] {
        let p = Prefix {
            parent,
            owner,
            ..prefix()
        };
        let mut encoded = p.encode(&c).unwrap();
        assert_eq!(encoded.len(), expected);
        encoded.put(43, 6);
        let (actual, bits) = Prefix::decode(encoded.span(), &c).unwrap();
        assert_eq!(actual, p);
        assert_eq!(bits, expected);
        assert_eq!(encoded.span().read_u32(bits, 6).unwrap(), 43);
        for end in 0..expected {
            assert!(Prefix::decode(BitSpan::new(encoded.bytes(), 0, end).unwrap(), &c).is_err());
        }
    }
}

#[test]
fn unknown_transform_family_mode_and_catalog_stop_before_body() {
    let c = content();
    let base = prefix().encode(&c).unwrap();
    for bit in [0, 48, 49, 50] {
        let mut bytes = base.bytes().to_vec();
        bytes[bit / 8] ^= 1 << (7 - bit % 8);
        assert_eq!(
            Prefix::decode(BitSpan::new(&bytes, 0, 64).unwrap(), &c),
            Err(Error::Unsupported)
        );
    }
    let mut transform = BitWriter::new();
    transform.put(0b1101, 4);
    assert_eq!(
        Prefix::decode(transform.span(), &c),
        Err(Error::Unsupported)
    );
    let missing = Content::new(vec![Catalog::new(4, &[(1, 1)]).unwrap()], vec![]).unwrap();
    assert_eq!(
        Prefix::decode(base.span(), &missing),
        Err(Error::Unsupported)
    );
    let mut invalid = base.bytes().to_vec();
    invalid[7] |= 1;
    assert_eq!(
        Prefix::decode(BitSpan::new(&invalid, 0, 64).unwrap(), &c),
        Err(Error::Shape)
    );
}

#[test]
fn complete_creation_composes_static_profile_and_preserves_next_body() {
    let c = content();
    let original = creation();
    let mut wire = original.encode(&c).unwrap();
    assert_eq!(wire.len(), 112);
    wire.put(57, 6);
    let d = Creation::decode(wire.span(), &c).unwrap();
    assert_eq!((d.prefix_bits, d.initial_bits, d.bits), (64, 112, 112));
    assert_eq!(d.creation, original);
    assert_eq!(wire.span().read_u32(d.bits, 6).unwrap(), 57);
    for end in 0..112 {
        assert!(Creation::decode(BitSpan::new(wire.bytes(), 0, end).unwrap(), &c).is_err());
    }
    let mut invalid = original.clone();
    invalid.body.initial = None;
    assert_eq!(invalid.encode(&c), Err(Error::Shape));
    let no_profile =
        Content::new(vec![Catalog::new(3, &[(10, 2), (12, 1)]).unwrap()], vec![]).unwrap();
    assert_eq!(
        Creation::decode(wire.span(), &no_profile),
        Err(Error::Unsupported)
    );
}

#[test]
fn static_bindings_are_bounded_unique_and_resolve_real_assets() {
    let c = Catalog::new(3, &[(10, 2), (12, 1)]).unwrap();
    let p = Profile::new(&[Kind::Root]).unwrap();
    assert_eq!(
        Content::new(vec![c.clone(), c.clone()], vec![]),
        Err(Error::Shape)
    );
    assert_eq!(
        Content::new(
            vec![c.clone()],
            vec![(asset(), p.clone()), (asset(), p.clone())]
        ),
        Err(Error::Shape)
    );
    assert_eq!(
        Content::new(
            vec![c.clone()],
            vec![(
                Asset {
                    local_index: 1,
                    ..asset()
                },
                p
            )]
        ),
        Err(Error::Shape)
    );
    assert_eq!(
        Content::new(vec![c; MAX_CATALOGS + 1], vec![]),
        Err(Error::Bound)
    );
}
