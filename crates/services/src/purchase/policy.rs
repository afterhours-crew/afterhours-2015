// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

pub fn spatial_class(class: DefinitionClass) -> bool {
    use DefinitionClass::*;
    matches!(
        class,
        BrakeDiscsItemData
            | BumperItemData
            | CalipersItemData
            | CanardItemData
            | FendersItemData
            | LightsItemData
            | RimsItemData
            | TiresItemData
    )
}
pub fn visual_class(class: DefinitionClass) -> bool {
    use DefinitionClass::*;
    spatial_class(class)
        || matches!(
            class,
            DiffuserItemData
                | ExhaustItemData
                | HoodItemData
                | LicensePlateBackgroundItemData
                | LicensePlateFrameItemData
                | RollCageItemData
                | RoofItemData
                | SideSkirtsItemData
                | SoundSystemItemData
                | SplitterItemData
                | SpoilerItemData
                | SuspensionItemData
                | TrunkLidItemData
                | WingMirrorsItemData
        )
}
pub fn part_class(class: DefinitionClass) -> bool {
    performance_class(class) || visual_class(class)
}

/// Locations cover stock definitions as well as offers. Minimum REP scores are
/// resolved from the same installed thresholds used for reputation presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartPolicy {
    pub locations: BTreeMap<Guid, u8>,
    pub minimum_rep_scores: BTreeMap<Guid, i32>,
}
impl Offers {
    pub fn item_unlocks(&self) -> Option<&crate::item_unlocks::Policy> {
        self.item_unlocks.as_ref()
    }
    pub fn with_item_unlocks(mut self, policy: crate::item_unlocks::Policy) -> Result<Self, Error> {
        let parts = self
            .part_policy
            .as_ref()
            .ok_or(Error::Rejected(Reason::Offer))?;
        for (guid, score) in policy.requirements() {
            if !self.definitions.contains(guid) || parts.minimum_rep_scores.get(guid) != Some(score)
            {
                return Err(Error::Rejected(Reason::Offer));
            }
        }
        self.item_unlocks = Some(policy);
        Ok(self)
    }
    pub(super) fn requires_item_unlock(&self, guid: &Guid) -> bool {
        self.item_unlocks
            .as_ref()
            .is_some_and(|p| p.requirements().contains_key(guid))
    }
    pub fn part_policy(&self) -> Option<&PartPolicy> {
        self.part_policy.as_ref()
    }
    pub fn with_part_policy(
        mut self,
        catalog: &InventoryCatalog,
        locations: impl IntoIterator<Item = (Guid, u8)>,
        requirements: impl IntoIterator<Item = (Guid, i32)>,
    ) -> Result<Self, Error> {
        let invalid = || Error::Rejected(Reason::Offer);
        if self.vehicle_scopes.is_none() {
            return Err(invalid());
        }
        let mut policy = PartPolicy {
            locations: BTreeMap::new(),
            minimum_rep_scores: BTreeMap::new(),
        };
        for (guid, location) in locations {
            if policy.locations.len() >= items::MAX_DEFINITIONS
                || location > 1
                || !catalog.bindings().class(&guid).is_ok_and(spatial_class)
                || policy.locations.insert(guid, location).is_some()
            {
                return Err(invalid());
            }
        }
        if catalog
            .definitions()
            .any(|(guid, d)| spatial_class(d.class) && !policy.locations.contains_key(guid))
        {
            return Err(invalid());
        }
        for (guid, score) in requirements {
            if score < 0
                || !self.definitions.contains(&guid)
                || policy.minimum_rep_scores.len() >= MAX_OFFERS
                || policy.minimum_rep_scores.insert(guid, score).is_some()
            {
                return Err(invalid());
            }
        }
        if policy.minimum_rep_scores.len() != self.definitions.len() {
            return Err(invalid());
        }
        self.part_policy = Some(policy);
        Ok(self)
    }
    pub fn supports(&self, class: DefinitionClass) -> bool {
        performance_class(class) || (self.part_policy.is_some() && visual_class(class))
    }
    /// Installed spatial metadata, including owned stock and spare definitions.
    pub fn location(&self, guid: &Guid) -> Option<u8> {
        self.part_policy.as_ref()?.locations.get(guid).copied()
    }
    /// Exact class is insufficient for front/rear visual parts. Missing spatial
    /// metadata is an error, including when restoring an owned stock default.
    pub fn same_slot(&self, catalog: &InventoryCatalog, a: &Guid, b: &Guid) -> Result<bool, Error> {
        let class = catalog
            .bindings()
            .class(a)
            .map_err(inventory::Error::from)?;
        if catalog
            .bindings()
            .class(b)
            .map_err(inventory::Error::from)?
            != class
        {
            return Ok(false);
        }
        if !spatial_class(class) {
            return Ok(true);
        }
        let locations = &self
            .part_policy
            .as_ref()
            .ok_or(Error::Rejected(Reason::Offer))?
            .locations;
        Ok(locations.get(a).ok_or(Error::Rejected(Reason::Offer))?
            == locations.get(b).ok_or(Error::Rejected(Reason::Offer))?)
    }
    pub(super) fn unlocked(&self, snapshot: &Snapshot, guid: &Guid) -> Result<bool, Error> {
        if self.requires_item_unlock(guid) {
            return Ok(crate::item_unlocks::Policy::granted(snapshot, guid));
        }
        let required = self
            .part_policy
            .as_ref()
            .map(|p| {
                p.minimum_rep_scores
                    .get(guid)
                    .copied()
                    .ok_or(Error::Rejected(Reason::Offer))
            })
            .transpose()?
            .unwrap_or(0);
        if required == 0 {
            return Ok(true);
        }
        let Some(Value::Int(score)) = snapshot
            .tables
            .get(&crate::persistent::key("RepValuesTable"))
            .and_then(|t| t.rows.get(&0))
            .and_then(|r| r.get(&crate::persistent::key("RepScore")))
        else {
            return Ok(false);
        };
        Ok(*score >= required)
    }
}
