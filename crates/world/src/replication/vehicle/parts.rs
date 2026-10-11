// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Error;
use crate::items::{
    Catalog, Collection, DefinitionClass, Derived, Item, MAX_COLLECTION, MAX_DEPTH, MAX_ITEMS,
    OWNED,
};
use std::collections::BTreeSet;

pub(super) fn installed<'a>(
    collection: &'a Collection,
    classes: &Catalog,
    vehicle: u64,
) -> Result<Vec<(&'a Item, DefinitionClass)>, Error> {
    if collection.items.len() > MAX_ITEMS {
        return Err(Error::Bound);
    }
    let car = collection.items.get(&vehicle).ok_or(Error::UnknownObject)?;
    if car.id != vehicle || vehicle == 0 || car.owner != 0 || car.state != OWNED {
        return Err(Error::Shape);
    }
    if !matches!(car.derived, Derived::RaceVehicle { .. }) {
        return Err(Error::TypeMismatch);
    }
    let mut result = Vec::new();
    let mut visited = BTreeSet::new();
    let mut pending = vec![(vehicle, 0, 0)];
    while let Some((id, parent, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            return Err(Error::Bound);
        }
        if !visited.insert(id) {
            return Err(Error::DuplicateObject);
        }
        let item = collection.items.get(&id).ok_or(Error::UnknownObject)?;
        if id == 0 || item.id != id || item.owner != parent || item.state != OWNED {
            return Err(Error::Shape);
        }
        let class = classes
            .class(&item.definition)
            .map_err(|_| Error::UnknownObject)?;
        if class.layout() != item.derived.layout() {
            return Err(Error::TypeMismatch);
        }
        if item.children.len() > MAX_COLLECTION || pending.len() + item.children.len() > MAX_ITEMS {
            return Err(Error::Bound);
        }
        pending.extend(
            item.children
                .iter()
                .rev()
                .map(|&child| (child, id, depth + 1)),
        );
        result.push((item, class));
    }
    Ok(result)
}
