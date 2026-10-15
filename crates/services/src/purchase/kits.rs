// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

/// Local policy limit for one supported visual kit, not a native wire maximum.
pub const MAX_KIT_PARTS: usize = 64;

impl Offers {
    /// Opt in only explicit offers whose children are independent visual leaves.
    /// Library liveries, nested kits, additional grants and unknown defaults are
    /// outside this contract. The kit's price/unlock applies to the bundle.
    pub fn with_body_kits(
        mut self,
        catalog: &InventoryCatalog,
        guids: impl IntoIterator<Item = Guid>,
    ) -> Result<Self, Error> {
        let invalid = || Error::Rejected(Reason::Offer);
        if self.part_policy.is_none() || self.fitment_restrictions.is_none() {
            return Err(invalid());
        }
        for guid in guids {
            let d = catalog.definition(&guid).map_err(inventory::Error::from)?;
            if !self.definitions.contains(&guid)
                || self.body_kits.contains_key(&guid)
                || self.body_kits.len() >= MAX_OFFERS
                || d.class != DefinitionClass::BodyKitItemData
                || !d.flags.purchasable
                || !d.flags.requires_owner
                || d.flags.deprecated
                || d.flags.uses_scope
                || d.quantity != 1
                || !d.additional_items.is_empty()
                || d.default_derived.is_none()
                || d.sub_items.is_empty()
                || d.sub_items.len() > MAX_KIT_PARTS
            {
                return Err(invalid());
            }
            for (index, child) in d.sub_items.iter().enumerate() {
                let part = catalog.definition(child).map_err(inventory::Error::from)?;
                if !visual_class(part.class)
                    || !part.flags.requires_owner
                    || part.flags.deprecated
                    || part.quantity != 1
                    || !part.sub_items.is_empty()
                    || !part.additional_items.is_empty()
                    || part.default_derived.is_none()
                {
                    return Err(invalid());
                }
                for other in &d.sub_items[..index] {
                    if self.same_slot(catalog, child, other)? {
                        return Err(invalid());
                    }
                }
            }
            self.body_kits.insert(guid, d.sub_items.clone());
        }
        Ok(self)
    }

    pub fn body_kits(&self) -> &BTreeMap<Guid, Vec<Guid>> {
        &self.body_kits
    }
}
