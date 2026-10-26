use super::*;
use crate::replication::{
    Kind, debris,
    section::{Bindings, Section, Setup},
};

fn content(count: u16) -> Content {
    Content::new(vec![
        Catalog::new(7, count, &[(1, Composition::RaceSpawner)]).unwrap(),
    ])
    .unwrap()
}
fn record(fields: Option<References>, parent: Option<Parent>) -> Record {
    Record::from_direct_entity(
        41,
        Prefix {
            parent,
            blueprint: 91,
            sub_id: 0xabcdef12,
            asset: Asset {
                bundle: 7,
                index: 1,
            },
        },
        fields,
        &content(8),
    )
    .unwrap()
}
fn section(deleted: Vec<u16>, records: Vec<Record>) -> Section {
    Section {
        float_bits: None,
        flag: false,
        deleted,
        setup: (!records.is_empty()).then_some(Setup::Packed {
            tag: None,
            width: None,
            axes: [None; 3],
        }),
        records,
    }
}

#[test]
fn native_direct_header_and_optional_references_have_independent_boundaries() {
    // Independently packed native field widths; count=8 requires FOUR bits.
    let mut w = BitWriter::new();
    w.put(41, 13)
        .put(1, 1)
        .put(6, 5)
        .put(0, 1)
        .put(0, 1)
        .put(91, 13)
        .put(0xabcdef12, 32)
        .put(2, 2)
        .put(7, 11)
        .put(1, 4);
    let initial_bits = w.len();
    w.put(1, 1).put(1, 1).put(0, 13).put(1, 1).put(8191, 13);
    let bits = w.len();
    let expected = record(
        Some(References {
            first: Some(0),
            second: Some(8191),
        }),
        None,
    );
    assert_eq!(expected.encode().unwrap(), w);
    // Unrelated following object must begin at precisely the decoded boundary.
    let following = Record {
        id: 2,
        initial: Some(Initial::DebrisManager(debris::Initial {
            words: [0xfeedbeef, 17],
        })),
        update: Update::DebrisManager(debris::Update {
            values: [Some(3), None],
        }),
    };
    w.put_span(following.encode().unwrap().span());
    let decoded =
        Record::decode_direct_entity(w.span(), 41, true, Some(&content(8)), None).unwrap();
    assert_eq!(decoded.record, expected);
    assert_eq!(decoded.initial_bits, Some(initial_bits));
    assert_eq!(decoded.bits, bits);
    assert_eq!(
        Record::decode(w.span().after(bits).unwrap(), None)
            .unwrap()
            .record,
        following
    );
    for end in 0..bits {
        assert!(
            Record::decode_direct_entity(
                w.span().slice(0, end).unwrap(),
                41,
                true,
                Some(&content(8)),
                None
            )
            .is_err(),
            "end={end}"
        );
    }
}

#[test]
fn parent_and_absent_fields_round_trip_without_equating_absent_with_null() {
    for parent in [
        None,
        Some(Parent { reference: None }),
        Some(Parent { reference: Some(0) }),
        Some(Parent {
            reference: Some(8191),
        }),
    ] {
        for fields in [
            None,
            Some(References::default()),
            Some(References {
                first: Some(0),
                second: None,
            }),
            Some(References {
                first: None,
                second: Some(81),
            }),
        ] {
            let expected = record(fields, parent.clone());
            let w = expected.encode().unwrap();
            let decoded =
                Record::decode_direct_entity(w.span(), 41, true, Some(&content(8)), None).unwrap();
            assert_eq!(decoded.record, expected);
            assert_eq!(decoded.bits, w.len());
        }
    }
    let mut invalid = record(None, None);
    if let Update::DirectEntity { fields, .. } = &mut invalid.update {
        *fields = Some(References {
            first: Some(8192),
            second: None,
        });
    }
    assert_eq!(invalid.encode(), Err(Error::Shape));
    assert!(Catalog::new(7, 0, &[]).is_err());
    assert!(Catalog::new(2048, 8, &[]).is_err());
    assert!(Catalog::new(7, 8, &[(8, Composition::RaceSpawner)]).is_err());
    assert_eq!(
        content(8).binding(Asset {
            bundle: 7,
            index: 0
        }),
        Err(Error::Unsupported)
    );
    assert_eq!(
        content(8).binding(Asset {
            bundle: 7,
            index: 8
        }),
        Err(Error::Shape)
    );
    let duplicate = Catalog::new(7, 8, &[]).unwrap();
    assert!(Content::new(vec![duplicate.clone(), duplicate]).is_err());
}

#[test]
fn section_bindings_are_transactional_retained_and_removed_with_object_lifetime() {
    let create = record(Some(References::default()), None);
    let mut update = create.clone();
    update.initial = None;
    let incoming = section(vec![], vec![create.clone(), update.clone()]);
    let w = incoming.encode().unwrap();
    let mut bindings = Bindings::default().with_direct_entities(content(8));
    let original = bindings.clone();
    for end in 0..w.len() {
        assert!(bindings.decode(w.span().slice(0, end).unwrap()).is_err());
        assert_eq!(bindings, original);
    }
    let partial = bindings.decode_partial(w.span()).unwrap();
    assert!(partial.failure.is_none());
    assert_eq!(partial.records, incoming.records);
    assert_eq!(bindings, original);
    assert_eq!(bindings.decode(w.span()).unwrap().section, incoming);
    assert_eq!(bindings.get(41), Some(Kind::Entity));
    let mut applied = original.clone();
    applied.apply(&incoming).unwrap();
    assert_eq!(applied, bindings);
    let before = bindings.clone();
    assert_eq!(
        bindings.apply(&section(vec![], vec![create.clone()])),
        Err(Error::DuplicateObject)
    );
    assert_eq!(bindings, before);
    // New registration tables do not change an existing object's binding.
    bindings = bindings.with_direct_entities(content(16));
    bindings
        .apply(&section(vec![], vec![update.clone()]))
        .unwrap();
    let mut wrong = update.clone();
    if let Update::DirectEntity { binding, .. } = &mut wrong.update {
        *binding = Arc::new(content(16).binding(binding.asset()).unwrap());
    }
    assert_eq!(
        bindings.apply(&section(vec![], vec![wrong])),
        Err(Error::TypeMismatch)
    );
    bindings.apply(&section(vec![41], vec![])).unwrap();
    assert_eq!(
        bindings.apply(&section(vec![], vec![update.clone()])),
        Err(Error::UnknownObject)
    );
    assert_eq!(
        bindings.apply(&section(vec![], vec![create])),
        Err(Error::TypeMismatch)
    );
    assert!(bindings.is_empty());
    assert_eq!(
        original.clone().apply(&section(vec![], vec![update])),
        Err(Error::UnknownObject)
    );
    applied.reset();
    assert!(applied.is_empty());
    assert_eq!(applied, original);
}
