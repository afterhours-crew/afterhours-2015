// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world_core::items::DefinitionClass;

impl Edit {
    fn livery<'a>(
        &self,
        items: &'a Collection,
        id: u64,
        catalog: &InventoryCatalog,
    ) -> Result<&'a Item, Error> {
        let item = items.items.get(&id).ok_or(Error::Ownership)?;
        let definition = catalog
            .definition(&item.definition)
            .map_err(inventory::Error::from)?;
        if item.state != OWNED || !item.belongs_under(self.vehicle()) {
            return Err(Error::Ownership);
        }
        if definition.class != DefinitionClass::LiveryCustomizationItemData
            || (item.owner == 0 && definition.flags.requires_owner)
            || !item.children.is_empty()
            || !item.defaults.is_empty()
            || !matches!(item.derived, Derived::LiveryCustomization { .. })
        {
            return Err(Error::Unsupported);
        }
        // Do not silently repaint a second vehicle through a shared library
        // leaf. Per-car copies need a separately established allocation model.
        if items.items.values().any(|other| {
            other.id != self.vehicle()
                && (other.children.contains(&id) || other.defaults.contains(&id))
        }) {
            return Err(Error::Unsupported);
        }
        Ok(item)
    }

    /// Stage the two native livery fields and clear only the edited flag bit.
    /// The library record remains account-owned until an explicit equip call;
    /// this method neither installs it nor creates an inventory item.
    pub fn with_livery(
        mut self,
        items: &Collection,
        catalog: &InventoryCatalog,
        item: u64,
        data_id: u32,
        preset: u32,
    ) -> Result<Self, Error> {
        let original = self.livery(items, item, catalog)?;
        let (_, next) = self
            .parts
            .entry(item)
            .or_insert_with(|| (original.clone(), original.clone()));
        let Derived::LiveryCustomization {
            byte_vault_data_id,
            library_preset_id,
            dynamic_flags,
        } = &mut next.derived
        else {
            return Err(Error::Shape);
        };
        *byte_vault_data_id = data_id;
        *library_preset_id = preset;
        *dynamic_flags &= !1;
        self.preview(items)?
            .encode(catalog.bindings())
            .map_err(inventory::Error::from)?;
        Ok(self)
    }

    pub(super) fn with_livery_selection(
        mut self,
        items: &Collection,
        item: u64,
        catalog: &InventoryCatalog,
    ) -> Result<Self, Error> {
        let selected = self.livery(items, item, catalog)?.clone();
        let preview = self.preview(items)?;
        let mut previous = Vec::new();
        for id in &self.after.children {
            let child = preview.items.get(id).ok_or(Error::Shape)?;
            if !child.belongs_under(self.vehicle()) || child.state != OWNED {
                return Err(Error::Ownership);
            }
            if matches!(
                catalog
                    .bindings()
                    .class(&child.definition)
                    .map_err(inventory::Error::from)?,
                DefinitionClass::LiveryCustomizationItemData
                    | DefinitionClass::StaticLiveryCustomizationItemData
            ) {
                previous.push(*id);
            }
        }
        if previous.len() > 1 {
            return Err(Error::Unsupported);
        }
        if previous.first() != Some(&item) {
            self.after.children.retain(|id| !previous.contains(id));
            self.after.children.push(item);
        }
        self.selected.insert(item, selected);
        self.preview(items)?
            .encode(catalog.bindings())
            .map_err(inventory::Error::from)?;
        Ok(self)
    }
}
