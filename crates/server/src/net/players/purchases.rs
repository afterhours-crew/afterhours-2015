// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_world::items::{Collection, OWNED, PURCHASABLE};

/// Accept one committed leaf purchase, including an unchanged retry. A selected
/// purchasable instance keeps its ID: only its state and owner may change.
pub(super) fn valid_refresh(before: &Collection, after: &Collection) -> bool {
    if before.items.keys().any(|id| !after.items.contains_key(id)) {
        return false;
    }
    let mut changed = 0;
    for (id, item) in &after.items {
        let previous = before.items.get(id);
        if previous == Some(item) {
            continue;
        }
        changed += 1;
        if changed > 1
            || item.id != *id
            || item.state != OWNED
            || !item.children.is_empty()
            || !item.defaults.is_empty()
            || item.owner == 0
            || !before
                .items
                .get(&item.owner)
                .is_some_and(|owner| owner.state == OWNED)
        {
            return false;
        }
        if let Some(previous) = previous {
            if previous.state != PURCHASABLE || previous.owner != 0 {
                return false;
            }
            let mut promoted = previous.clone();
            promoted.state = OWNED;
            promoted.owner = item.owner;
            if &promoted != item {
                return false;
            }
        }
    }
    true
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
}
