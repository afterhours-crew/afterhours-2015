// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::Error;
use crate::items::{Catalog, Collection, DefinitionClass, Derived, Guid, MAX_DEFINITIONS};
#[cfg(test)]
use crate::items::{MAX_COLLECTION, MAX_DEPTH, OWNED};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartInput {
    pub asset_present: bool,
    pub transmission_values: Option<[u8; 2]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Definitions {
    pub(super) classes: Catalog,
    pub(super) parts: BTreeMap<Guid, PartInput>,
}
impl Definitions {
    pub fn new(
        classes: Catalog,
        entries: impl IntoIterator<Item = (Guid, PartInput)>,
    ) -> Result<Self, Error> {
        let mut parts = BTreeMap::new();
        for (guid, input) in entries {
            if parts.len() >= MAX_DEFINITIONS {
                return Err(Error::Bound);
            }
            let class = classes.class(&guid).map_err(|_| Error::UnknownObject)?;
            if !applies(class) {
                return Err(Error::TypeMismatch);
            }
            let needs_transmission =
                class == DefinitionClass::PersistantTuningItemData && input.asset_present;
            if input.transmission_values.is_some() != needs_transmission {
                return Err(Error::Shape);
            }
            if input
                .transmission_values
                .is_some_and(|values| values.iter().any(|&v| v > 15))
            {
                return Err(Error::Bound);
            }
            if parts.insert(guid, input).is_some() {
                return Err(Error::DuplicateObject);
            }
        }
        Ok(Self { classes, parts })
    }
}

pub(super) fn applies(class: DefinitionClass) -> bool {
    use DefinitionClass::*;
    matches!(
        class,
        SpoilerItemData
            | BrakesItemData
            | HandbrakeTuningItemData
            | DifferentialTuningItemData
            | PersistantTuningItemData
            | SteeringTuningItemData
            | ControlArmTuningItemData
            | SuspensionTuningItemData
            | SwaybarTuningItemData
            | TireCompositionItemData
            | NosTuningItemData
            | GearboxTuningItemData
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Setting {
    Downforce,
    BrakeStrength,
    BrakeBias,
    HandbrakeStrength,
    DifferentialType,
    Abs,
    StabilityControl,
    TractionControl,
    SteerRate,
    SteerRange,
    RideHeightHeight,
    RideHeightRake,
    TrackWidthFront,
    TrackWidthRear,
    Caster,
    CamberFront,
    CamberRear,
    ToeFront,
    ToeRear,
    SpringStiffnessFront,
    SpringStiffnessRear,
    DampingFront,
    DampingRear,
    AntiRollBarFront,
    AntiRollBarRear,
    Tire,
    AirPressureFront,
    AirPressureRear,
    Nos,
    Gearbox,
    Clutch,
    ManualTransmission,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Settings([u8; 32]);
impl Default for Settings {
    fn default() -> Self {
        Self([5; 32])
    }
}
impl Settings {
    pub fn values(&self) -> [u8; 32] {
        self.0
    }
    pub fn set(&mut self, setting: Setting, value: u32) -> Result<(), Error> {
        if value > 15 {
            return Err(Error::Bound);
        }
        self.0[setting as usize] = value as u8;
        Ok(())
    }
    pub fn from_vehicle(
        collection: &Collection,
        definitions: &Definitions,
        vehicle: u64,
    ) -> Result<Self, Error> {
        let installed = super::parts::installed(collection, &definitions.classes, vehicle)?;
        let car = installed.first().ok_or(Error::Shape)?.0;
        let Derived::RaceVehicle {
            vehicle_item_flags, ..
        } = car.derived
        else {
            return Err(Error::TypeMismatch);
        };
        let mut result = Self::default();
        let mut first_persistent = None;
        for (item, class) in installed {
            if applies(class) {
                let input = *definitions
                    .parts
                    .get(&item.definition)
                    .ok_or(Error::UnknownObject)?;
                if class == DefinitionClass::PersistantTuningItemData && first_persistent.is_none()
                {
                    first_persistent = Some(input);
                }
                if input.asset_present {
                    apply(&item.derived, &mut |setting, value| {
                        result.set(setting, value)
                    })?;
                }
            }
        }
        if let Some(input) = first_persistent {
            let values = input.transmission_values.ok_or(Error::Unsupported)?;
            let index = usize::from(vehicle_item_flags & 4 == 0);
            result.set(Setting::ManualTransmission, u32::from(values[index]))?;
        }
        Ok(result)
    }
}

fn apply(
    value: &Derived,
    set: &mut impl FnMut(Setting, u32) -> Result<(), Error>,
) -> Result<(), Error> {
    use Setting::*;
    match *value {
        Derived::Spoiler { downforce_setting } => set(Downforce, downforce_setting)?,
        Derived::NosTuning { nos_setting } => set(Nos, nos_setting)?,
        Derived::PersistantTuning {
            abs_setting,
            stability_control_setting,
            traction_control_setting,
            air_pressure_front_setting,
            air_pressure_rear_setting,
        } => {
            set(Abs, abs_setting)?;
            set(StabilityControl, stability_control_setting)?;
            set(TractionControl, traction_control_setting)?;
            set(AirPressureFront, air_pressure_front_setting)?;
            set(AirPressureRear, air_pressure_rear_setting)?;
        }
        Derived::SteeringTuning {
            steer_rate_setting,
            steer_range_setting,
        } => {
            set(SteerRate, steer_rate_setting)?;
            set(SteerRange, steer_range_setting)?;
        }
        Derived::SuspensionTuning {
            spring_stiffness_front_setting,
            spring_stiffness_rear_setting,
            damping_front_setting,
            damping_rear_setting,
        } => {
            set(SpringStiffnessFront, spring_stiffness_front_setting)?;
            set(SpringStiffnessRear, spring_stiffness_rear_setting)?;
            set(DampingFront, damping_front_setting)?;
            set(DampingRear, damping_rear_setting)?;
        }
        Derived::SwaybarTuning {
            anti_roll_bars_front_setting,
            anti_roll_bars_rear_setting,
        } => {
            set(AntiRollBarFront, anti_roll_bars_front_setting)?;
            set(AntiRollBarRear, anti_roll_bars_rear_setting)?;
        }
        Derived::TireComposition { tire_setting } => set(Tire, tire_setting)?,
        Derived::BrakeDiscs {
            brake_strength_setting,
            brake_bias_setting,
        } => {
            set(BrakeStrength, brake_strength_setting)?;
            set(BrakeBias, brake_bias_setting)?;
        }
        Derived::ControlArmTuning {
            caster_setting,
            camber_front_setting,
            camber_rear_setting,
            ride_height_height_setting,
            ride_height_rake_setting,
            toe_front_setting,
            toe_rear_setting,
            track_width_front_setting,
            track_width_rear_setting,
        } => {
            set(Caster, caster_setting)?;
            set(CamberFront, camber_front_setting)?;
            set(CamberRear, camber_rear_setting)?;
            set(RideHeightHeight, ride_height_height_setting)?;
            set(RideHeightRake, ride_height_rake_setting)?;
            set(ToeFront, toe_front_setting)?;
            set(ToeRear, toe_rear_setting)?;
            set(TrackWidthFront, track_width_front_setting)?;
            set(TrackWidthRear, track_width_rear_setting)?;
        }
        Derived::DifferentialTuning {
            differential_setting,
        } => set(DifferentialType, differential_setting)?,
        Derived::GearboxTuning { gearbox_setting } => set(Gearbox, gearbox_setting)?,
        Derived::HandbrakeTuning {
            handbrake_strength_setting,
        } => set(HandbrakeStrength, handbrake_strength_setting)?,
        _ => (),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
