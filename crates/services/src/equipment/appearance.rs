// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world_core::items::DefinitionClass;

/// Body (0), front rim primary/secondary (1/2), rear primary/secondary (3/4).
/// Keep the original float bits in inventory; replication quantizes separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Paint {
    target: u8,
    color: [u32; 3],
    material: [u32; 3],
}
impl Paint {
    pub fn new(target: u8, color: [u32; 3], material: [u32; 3]) -> Result<Self, Error> {
        if target > 4
            || color.into_iter().chain(material).any(|bits| {
                let value = f32::from_bits(bits);
                !value.is_finite() || !(0.0..=1.0).contains(&value)
            })
        {
            return Err(Error::Shape);
        }
        Ok(Self {
            target,
            color,
            material,
        })
    }
}

impl Edit {
    /// Paint only the current car or its installed rim on the selected axle.
    /// Defaults and owned spares are not implicit paint targets.
    pub fn with_paint(
        mut self,
        items: &Collection,
        catalog: &InventoryCatalog,
        offers: &Offers,
        paint: Paint,
    ) -> Result<Self, Error> {
        if paint.target == 0 {
            let Derived::RaceVehicle {
                default_paint,
                material,
                ..
            } = &mut self.after.derived
            else {
                return Err(Error::Shape);
            };
            *default_paint = paint.color;
            *material = paint.material;
        } else {
            let preview = self.preview(items)?;
            let location = (paint.target - 1) / 2;
            let mut targets = Vec::new();
            for id in &self.after.children {
                let part = preview.items.get(id).ok_or(Error::Shape)?;
                if !part.belongs_under(self.vehicle()) || part.state != OWNED {
                    return Err(Error::Ownership);
                }
                if catalog
                    .bindings()
                    .class(&part.definition)
                    .map_err(inventory::Error::from)?
                    == DefinitionClass::RimsItemData
                    && offers.location(&part.definition) == Some(location)
                {
                    targets.push(*id);
                }
            }
            let [id] = targets.as_slice() else {
                return Err(Error::Unsupported);
            };
            let original = items.items.get(id).ok_or(Error::Shape)?;
            let (_, part) = self
                .parts
                .entry(*id)
                .or_insert_with(|| (original.clone(), original.clone()));
            let Derived::Rims {
                primary_paint,
                primary_material,
                secondary_paint,
                secondary_material,
                ..
            } = &mut part.derived
            else {
                return Err(Error::Shape);
            };
            let (color, material) = if paint.target % 2 == 1 {
                (primary_paint, primary_material)
            } else {
                (secondary_paint, secondary_material)
            };
            *color = paint.color;
            *material = paint.material;
        }
        self.preview(items)?
            .encode(catalog.bindings())
            .map_err(inventory::Error::from)?;
        Ok(self)
    }

    /// The wire carries an explicit owned rim ID and an eight-bit selection.
    /// The runtime additionally validates that selection against installed sizes.
    pub fn with_rim_size(
        mut self,
        items: &Collection,
        catalog: &InventoryCatalog,
        item: u64,
        size: u8,
    ) -> Result<Self, Error> {
        let original = items.items.get(&item).ok_or(Error::Ownership)?;
        if original.owner != self.vehicle()
            || original.state != OWNED
            || !self.after.children.contains(&item)
        {
            return Err(Error::Ownership);
        }
        if catalog
            .bindings()
            .class(&original.definition)
            .map_err(inventory::Error::from)?
            != DefinitionClass::RimsItemData
        {
            return Err(Error::Unsupported);
        }
        let (_, part) = self
            .parts
            .entry(item)
            .or_insert_with(|| (original.clone(), original.clone()));
        let Derived::Rims { rim_selection, .. } = &mut part.derived else {
            return Err(Error::Shape);
        };
        *rim_selection = u32::from(size);
        self.preview(items)?
            .encode(catalog.bindings())
            .map_err(inventory::Error::from)?;
        Ok(self)
    }
}
