// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world_core::items::DefinitionClass;

impl Edit {
    /// Remove the installed child matching the selected item's exact class and
    /// spatial location. Keep ownership, saved part values and cash unchanged;
    /// this is unequip, not sale. A repeated removal is an unchanged preview.
    pub fn without_part(
        mut self,
        items: &Collection,
        item: u64,
        catalog: &InventoryCatalog,
        offers: &Offers,
    ) -> Result<Self, Error> {
        let selected = items.items.get(&item).ok_or(Error::Ownership)?;
        if selected.state != OWNED || !selected.belongs_under(self.vehicle()) {
            return Err(Error::Ownership);
        }
        let definition = catalog
            .definition(&selected.definition)
            .map_err(inventory::Error::from)?;
        let livery = matches!(
            definition.class,
            DefinitionClass::LiveryCustomizationItemData
                | DefinitionClass::StaticLiveryCustomizationItemData
        );
        if (!livery && selected.owner != self.vehicle())
            || (selected.owner == 0 && definition.flags.requires_owner)
        {
            return Err(Error::Ownership);
        }
        if (!livery && !offers.supports(definition.class))
            || !selected.children.is_empty()
            || !selected.defaults.is_empty()
        {
            return Err(Error::Unsupported);
        }
        let preview = self.preview(items)?;
        let mut matched = None;
        for &id in &self.after.children {
            let child = preview.items.get(&id).ok_or(Error::Shape)?;
            if !child.belongs_under(self.vehicle()) || child.state != OWNED {
                return Err(Error::Ownership);
            }
            if offers
                .same_slot(catalog, &child.definition, &selected.definition)
                .map_err(|_| Error::Unsupported)?
            {
                if matched.replace(id).is_some() {
                    return Err(Error::Unsupported);
                }
                if !child.children.is_empty() || !child.defaults.is_empty() {
                    return Err(Error::Unsupported);
                }
            }
        }
        if let Some(id) = matched {
            self.after.children.retain(|child| *child != id);
            self.selected
                .insert(id, items.items.get(&id).ok_or(Error::Shape)?.clone());
        }
        self.selected.insert(item, selected.clone());
        self.preview(items)?
            .encode(catalog.bindings())
            .map_err(inventory::Error::from)?;
        Ok(self)
    }
}
