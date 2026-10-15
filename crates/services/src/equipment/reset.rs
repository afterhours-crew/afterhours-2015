// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world_core::items::DefinitionClass;

fn resettable(class: DefinitionClass, keep_wheels: bool) -> bool {
    use DefinitionClass::*;
    crate::purchase::visual_class(class)
        && !matches!(
            class,
            LicensePlateBackgroundItemData | LicensePlateFrameItemData
        )
        && !(keep_wheels
            && matches!(
                class,
                RimsItemData | TiresItemData | CalipersItemData | BrakeDiscsItemData
            ))
}

impl Edit {
    /// Restore the car's stock visual children and tint, retaining performance,
    /// body paint, plate text and livery. Plate fittings and body kits are removed.
    /// The optional wheel group retains rims, tires, calipers and brake discs.
    /// The installed control-arm record's stance values return to its defaults.
    pub fn with_stock_visual(
        mut self,
        items: &Collection,
        catalog: &InventoryCatalog,
        keep_wheels: bool,
    ) -> Result<Self, Error> {
        use DefinitionClass::*;
        let preview = self.preview(items)?;
        let mut stance = None;
        let mut visited = BTreeSet::new();
        let mut pending = vec![(self.vehicle(), 0, 0)];
        while let Some((id, owner, depth)) = pending.pop() {
            if depth > nfs_world_core::items::MAX_DEPTH || !visited.insert(id) {
                return Err(Error::Shape);
            }
            let item = preview.items.get(&id).ok_or(Error::Shape)?;
            if item.state != OWNED || !item.belongs_under(owner) {
                return Err(Error::Ownership);
            }
            if catalog
                .bindings()
                .class(&item.definition)
                .map_err(inventory::Error::from)?
                == ControlArmTuningItemData
            {
                stance = Some(id);
                break;
            }
            pending.extend(
                item.children
                    .iter()
                    .rev()
                    .map(|&child| (child, id, depth + 1)),
            );
        }
        let mut children = Vec::new();
        for (ids, defaults) in [(&self.after.defaults, true), (&self.after.children, false)] {
            for &id in ids {
                let part = preview.items.get(&id).ok_or(Error::Shape)?;
                if !part.belongs_under(self.vehicle()) || part.state != OWNED {
                    return Err(Error::Ownership);
                }
                let class = catalog
                    .bindings()
                    .class(&part.definition)
                    .map_err(inventory::Error::from)?;
                let take = if defaults {
                    resettable(class, keep_wheels)
                } else {
                    !resettable(class, keep_wheels)
                        && !matches!(
                            class,
                            BodyKitItemData
                                | LicensePlateBackgroundItemData
                                | LicensePlateFrameItemData
                        )
                };
                if take {
                    if children.contains(&id) {
                        return Err(Error::Shape);
                    }
                    children.push(id);
                    self.selected
                        .insert(id, items.items.get(&id).ok_or(Error::Shape)?.clone());
                }
            }
        }
        self.after.children = children;
        if let Some(id) = stance {
            let original = items.items.get(&id).ok_or(Error::Shape)?;
            let definition = catalog
                .definition(&original.definition)
                .map_err(inventory::Error::from)?;
            let Some(default @ Derived::ControlArmTuning { .. }) = &definition.default_derived
            else {
                return Err(Error::Unsupported);
            };
            let (_, part) = self
                .parts
                .entry(id)
                .or_insert_with(|| (original.clone(), original.clone()));
            part.derived = default.clone();
        }
        let definition = catalog
            .definition(&self.after.definition)
            .map_err(inventory::Error::from)?;
        let Some(Derived::RaceVehicle { window_tint, .. }) = definition.default_derived.as_ref()
        else {
            return Err(Error::Unsupported);
        };
        self = self.with_window_tint(WindowTint::new(*window_tint)?)?;
        self.preview(items)?
            .encode(catalog.bindings())
            .map_err(inventory::Error::from)?;
        Ok(self)
    }
}
