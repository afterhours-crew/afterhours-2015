// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world_core::items::DefinitionClass;

impl Edit {
    fn remember_kit(
        &mut self,
        items: &Collection,
        kit: &Item,
        offers: &Offers,
    ) -> Result<(), Error> {
        let definitions = offers
            .body_kits()
            .get(&kit.definition)
            .ok_or(Error::Unsupported)?;
        if kit.owner != self.vehicle() || kit.state != OWNED {
            return Err(Error::Ownership);
        }
        if kit.children != kit.defaults || kit.children.len() != definitions.len() {
            return Err(Error::Shape);
        }
        let mut remaining: BTreeSet<_> = definitions.iter().copied().collect();
        for &id in &kit.children {
            let child = items.items.get(&id).ok_or(Error::Shape)?;
            if child.owner != kit.id || child.state != OWNED {
                return Err(Error::Ownership);
            }
            if !child.children.is_empty()
                || !child.defaults.is_empty()
                || !remaining.remove(&child.definition)
            {
                return Err(Error::Shape);
            }
            self.selected.insert(id, child.clone());
        }
        self.selected.insert(
            kit.id,
            items.items.get(&kit.id).ok_or(Error::Shape)?.clone(),
        );
        Ok(())
    }

    /// A kit is a single car-owned item with owned leaf children. Replacing it
    /// removes overlapping root slots, then restores uncovered mandatory stock
    /// visual slots. The old kit and all individual parts remain owned spares.
    /// Later individual car-owned selections override matching kit children by
    /// appearing after the kit; they never reparent or modify its children.
    pub(super) fn with_body_kit(
        mut self,
        items: &Collection,
        part: u64,
        catalog: &InventoryCatalog,
        offers: &Offers,
    ) -> Result<Self, Error> {
        let preview = self.preview(items)?;
        let kit = preview.items.get(&part).ok_or(Error::Ownership)?;
        if !offers.allows(&self.before.definition, &kit.definition) {
            return Err(Error::Unsupported);
        }
        self.remember_kit(items, kit, offers)?;
        let definitions = &offers.body_kits()[&kit.definition];
        let same_slot = |a, b| {
            offers
                .same_slot(catalog, a, b)
                .map_err(|_| Error::Unsupported)
        };
        let mut previous_kit = None;
        let mut children = Vec::new();
        for &id in &preview.items[&self.vehicle()].children {
            let item = preview.items.get(&id).ok_or(Error::Shape)?;
            if item.state != OWNED || !item.belongs_under(self.vehicle()) {
                return Err(Error::Ownership);
            }
            if catalog
                .bindings()
                .class(&item.definition)
                .map_err(inventory::Error::from)?
                == DefinitionClass::BodyKitItemData
            {
                if previous_kit.replace(id).is_some() {
                    return Err(Error::Unsupported);
                }
                self.remember_kit(items, item, offers)?;
                continue;
            }
            let mut covered = false;
            for definition in definitions {
                covered |= same_slot(&item.definition, definition)?;
            }
            self.selected
                .insert(id, items.items.get(&id).ok_or(Error::Shape)?.clone());
            if !covered {
                children.push(id);
            }
        }
        children.push(part);
        if previous_kit.is_some() {
            for &id in &self.after.defaults {
                let item = preview.items.get(&id).ok_or(Error::Shape)?;
                let definition = catalog
                    .definition(&item.definition)
                    .map_err(inventory::Error::from)?;
                if definition.flags.optional || !crate::purchase::visual_class(definition.class) {
                    continue;
                }
                if item.owner != self.vehicle()
                    || item.state != OWNED
                    || !item.children.is_empty()
                    || !item.defaults.is_empty()
                {
                    return Err(Error::Ownership);
                }
                let mut covered = false;
                for other in definitions
                    .iter()
                    .chain(children.iter().map(|id| &preview.items[id].definition))
                {
                    covered |= same_slot(&item.definition, other)?;
                }
                if !covered {
                    children.push(id);
                    self.selected
                        .insert(id, items.items.get(&id).ok_or(Error::Shape)?.clone());
                }
            }
        }
        self.after.children = children;
        let proposed = self.preview(items)?;
        // The selected kit's authored restrictions apply to parts retained
        // outside the replaced slots. Its own children are not old conflicts.
        if !offers
            .fits(&proposed, self.vehicle(), &kit.definition, catalog)
            .map_err(|_| Error::Unsupported)?
        {
            return Err(Error::Unsupported);
        }
        proposed
            .encode(catalog.bindings())
            .map_err(inventory::Error::from)?;
        Ok(self)
    }
}
