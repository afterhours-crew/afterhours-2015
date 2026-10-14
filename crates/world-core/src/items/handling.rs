// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{DefinitionClass, Derived, Error};

/// Garage tuning categories 0..30. Category 30 is transmission, not the
/// replicated chassis's unused clutch slot. Sliders carry integer values 0..10.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HandlingChanges([Option<u8>; 31]);

impl HandlingChanges {
    pub fn new(changes: impl IntoIterator<Item = (u8, u8)>) -> Result<Self, Error> {
        let mut result = Self::default();
        for (category, value) in changes {
            let entry = result
                .0
                .get_mut(usize::from(category))
                .ok_or(Error::Shape)?;
            if value > 10 || entry.replace(value).is_some() {
                return Err(Error::Shape);
            }
        }
        Ok(result)
    }
    pub fn iter(&self) -> impl Iterator<Item = (u8, u8)> + '_ {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(i, value)| value.map(|v| (i as u8, v)))
    }
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }
    pub fn transmission(&self) -> Option<u8> {
        self.0[30]
    }
    /// Later calls in the same edit replace earlier values for that category.
    pub fn merge(&mut self, changes: Self) {
        for (index, value) in changes.iter() {
            self.0[usize::from(index)] = Some(value);
        }
    }
    pub fn class(category: u8) -> Result<DefinitionClass, Error> {
        use DefinitionClass::*;
        Ok(match category {
            0 => SpoilerItemData,
            1 | 2 => BrakesItemData,
            3 => HandbrakeTuningItemData,
            4 => DifferentialTuningItemData,
            5..=7 | 26 | 27 | 30 => PersistantTuningItemData,
            8 | 9 => SteeringTuningItemData,
            10..=18 => ControlArmTuningItemData,
            19..=22 => SuspensionTuningItemData,
            23 | 24 => SwaybarTuningItemData,
            25 => TireCompositionItemData,
            28 => NosTuningItemData,
            29 => GearboxTuningItemData,
            _ => return Err(Error::Shape),
        })
    }
    /// Updates one selected part. The caller must validate the definition class
    /// using `class`; cosmetic items can share these serialized layouts.
    pub fn apply(category: u8, value: u8, derived: &mut Derived) -> Result<(), Error> {
        if value > 10 {
            return Err(Error::Shape);
        }
        use Derived::*;
        let slot = match (category, derived) {
            (0, Spoiler { downforce_setting }) => downforce_setting,
            (
                1,
                BrakeDiscs {
                    brake_strength_setting,
                    ..
                },
            ) => brake_strength_setting,
            (
                2,
                BrakeDiscs {
                    brake_bias_setting, ..
                },
            ) => brake_bias_setting,
            (
                3,
                HandbrakeTuning {
                    handbrake_strength_setting,
                },
            ) => handbrake_strength_setting,
            (
                4,
                DifferentialTuning {
                    differential_setting,
                },
            ) => differential_setting,
            (5, PersistantTuning { abs_setting, .. }) => abs_setting,
            (
                6,
                PersistantTuning {
                    stability_control_setting,
                    ..
                },
            ) => stability_control_setting,
            (
                7,
                PersistantTuning {
                    traction_control_setting,
                    ..
                },
            ) => traction_control_setting,
            (
                8,
                SteeringTuning {
                    steer_rate_setting, ..
                },
            ) => steer_rate_setting,
            (
                9,
                SteeringTuning {
                    steer_range_setting,
                    ..
                },
            ) => steer_range_setting,
            (
                10,
                ControlArmTuning {
                    ride_height_height_setting,
                    ..
                },
            ) => ride_height_height_setting,
            (
                11,
                ControlArmTuning {
                    ride_height_rake_setting,
                    ..
                },
            ) => ride_height_rake_setting,
            (
                12,
                ControlArmTuning {
                    track_width_front_setting,
                    ..
                },
            ) => track_width_front_setting,
            (
                13,
                ControlArmTuning {
                    track_width_rear_setting,
                    ..
                },
            ) => track_width_rear_setting,
            (14, ControlArmTuning { caster_setting, .. }) => caster_setting,
            (
                15,
                ControlArmTuning {
                    camber_front_setting,
                    ..
                },
            ) => camber_front_setting,
            (
                16,
                ControlArmTuning {
                    camber_rear_setting,
                    ..
                },
            ) => camber_rear_setting,
            (
                17,
                ControlArmTuning {
                    toe_front_setting, ..
                },
            ) => toe_front_setting,
            (
                18,
                ControlArmTuning {
                    toe_rear_setting, ..
                },
            ) => toe_rear_setting,
            (
                19,
                SuspensionTuning {
                    spring_stiffness_front_setting,
                    ..
                },
            ) => spring_stiffness_front_setting,
            (
                20,
                SuspensionTuning {
                    spring_stiffness_rear_setting,
                    ..
                },
            ) => spring_stiffness_rear_setting,
            (
                21,
                SuspensionTuning {
                    damping_front_setting,
                    ..
                },
            ) => damping_front_setting,
            (
                22,
                SuspensionTuning {
                    damping_rear_setting,
                    ..
                },
            ) => damping_rear_setting,
            (
                23,
                SwaybarTuning {
                    anti_roll_bars_front_setting,
                    ..
                },
            ) => anti_roll_bars_front_setting,
            (
                24,
                SwaybarTuning {
                    anti_roll_bars_rear_setting,
                    ..
                },
            ) => anti_roll_bars_rear_setting,
            (25, TireComposition { tire_setting }) => tire_setting,
            (
                26,
                PersistantTuning {
                    air_pressure_front_setting,
                    ..
                },
            ) => air_pressure_front_setting,
            (
                27,
                PersistantTuning {
                    air_pressure_rear_setting,
                    ..
                },
            ) => air_pressure_rear_setting,
            (28, NosTuning { nos_setting }) => nos_setting,
            (29, GearboxTuning { gearbox_setting }) => gearbox_setting,
            _ => return Err(Error::Shape),
        };
        *slot = u32::from(value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_category_changes_only_its_named_serialized_word() {
        // Independent serializer word order, including the differently ordered
        // control-arm slots. Cosmetic brakes/tires are deliberately not targets.
        let offsets = [
            0, 0, 1, 0, 0, 0, 1, 2, 0, 1, 3, 4, 7, 8, 0, 1, 2, 5, 6, 0, 1, 2, 3, 0, 1, 0, 3, 4, 0,
            0,
        ];
        for (category, offset) in offsets.into_iter().enumerate() {
            let class = HandlingChanges::class(category as u8).unwrap();
            let sizes = match category {
                5..=7 | 26 | 27 => 5,
                8 | 9 | 1 | 2 | 23 | 24 => 2,
                10..=18 => 9,
                19..=22 => 4,
                _ => 1,
            };
            let bytes: Vec<_> = (0..sizes)
                .flat_map(|i| (100u32 + i).to_le_bytes())
                .collect();
            let mut derived = Derived::decode(class.layout(), &bytes).unwrap();
            for value in [0, 5, 10] {
                HandlingChanges::apply(category as u8, value, &mut derived).unwrap();
                let mut expected = bytes.clone();
                expected[offset * 4..offset * 4 + 4]
                    .copy_from_slice(&u32::from(value).to_le_bytes());
                assert_eq!(derived.encode().unwrap(), expected, "category {category}");
            }
            assert!(HandlingChanges::apply(category as u8, 11, &mut derived).is_err());
            assert!(HandlingChanges::apply(category as u8, 5, &mut Derived::Empty).is_err());
        }
        assert_eq!(
            HandlingChanges::class(1),
            Ok(DefinitionClass::BrakesItemData)
        );
        assert_eq!(
            HandlingChanges::class(25),
            Ok(DefinitionClass::TireCompositionItemData)
        );
        assert!(HandlingChanges::class(31).is_err());
    }
    #[test]
    fn bounds_duplicates_and_later_calls_are_explicit() {
        assert!(HandlingChanges::new([(31, 5)]).is_err());
        assert!(HandlingChanges::new([(0, 11)]).is_err());
        assert!(HandlingChanges::new([(0, 5), (0, 6)]).is_err());
        let mut changes = HandlingChanges::new((0..31).map(|c| (c, 5))).unwrap();
        changes.merge(HandlingChanges::new([(26, 6), (30, 10)]).unwrap());
        assert_eq!(changes.iter().count(), 31);
        assert_eq!(changes.iter().find(|(c, _)| *c == 26), Some((26, 6)));
        assert_eq!(changes.transmission(), Some(10));
        assert!(HandlingChanges::default().is_empty());
    }
}
