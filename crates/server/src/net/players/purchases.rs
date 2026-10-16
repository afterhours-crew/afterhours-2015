// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_world::items::{Collection, OWNED, PURCHASABLE};

/// Accept one committed purchase tree, including an unchanged retry. Existing
/// records stay unchanged except a single purchasable leaf promotion. New kit
/// children must belong to the new root; unrelated grants are not a purchase.
pub(super) fn valid_refresh(before: &Collection, after: &Collection) -> bool {
    if before.items.keys().any(|id| !after.items.contains_key(id)) {
        return false;
    }
    let mut added = std::collections::BTreeSet::new();
    let mut promoted_id = None;
    for (id, item) in &after.items {
        let previous = before.items.get(id);
        if previous == Some(item) {
            continue;
        }
        if item.id != *id || item.state != OWNED || item.owner == 0 {
            return false;
        }
        if let Some(previous) = previous {
            if previous.state != PURCHASABLE
                || previous.owner != 0
                || !item.children.is_empty()
                || !item.defaults.is_empty()
                || promoted_id.replace(*id).is_some()
                || !before
                    .items
                    .get(&item.owner)
                    .is_some_and(|owner| owner.state == OWNED)
            {
                return false;
            }
            let mut promoted = previous.clone();
            promoted.state = OWNED;
            promoted.owner = item.owner;
            if &promoted != item {
                return false;
            }
        } else {
            added.insert(*id);
        }
    }
    if added.is_empty() {
        return true;
    }
    if promoted_id.is_some() || added.len() > nfs_world::items::MAX_COLLECTION {
        return false;
    }
    let roots: Vec<_> = added
        .iter()
        .filter(|id| !added.contains(&after.items[id].owner))
        .copied()
        .collect();
    let [root] = roots.as_slice() else {
        return false;
    };
    let item = &after.items[root];
    if !before
        .items
        .get(&item.owner)
        .is_some_and(|owner| owner.state == OWNED)
        || item.children != item.defaults
        || item.children.len() + 1 != added.len()
    {
        return false;
    }
    added.remove(root);
    for id in &item.children {
        if !added.remove(id) {
            return false;
        }
        let child = &after.items[id];
        if child.owner != *root || !child.children.is_empty() || !child.defaults.is_empty() {
            return false;
        }
    }
    added.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world::items::{Derived, Item};

    fn fixture() -> Collection {
        let item = Item {
            id: 3,
            definition: [8; 16],
            owner: 0,
            defaults: vec![],
            children: vec![],
            state: PURCHASABLE,
            buy_price: 17,
            sell_price: 8,
            derived: Derived::Empty,
        };
        Collection {
            roots: vec![1, 3],
            items: [
                (
                    1,
                    Item {
                        id: 1,
                        state: OWNED,
                        ..item.clone()
                    },
                ),
                (3, item),
            ]
            .into(),
        }
    }

    #[test]
    fn purchase_refresh_accepts_promotion_insert_and_retry_only() {
        let before = fixture();
        let mut after = before.clone();
        let bought = after.items.get_mut(&3).unwrap();
        bought.owner = 1;
        bought.state = OWNED;
        assert!(valid_refresh(&before, &after));
        assert!(valid_refresh(&after, &after));
        let mut inserted = before.clone();
        let mut bought = after.items[&3].clone();
        bought.id = 4;
        inserted.items.insert(4, bought);
        inserted.roots.push(4);
        assert!(valid_refresh(&before, &inserted));
        for mutation in 0..10 {
            let mut bad = after.clone();
            let item = bad.items.get_mut(&3).unwrap();
            match mutation {
                0 => item.id = 4,
                1 => item.owner = 0,
                2 => item.owner = 999,
                3 => item.state = PURCHASABLE,
                4 => item.definition = [9; 16],
                5 => item.buy_price += 1,
                6 => item.sell_price += 1,
                7 => item.children.push(1),
                8 => item.defaults.push(1),
                _ => {
                    bad.items.remove(&1);
                }
            }
            assert!(!valid_refresh(&before, &bad), "mutation {mutation}");
        }
        let mut reparented = after.clone();
        reparented.items.get_mut(&3).unwrap().owner = 3;
        assert!(!valid_refresh(&after, &reparented));
        let mut two = inserted;
        two.items.insert(3, after.items[&3].clone());
        assert!(!valid_refresh(&before, &two));
    }

    #[test]
    fn purchase_refresh_accepts_one_owned_kit_and_rejects_graph_changes() {
        let before = fixture();
        let mut after = before.clone();
        let mut kit = before.items[&3].clone();
        kit.id = 4;
        kit.state = OWNED;
        kit.owner = 1;
        let mut child = kit.clone();
        child.id = 5;
        child.owner = 4;
        kit.children = vec![5];
        kit.defaults = vec![5];
        after.items.insert(4, kit);
        after.items.insert(5, child);
        after.roots.extend([4, 5]);
        assert!(valid_refresh(&before, &after));
        assert!(valid_refresh(&after, &after));
        for mutation in 0..8 {
            let mut bad = after.clone();
            match mutation {
                0 => bad.items.get_mut(&5).unwrap().owner = 1,
                1 => bad.items.get_mut(&4).unwrap().children.push(3),
                2 => bad.items.get_mut(&4).unwrap().defaults.clear(),
                3 => bad.items.get_mut(&5).unwrap().state = PURCHASABLE,
                4 => bad.items.get_mut(&5).unwrap().children.push(4),
                5 => bad.items.get_mut(&4).unwrap().owner = 5,
                6 => {
                    bad.items.get_mut(&3).unwrap().state = OWNED;
                    bad.items.get_mut(&3).unwrap().owner = 1;
                }
                _ => bad.items.get_mut(&1).unwrap().buy_price += 1,
            }
            assert!(!valid_refresh(&before, &bad), "mutation {mutation}");
        }
    }
}
