// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn catalog() -> Catalog {
    Catalog::new([
        ([0x11; 16], DefinitionClass::CurrencyItemData),
        ([0x22; 16], DefinitionClass::TimedDiscountItemData),
    ])
    .unwrap()
}
fn item(id: u64) -> Item {
    Item {
        id,
        definition: [0x11; 16],
        owner: 0,
        defaults: vec![],
        children: vec![],
        state: 2,
        buy_price: 123,
        sell_price: 45,
        derived: Derived::Empty,
    }
}
fn one() -> Collection {
    Collection {
        roots: vec![7],
        items: [(7, item(7))].into(),
    }
}

#[test]
fn changes_name_new_roots_changed_subtrees_and_removed_roots() {
    let mut before = one();
    before.items.get_mut(&7).unwrap().children = vec![8];
    before.items.insert(8, item(8));
    before.roots.push(10);
    before.items.insert(10, item(10));
    assert_eq!(before.changes_since(&before), Ok(Changes::default()));
    let mut after = before.clone();
    // A child edit changes its root; a new root is added; a vanished root is removed.
    after.items.get_mut(&8).unwrap().buy_price = 1;
    after.roots = vec![11, 7];
    after.items.remove(&10);
    after.items.insert(11, item(11));
    assert_eq!(
        after.changes_since(&before),
        Ok(Changes {
            added: vec![11],
            changed: vec![7],
            removed: vec![10],
        })
    );
    // A new child under an unchanged root record changes that root.
    let mut grown = before.clone();
    grown.items.get_mut(&10).unwrap().children = vec![12];
    grown.items.insert(12, item(12));
    assert_eq!(grown.changes_since(&before).unwrap().changed, [10]);
    let mut broken = after;
    broken.items.get_mut(&7).unwrap().children.push(13);
    assert_eq!(broken.changes_since(&before), Err(Error::MissingItem));
}

#[test]
fn selected_roots_include_shared_defaults_and_children_without_unrelated_items() {
    let mut graph = one();
    graph.items.get_mut(&7).unwrap().defaults = vec![8];
    graph.items.get_mut(&7).unwrap().children = vec![9];
    for id in [8, 9, 10] {
        graph.items.insert(id, item(id));
    }
    graph.items.get_mut(&9).unwrap().children = vec![8];
    graph.roots.push(10);
    let selected = graph.for_roots(vec![9, 7, 9]).unwrap();
    assert_eq!(selected.roots, [9, 7, 9]);
    assert_eq!(
        selected.items.keys().copied().collect::<Vec<_>>(),
        [7, 8, 9]
    );
    assert_eq!(
        Collection::decode(&selected.encode(&catalog()).unwrap(), &catalog()).unwrap(),
        selected
    );
    assert_eq!(graph.for_roots(vec![11]), Err(Error::MissingItem));
    assert_eq!(
        graph.for_roots(vec![7; MAX_COLLECTION + 1]),
        Err(Error::Bound)
    );
    graph.items.get_mut(&8).unwrap().children = vec![7];
    let cycle = graph.for_roots(vec![7]).unwrap();
    assert_eq!(cycle.encode(&catalog()), Err(Error::Cycle));
    graph.items.get_mut(&8).unwrap().children = vec![7; MAX_COLLECTION + 1];
    assert_eq!(graph.for_roots(vec![7]), Err(Error::Bound));
    // A small pending stack must not allow an unbounded total traversal.
    let mut wide = Collection {
        roots: (1..=9).collect(),
        ..Default::default()
    };
    for id in 1..=9 {
        let mut next = item(id);
        next.children = vec![10; MAX_COLLECTION];
        wide.items.insert(id, next);
    }
    wide.items.insert(10, item(10));
    assert_eq!(wide.for_roots(wide.roots.clone()), Err(Error::Bound));
}

#[test]
fn e120_common_layout_and_repeated_reference_have_exact_independent_bytes() {
    // Synthetic values in the common field layout.
    // confirms Owner is eight bytes and cache insertion is after suffix.
    let mut graph = one();
    graph.roots.push(7);
    let bytes = graph.encode(&catalog()).unwrap();
    let mut expected = vec![2, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0];
    expected.extend([0x11; 16]);
    expected.extend([7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    expected.extend([0, 0, 0, 0, 0, 0, 0, 0, 2, 123, 0, 0, 0, 45, 0, 0, 0]);
    expected.extend([7, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(bytes, expected);
    assert_eq!(Collection::decode(&expected, &catalog()).unwrap(), graph);
    let envelope = Envelope::from_collection(2, &graph, &catalog()).unwrap();
    let wire = envelope.encode().unwrap();
    assert_eq!(&wire[..9], &[2, 0, 0, 0, 2, 69, 0, 0, 0]);
    assert_eq!(Envelope::decode(&wire).unwrap(), envelope);
    assert_eq!(
        Request::decode(&[2, 0, 0, 0, 0x10]).unwrap().encode(),
        [2, 0, 0, 0, 0x10]
    );
}

#[test]
fn nested_graph_caches_completed_children_and_keeps_owner_bits() {
    let mut graph = one();
    let high = 0xffff_ffff_0000_0001;
    graph.items.get_mut(&7).unwrap().defaults = vec![high];
    graph.items.get_mut(&7).unwrap().children = vec![high];
    let mut child = item(high);
    child.owner = 7;
    child.state = 0xfe;
    child.buy_price = u32::MAX;
    child.sell_price = 0x8000_0000;
    graph.items.insert(high, child);
    graph.roots.push(high);
    let raw = graph.encode(&catalog()).unwrap();
    assert_eq!(raw.len(), 4 + 2 * 57 + 2 * 8);
    assert_eq!(Collection::decode(&raw, &catalog()).unwrap(), graph);
    // Decode state and reference caches cannot leak across calls/connections.
    assert_eq!(Collection::decode(&raw, &catalog()).unwrap(), graph);
    assert!(Collection::decode(&[1, 0, 0, 0, 1, 0, 0, 0, 255, 255, 255, 255], &catalog()).is_err());
}

#[test]
fn all_definition_bindings_and_derived_widths_match_the_e343_inventory() {
    // Independent field-width inventory. Empty:39 classes;18callbacks
    // serve20classes (Tire andBrake have two definition classes apiece).
    let mut widths = BTreeMap::<usize, usize>::new();
    let mut empty = 0;
    for class in DefinitionClass::ALL {
        assert_eq!(DefinitionClass::from_name(class.name()), Some(class));
        let expected = match class {
            DefinitionClass::RaceVehicleItemData => 68,
            DefinitionClass::SpoilerItemData
            | DefinitionClass::CategoryUnlockControllerItemData
            | DefinitionClass::NosTuningItemData
            | DefinitionClass::TireCompositionItemData
            | DefinitionClass::TiresItemData
            | DefinitionClass::DiscountItemData
            | DefinitionClass::DifferentialTuningItemData
            | DefinitionClass::GearboxTuningItemData
            | DefinitionClass::HandbrakeTuningItemData => 4,
            DefinitionClass::LiveryCustomizationItemData => 12,
            DefinitionClass::PersistantTuningItemData => 20,
            DefinitionClass::SteeringTuningItemData
            | DefinitionClass::SwaybarTuningItemData
            | DefinitionClass::BrakeDiscsItemData
            | DefinitionClass::BrakesItemData => 8,
            DefinitionClass::SuspensionTuningItemData => 16,
            DefinitionClass::RimsItemData => 52,
            DefinitionClass::TimedDiscountItemData => 60,
            DefinitionClass::ControlArmTuningItemData => 36,
            _ => 0,
        };
        let raw = vec![0; expected];
        let derived = Derived::decode(class.layout(), &raw).unwrap();
        assert_eq!(derived.encode().unwrap(), raw);
        if expected == 0 {
            empty += 1;
        } else {
            assert!(Derived::decode(class.layout(), &raw[..expected - 1]).is_err());
        }
        let mut trailing = raw.clone();
        trailing.push(0);
        assert_eq!(
            Derived::decode(class.layout(), &trailing),
            Err(Error::Trailing)
        );
        *widths.entry(expected).or_default() += 1;
    }
    assert_eq!(empty, 39);
    assert_eq!(widths.values().sum::<usize>(), 59);
    assert_eq!(DefinitionClass::from_name("UnknownItemData"), None);
}

#[test]
fn typed_variable_fields_keep_words_and_refuse_excessive_counts() {
    let value = Derived::TimedDiscount {
        discount_percent: 0xffff_ffff,
        start_time: [0xaa; 24],
        end_time: [0xbb; 24],
        applicable_item_ids: vec![0, u32::MAX],
        applicable_item_tag_ids: vec![17],
    };
    let bytes = value.encode().unwrap();
    assert_eq!(bytes.len(), 72);
    assert_eq!(
        &bytes[52..64],
        &[2, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255]
    );
    assert_eq!(
        Derived::decode(Layout::TimedDiscount, &bytes).unwrap(),
        value
    );
    let mut too_many = bytes.clone();
    too_many[52..56].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        Derived::decode(Layout::TimedDiscount, &too_many),
        Err(Error::Bound)
    );
    let excessive = Derived::TimedDiscount {
        applicable_item_ids: vec![0; MAX_WORDS + 1],
        discount_percent: 0,
        start_time: [0; 24],
        end_time: [0; 24],
        applicable_item_tag_ids: vec![],
    };
    assert_eq!(excessive.encode(), Err(Error::Bound));
}

#[test]
fn rejects_cycles_unknown_types_wrong_ids_and_unreachable_items() {
    let mut graph = one();
    graph.items.get_mut(&7).unwrap().children.push(7);
    assert_eq!(graph.encode(&catalog()), Err(Error::Cycle));
    let mut raw = one().encode(&catalog()).unwrap();
    // Replace the zero child count with one, and its following record with an
    // active reference. A cache insertion at the wrong point would accept it.
    raw[48..52].copy_from_slice(&1u32.to_le_bytes());
    raw.splice(52..52, 7u64.to_le_bytes());
    assert_eq!(Collection::decode(&raw, &catalog()), Err(Error::Cycle));
    let mut raw = one().encode(&catalog()).unwrap();
    raw[28] = 8;
    assert_eq!(Collection::decode(&raw, &catalog()), Err(Error::Shape));
    assert_eq!(
        one().encode(&Catalog::default()),
        Err(Error::UnknownDefinition)
    );
    let mut graph = one();
    graph.items.get_mut(&7).unwrap().id = 8;
    assert_eq!(graph.encode(&catalog()), Err(Error::Shape));
    let mut graph = one();
    graph.roots.push(9);
    assert_eq!(graph.encode(&catalog()), Err(Error::MissingItem));
    let mut graph = one();
    graph.items.insert(8, item(8));
    assert_eq!(graph.encode(&catalog()), Err(Error::Shape));
    let mut graph = one();
    graph.items.get_mut(&7).unwrap().definition = [0x22; 16];
    assert_eq!(graph.encode(&catalog()), Err(Error::Shape));
}

#[test]
fn partial_inputs_bad_lengths_and_unknown_envelopes_are_bounded() {
    let wire = Envelope::from_collection(2, &one(), &catalog())
        .unwrap()
        .encode()
        .unwrap();
    for end in 0..wire.len() {
        assert!(Envelope::decode(&wire[..end]).is_err());
    }
    let body = one().encode(&catalog()).unwrap();
    for end in 0..body.len() {
        assert!(Collection::decode(&body[..end], &catalog()).is_err());
    }
    let mut trailing = wire.clone();
    trailing.push(0);
    assert_eq!(Envelope::decode(&trailing), Err(Error::Trailing));
    let mut invalid = wire;
    invalid[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(Envelope::decode(&invalid), Err(Error::Truncated));
    let unknown = Envelope {
        sequence: 11,
        state: 0xff,
        body: vec![1, 2, 3],
    };
    assert_eq!(
        Envelope::decode(&unknown.encode().unwrap()).unwrap(),
        unknown
    );
    assert_eq!(unknown.collection(&catalog()), Err(Error::UnsupportedState));
    assert!(Request::decode(&[0; 4]).is_err());
    assert_eq!(Request::decode(&[0; 6]), Err(Error::Trailing));
    assert_eq!(Envelope::decode(&vec![0; MAX_BYTES + 1]), Err(Error::Bound));
    let too_large = Envelope {
        sequence: 0,
        state: 2,
        body: vec![0; MAX_BYTES],
    };
    assert_eq!(too_large.encode(), Err(Error::Bound));
}

#[test]
fn graph_depth_collection_and_total_reference_bounds() {
    let mut graph = Collection::default();
    graph.roots.push(1);
    for id in 1..=MAX_DEPTH as u64 + 1 {
        let mut node = item(id);
        if id <= MAX_DEPTH as u64 {
            node.children.push(id + 1);
        }
        graph.items.insert(id, node);
    }
    assert_eq!(graph.encode(&catalog()), Err(Error::Bound));
    let mut graph = one();
    graph.roots = vec![7; MAX_COLLECTION + 1];
    assert_eq!(graph.encode(&catalog()), Err(Error::Bound));
    let mut graph = one();
    for id in 8..=16 {
        let mut node = item(id);
        node.children = vec![7; MAX_COLLECTION];
        graph.items.insert(id, node);
        graph.roots.push(id);
    }
    assert_eq!(graph.encode(&catalog()), Err(Error::Bound));
    assert_eq!(
        Collection::decode(&u32::MAX.to_le_bytes(), &catalog()),
        Err(Error::Bound)
    );
}

#[test]
fn duplicate_and_excessive_definition_maps_are_refused() {
    assert_eq!(
        Catalog::new([([1; 16], DefinitionClass::CurrencyItemData); 2]),
        Err(Error::Shape)
    );
    let entries = (0..=MAX_DEFINITIONS)
        .map(|n| ((n as u128).to_le_bytes(), DefinitionClass::CurrencyItemData));
    assert_eq!(Catalog::new(entries), Err(Error::Bound));
}

#[test]
fn deterministic_malformed_corpus_cannot_panic_or_escape_bounds() {
    let mut seed = 0x3141_5926u32;
    for len in 0..1024 {
        let raw: Vec<_> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        let _ = Collection::decode(&raw, &catalog());
        let _ = Envelope::decode(&raw);
        let _ = Derived::decode(Layout::TimedDiscount, &raw);
    }
}
