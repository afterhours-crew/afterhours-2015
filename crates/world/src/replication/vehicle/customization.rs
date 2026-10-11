// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Customization, Error, parts, tuning};
use crate::items::{Collection, DefinitionClass, Derived, Guid, MAX_DEFINITIONS};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Family {
    Performance,
    Appearance,
    Other,
}

pub fn family(class: DefinitionClass) -> Family {
    use DefinitionClass::*;
    match class {
        AirFilterItemData
        | BrakesItemData
        | CamShaftItemData
        | ClutchItemData
        | ControlArmTuningItemData
        | CylinderHeadsItemData
        | DifferentialTuningItemData
        | EcuItemData
        | ElectricSystemItemData
        | EngineBlockItemData
        | EngineDisplacementItemData
        | ExhaustManifoldItemData
        | ExhaustSystemItemData
        | ForcedInductionItemData
        | FuelSystemItemData
        | GearboxTuningItemData
        | GearsItemData
        | HandbrakeTuningItemData
        | IgnitionItemData
        | IntakeManifoldItemData
        | NosTuningItemData
        | PersistantTuningItemData
        | RadiatorItemData
        | SteeringTuningItemData
        | SuspensionTuningItemData
        | SwaybarTuningItemData
        | TireCompositionItemData => Family::Performance,
        BrakeDiscsItemData | BumperItemData | CalipersItemData | CanardItemData
        | DiffuserItemData | ExhaustItemData | FendersItemData | HoodItemData | LightsItemData
        | RimsItemData | RollCageItemData | RoofItemData | SideSkirtsItemData
        | SplitterItemData | SpoilerItemData | SuspensionItemData | TiresItemData
        | TrunkLidItemData | WingMirrorsItemData => Family::Appearance,
        BodyKitItemData
        | CategoryUnlockControllerItemData
        | CurrencyItemData
        | DiscountItemData
        | LicensePlateBackgroundItemData
        | LicensePlateFrameItemData
        | LiveryCustomizationItemData
        | LiveryDecalSwatchPackItemData
        | RaceVehicleItemData
        | SoundSystemItemData
        | StaticLiveryCustomizationItemData
        | StyleItemData
        | TimedDiscountItemData => Family::Other,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rim {
    pub rear: bool,
    pub values: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Performance {
        value: f32,
        group_a: Option<u8>,
        clutch_group: Option<u8>,
        induction: Option<u8>,
    },
    Appearance {
        spoiler_group: Option<u8>,
        group_b: Option<u8>,
        rim: Option<Rim>,
        flag: bool,
    },
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Definitions {
    tuning: tuning::Definitions,
    inputs: BTreeMap<Guid, Input>,
}
impl Definitions {
    pub fn new(
        tuning: tuning::Definitions,
        entries: impl IntoIterator<Item = (Guid, Input)>,
    ) -> Result<Self, Error> {
        let mut inputs = BTreeMap::new();
        for (guid, input) in entries {
            if inputs.len() >= MAX_DEFINITIONS {
                return Err(Error::Bound);
            }
            let class = tuning
                .classes
                .class(&guid)
                .map_err(|_| Error::UnknownObject)?;
            if tuning::applies(class) {
                let asset = tuning
                    .parts
                    .get(&guid)
                    .ok_or(Error::UnknownObject)?
                    .asset_present;
                let group_present = match &input {
                    Input::Performance { group_a, .. } => group_a.is_some(),
                    Input::Appearance { spoiler_group, .. } => spoiler_group.is_some(),
                    Input::Other => false,
                };
                if asset != group_present {
                    return Err(Error::Shape);
                }
            }
            use DefinitionClass::*;
            match (&input, family(class)) {
                (
                    Input::Performance {
                        value,
                        group_a,
                        clutch_group,
                        induction,
                    },
                    Family::Performance,
                ) => {
                    if !value.is_finite() || *value < 0.0 || induction.is_some_and(|v| v > 63) {
                        return Err(Error::Bound);
                    }
                    if group_a.is_some() && !tuning::applies(class)
                        || clutch_group.is_some() && class != ClutchItemData
                        || induction.is_some() != (class == ForcedInductionItemData)
                    {
                        return Err(Error::Shape);
                    }
                }
                (
                    Input::Appearance {
                        spoiler_group, rim, ..
                    },
                    Family::Appearance,
                ) => {
                    if spoiler_group.is_some() && class != SpoilerItemData
                        || rim.is_some() != (class == RimsItemData)
                    {
                        return Err(Error::Shape);
                    }
                    if rim.as_ref().is_some_and(|rim| {
                        rim.values.len() > 256 || rim.values.iter().any(|v| !v.is_finite())
                    }) {
                        return Err(Error::Bound);
                    }
                }
                (Input::Other, Family::Other) => (),
                _ => return Err(Error::TypeMismatch),
            }
            if inputs.insert(guid, input).is_some() {
                return Err(Error::DuplicateObject);
            }
        }
        Ok(Self { tuning, inputs })
    }
}

impl Customization {
    pub fn from_vehicle(
        collection: &Collection,
        definitions: &Definitions,
        vehicle: u64,
    ) -> Result<Self, Error> {
        let mut performance = 0.0_f32;
        let mut group_a = BTreeSet::new();
        let mut group_b = BTreeSet::new();
        let mut index = None;
        let mut rims = [0.0; 2];
        let mut flag = false;
        for (item, _) in parts::installed(collection, &definitions.tuning.classes, vehicle)? {
            match definitions
                .inputs
                .get(&item.definition)
                .ok_or(Error::UnknownObject)?
            {
                Input::Performance {
                    value,
                    group_a: group,
                    clutch_group,
                    induction,
                } => {
                    if value.abs() >= 1e-6_f32 {
                        performance += value;
                    }
                    group_a.extend(group);
                    group_b.extend(clutch_group);
                    if induction.is_some() {
                        index = *induction;
                    }
                }
                Input::Appearance {
                    spoiler_group,
                    group_b: group,
                    rim,
                    flag: part_flag,
                } => {
                    group_a.extend(spoiler_group);
                    group_b.extend(group);
                    flag |= part_flag;
                    if let Some(rim) = rim
                        && !rim.values.is_empty()
                    {
                        let Derived::Rims { rim_selection, .. } = item.derived else {
                            return Err(Error::TypeMismatch);
                        };
                        rims[usize::from(rim.rear)] = *rim
                            .values
                            .get(usize::try_from(rim_selection).map_err(|_| Error::Bound)?)
                            .ok_or(Error::Bound)?;
                    }
                }
                Input::Other => (),
            }
        }
        if group_a.len() > 255 || group_b.len() > 255 {
            return Err(Error::Bound);
        }
        let scaled = performance * 255.0;
        if !scaled.is_finite() || scaled >= 4_294_967_296.0_f32 {
            return Err(Error::Bound);
        }
        let tuning =
            tuning::Settings::from_vehicle(collection, &definitions.tuning, vehicle)?.values();
        Ok(Self {
            value: (scaled as u32).min(255) as u8,
            list_a: group_a.into_iter().collect(),
            list_b: group_b.into_iter().collect(),
            index,
            values: rims.map(quantize_rim),
            flag,
            tuning,
        })
    }
}

fn quantize_rim(value: f32) -> u16 {
    let normalized = value.clamp(0.0, 0.8) / 0.8;
    (normalized * 65_535.0 + 0.5) as u16
}

#[cfg(test)]
mod tests;
