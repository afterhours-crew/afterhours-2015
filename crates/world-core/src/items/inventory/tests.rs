// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::items::PURCHASABLE;

fn definition(sub_items: Vec<Guid>) -> Definition {
    Definition {
        class: DefinitionClass::CurrencyItemData,
        buy_price: 123,
        sell_price: 45,
        quantity: u32::MAX,
        ownership_level: OwnershipLevel::Claimable,
        flags: DefinitionFlags::default(),
        sub_items,
        additional_items: Vec::new(),
        default_derived: Some(Derived::Empty),
    }
}

#[test]
fn native_initial_recipe_keeps_parent_owners_shared_lists_and_distinct_instances() {
    let mut root = definition(vec![[2; 16], [2; 16]]);
    root.additional_items = vec![[3; 16]];
    let catalog = InventoryCatalog::new(
        [
            ([1; 16], root),
            ([2; 16], definition(vec![])),
            ([3; 16], definition(vec![])),
        ],
        vec![[1; 16], [1; 16]],
    )
    .unwrap();
    let first = (1u64 << 63) + 17;
    let generated = catalog.instantiate_initial(first).unwrap();
    assert_eq!(generated.next_id, first + 6);
    assert_eq!(
        generated.collection.roots,
        (first..first + 6).collect::<Vec<_>>()
    );
    for root_id in [first, first + 3] {
        let root = &generated.collection.items[&root_id];
        assert_eq!(root.owner, 0);
        assert_eq!(root.defaults, vec![root_id + 1, root_id + 2]);
        assert_eq!(root.children, root.defaults);
        for id in &root.children {
            let child = &generated.collection.items[id];
            assert_eq!(child.owner, root_id);
            assert_eq!(child.definition, [2; 16]);
            assert_eq!(
                (child.state, child.buy_price, child.sell_price),
                (OWNED, 123, 45)
            );
        }
    }
    // AdditionalItems is retained metadata, not silently added to this recipe.
    assert!(
        generated
            .collection
            .items
            .values()
            .all(|item| item.definition != [3; 16])
    );
    let bytes = generated.collection.encode(catalog.bindings()).unwrap();
    assert_eq!(
        Collection::decode(&bytes, catalog.bindings()).unwrap(),
        generated.collection
    );
    let mut other = catalog.instantiate_initial(first).unwrap();
    other.collection.items.get_mut(&first).unwrap().state = PURCHASABLE;
    assert_eq!(generated.collection.items[&first].state, OWNED);
    assert_eq!(catalog.instantiate_initial(first).unwrap(), generated);
}

#[test]
fn owned_subtree_indexes_all_records_and_has_bounded_parent_identity() {
    let catalog = InventoryCatalog::new(
        [
            ([1; 16], definition(vec![[2; 16], [3; 16]])),
            ([2; 16], definition(vec![])),
            ([3; 16], definition(vec![])),
        ],
        vec![],
    )
    .unwrap();
    let built = catalog.instantiate_owned([1; 16], 10, 100).unwrap();
    assert_eq!(built.next_id, 103);
    assert_eq!(built.collection.roots, [100, 101, 102]);
    assert_eq!(built.collection.items[&100].owner, 10);
    assert_eq!(built.collection.items[&100].defaults, [101, 102]);
    assert_eq!(built.collection.items[&100].children, [101, 102]);
    for id in [101, 102] {
        assert_eq!(built.collection.items[&id].owner, 100);
    }
    let bytes = built.collection.encode(catalog.bindings()).unwrap();
    assert_eq!(
        Collection::decode(&bytes, catalog.bindings()).unwrap(),
        built.collection
    );
    for (owner, first) in [(0, 100), (10, 0), (100, 100), (101, 100)] {
        assert_eq!(
            catalog.instantiate_owned([1; 16], owner, first),
            Err(Error::Shape)
        );
    }
    assert_eq!(
        catalog.instantiate_owned([1; 16], 10, u64::MAX - 1),
        Err(Error::Bound)
    );
}

#[test]
fn rejects_missing_duplicate_cyclic_and_wrong_layout_definitions() {
    let d = definition(vec![]);
    assert_eq!(
        InventoryCatalog::new([([1; 16], d.clone()), ([1; 16], d.clone())], vec![]),
        Err(Error::Shape)
    );
    assert_eq!(
        InventoryCatalog::new([([1; 16], definition(vec![[2; 16]]))], vec![]),
        Err(Error::UnknownDefinition)
    );
    assert_eq!(
        InventoryCatalog::new([([1; 16], d.clone())], vec![[2; 16]]),
        Err(Error::UnknownDefinition)
    );
    assert_eq!(
        InventoryCatalog::new(
            [
                ([1; 16], definition(vec![[2; 16]])),
                ([2; 16], definition(vec![[1; 16]]))
            ],
            vec![]
        ),
        Err(Error::Cycle)
    );
    let mut wrong = d;
    wrong.default_derived = Some(Derived::Discount {
        discount_percent: 1,
    });
    assert_eq!(
        InventoryCatalog::new([([1; 16], wrong)], vec![]),
        Err(Error::Shape)
    );
}

#[test]
fn unresolved_defaults_and_exhausted_ids_are_atomic_errors() {
    let mut unknown = definition(vec![]);
    unknown.default_derived = None;
    let catalog = InventoryCatalog::new(
        [([1; 16], definition(vec![[2; 16]])), ([2; 16], unknown)],
        vec![[1; 16]],
    )
    .unwrap();
    assert_eq!(
        catalog.instantiate_initial(8),
        Err(Error::UnsupportedDefault)
    );
    assert_eq!(
        catalog.instantiate_initial(8),
        Err(Error::UnsupportedDefault)
    );
    let valid = InventoryCatalog::new([([1; 16], definition(vec![]))], vec![[1; 16]]).unwrap();
    assert_eq!(valid.instantiate_initial(0), Err(Error::Shape));
    assert_eq!(valid.instantiate_initial(u64::MAX), Err(Error::Bound));
    assert_eq!(
        valid.instantiate_initial(u64::MAX - 1).unwrap().next_id,
        u64::MAX
    );
}

#[test]
fn bounds_static_depth_edges_and_exponential_instance_expansion() {
    let chain = (1..=MAX_DEPTH + 1)
        .map(|n| {
            (
                [n as u8; 16],
                definition(if n == MAX_DEPTH + 1 {
                    vec![]
                } else {
                    vec![[n as u8 + 1; 16]]
                }),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(InventoryCatalog::new(chain, vec![]), Err(Error::Bound));
    let mut too_wide = definition(vec![]);
    too_wide.additional_items = vec![[1; 16]; MAX_COLLECTION + 1];
    assert_eq!(
        InventoryCatalog::new([([1; 16], too_wide)], vec![]),
        Err(Error::Bound)
    );
    // A binary tree must exceed the current item budget, independently of the
    // static definition/depth limits.
    let levels = u8::try_from(MAX_ITEMS.ilog2() + 1).unwrap();
    assert!((1_usize << levels) - 1 > MAX_ITEMS);
    let graph = (1..=levels)
        .map(|n| {
            (
                [n; 16],
                definition(if n == levels {
                    vec![]
                } else {
                    vec![[n + 1; 16]; 2]
                }),
            )
        })
        .collect::<Vec<_>>();
    let catalog = InventoryCatalog::new(graph, vec![[1; 16]]).unwrap();
    assert_eq!(catalog.instantiate_initial(1), Err(Error::Bound));
    let many = (1..=9)
        .map(|n| ([n; 16], definition(vec![[10; 16]; MAX_COLLECTION])))
        .chain([([10; 16], definition(vec![]))]);
    assert_eq!(InventoryCatalog::new(many, vec![]), Err(Error::Bound));
}

#[test]
fn ordered_tree_serialization_has_an_independent_small_wire_fixture() {
    let catalog = InventoryCatalog::new([([1; 16], definition(vec![]))], vec![[1; 16]]).unwrap();
    let collection = catalog.instantiate_initial(7).unwrap().collection;
    // Count1, id7, GUID, repeated id7, owner0, two empty collections,
    // native owned state2, buy123 and sell45, with no derived fields.
    let expected = [
        vec![1, 0, 0, 0],
        vec![7, 0, 0, 0, 0, 0, 0, 0],
        vec![1; 16],
        vec![7, 0, 0, 0, 0, 0, 0, 0],
        vec![0; 8],
        vec![0; 8],
        vec![2],
        vec![123, 0, 0, 0],
        vec![45, 0, 0, 0],
    ]
    .concat();
    assert_eq!(collection.encode(catalog.bindings()).unwrap(), expected);
}
