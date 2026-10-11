// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Update, parts};
use crate::items::{Catalog, Collection, DefinitionClass, Derived, Guid, MAX_DEFINITIONS};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Definitions {
    classes: Catalog,
    variants: BTreeMap<Guid, u8>,
}
impl Definitions {
    pub fn new(
        classes: Catalog,
        entries: impl IntoIterator<Item = (Guid, u8)>,
    ) -> Result<Self, Error> {
        let mut variants = BTreeMap::new();
        for (guid, variant) in entries {
            if variants.len() >= MAX_DEFINITIONS || variant > 15 {
                return Err(Error::Bound);
            }
            if classes.class(&guid).map_err(|_| Error::UnknownObject)?
                != DefinitionClass::NosTuningItemData
            {
                return Err(Error::TypeMismatch);
            }
            if variants.insert(guid, variant).is_some() {
                return Err(Error::DuplicateObject);
            }
        }
        Ok(Self { classes, variants })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct State {
    pub variant: Option<u8>,
    pub setting: u8,
}
impl State {
    pub fn from_vehicle(
        collection: &Collection,
        definitions: &Definitions,
        vehicle: u64,
    ) -> Result<Self, Error> {
        let mut result = Self::default();
        for (item, class) in parts::installed(collection, &definitions.classes, vehicle)? {
            if class != DefinitionClass::NosTuningItemData {
                continue;
            }
            let Derived::NosTuning { nos_setting } = item.derived else {
                return Err(Error::TypeMismatch);
            };
            if nos_setting > 15 {
                return Err(Error::Bound);
            }
            result.variant = Some(
                *definitions
                    .variants
                    .get(&item.definition)
                    .ok_or(Error::UnknownObject)?,
            );
            result.setting = nos_setting as u8;
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Runtime {
    mode: u8,
    amount: f32,
    capacity: f32,
}
impl Runtime {
    pub fn new(capacity: f32) -> Result<Self, Error> {
        if !capacity.is_finite() || capacity <= 0.0 {
            return Err(Error::Shape);
        }
        Ok(Self {
            mode: 0,
            amount: capacity,
            capacity,
        })
    }
    pub fn set_mode(&mut self, mode: u8) -> Result<(), Error> {
        if mode > 3 {
            return Err(Error::Bound);
        }
        self.mode = mode;
        Ok(())
    }
    pub fn set_amount(&mut self, amount: f32) -> Result<(), Error> {
        if !amount.is_finite() {
            return Err(Error::Shape);
        }
        self.amount = amount;
        Ok(())
    }
    pub fn update(&self, mask: u8) -> Result<Update, Error> {
        if mask & !3 != 0 {
            return Err(Error::Bound);
        }
        let level = (self.amount / self.capacity).clamp(0.0, 1.0);
        Ok(Update::Index {
            index: (mask & 1 != 0).then_some(self.mode),
            value: (mask & 2 != 0).then_some((level * 255.0 + 0.5) as u8),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{Item, Layout, OWNED};

    #[test]
    fn runtime_full_capacity_is_scale_independent_and_masks_are_separate() {
        for capacity in [f32::from_bits(1), 0.125, 1., 127.5, 10000., f32::MAX] {
            let mut r = Runtime::new(capacity).unwrap();
            assert_eq!(
                r.update(3),
                Ok(Update::Index {
                    index: Some(0),
                    value: Some(255)
                })
            );
            assert_eq!(
                r.update(0),
                Ok(Update::Index {
                    index: None,
                    value: None
                })
            );
            r.set_mode(2).unwrap();
            assert_eq!(
                r.update(1),
                Ok(Update::Index {
                    index: Some(2),
                    value: None
                })
            );
            assert_eq!(
                r.update(2),
                Ok(Update::Index {
                    index: None,
                    value: Some(255)
                })
            );
        }
    }
    #[test]
    fn runtime_quantizes_rounding_clamps_and_preserves_state_on_invalid_input() {
        let mut r = Runtime::new(1.).unwrap();
        let other = r.clone();
        for (amount, byte) in [
            (-1., 0),
            (0., 0),
            (1. / 255., 1),
            (0.49, 125),
            (0.5, 128),
            (1., 255),
            (2., 255),
        ] {
            r.set_amount(amount).unwrap();
            assert_eq!(
                r.update(2),
                Ok(Update::Index {
                    index: None,
                    value: Some(byte)
                })
            );
        }
        assert_eq!(
            other.update(3),
            Ok(Update::Index {
                index: Some(0),
                value: Some(255)
            })
        );
        let before = r.clone();
        assert_eq!(r.set_mode(4), Err(Error::Bound));
        assert_eq!(r.set_amount(f32::NAN), Err(Error::Shape));
        assert_eq!(r.set_amount(f32::INFINITY), Err(Error::Shape));
        assert_eq!(r.update(4), Err(Error::Bound));
        assert_eq!(r, before);
        for capacity in [0., -1., f32::NAN, f32::INFINITY] {
            assert_eq!(Runtime::new(capacity), Err(Error::Shape));
        }
    }

    fn fixture() -> (Collection, Definitions) {
        let item = |id, owner, derived| Item {
            id,
            owner,
            definition: u128::from(id).to_le_bytes(),
            defaults: vec![],
            children: vec![],
            state: OWNED,
            buy_price: 0,
            sell_price: 0,
            derived,
        };
        let mut car = item(
            1,
            0,
            Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap(),
        );
        car.children = vec![2];
        let mut nos = item(2, 1, Derived::NosTuning { nos_setting: 7 });
        nos.children = vec![3];
        let nested = item(3, 2, Derived::NosTuning { nos_setting: 4 });
        let collection = Collection {
            roots: vec![1],
            items: BTreeMap::from([(1, car), (2, nos), (3, nested)]),
        };
        let classes = Catalog::new([
            (1u128.to_le_bytes(), DefinitionClass::RaceVehicleItemData),
            (2u128.to_le_bytes(), DefinitionClass::NosTuningItemData),
            (3u128.to_le_bytes(), DefinitionClass::NosTuningItemData),
        ])
        .unwrap();
        let definitions = Definitions::new(
            classes,
            [(2u128.to_le_bytes(), 2), (3u128.to_le_bytes(), 0)],
        )
        .unwrap();
        (collection, definitions)
    }

    #[test]
    fn recursive_current_order_present_zero_removal_and_isolation() {
        let (mut a, definitions) = fixture();
        let b = a.clone();
        assert_eq!(
            State::from_vehicle(&a, &definitions, 1).unwrap(),
            State {
                variant: Some(0),
                setting: 4
            }
        );
        a.items.get_mut(&2).unwrap().children.clear();
        assert_eq!(
            State::from_vehicle(&a, &definitions, 1).unwrap(),
            State {
                variant: Some(2),
                setting: 7
            }
        );
        a.items.get_mut(&1).unwrap().children.clear();
        a.items.get_mut(&1).unwrap().defaults = vec![2];
        assert_eq!(
            State::from_vehicle(&a, &definitions, 1).unwrap(),
            State::default()
        );
        assert_eq!(
            State::from_vehicle(&b, &definitions, 1).unwrap(),
            State {
                variant: Some(0),
                setting: 4
            }
        );
    }

    #[test]
    fn missing_static_input_and_out_of_range_current_setting_fail() {
        let (mut collection, mut definitions) = fixture();
        definitions.variants.remove(&3u128.to_le_bytes());
        assert_eq!(
            State::from_vehicle(&collection, &definitions, 1),
            Err(Error::UnknownObject)
        );
        definitions.variants.insert(3u128.to_le_bytes(), 0);
        collection.items.get_mut(&3).unwrap().derived = Derived::NosTuning { nos_setting: 16 };
        assert_eq!(
            State::from_vehicle(&collection, &definitions, 1),
            Err(Error::Bound)
        );
        collection.items.get_mut(&3).unwrap().owner = 1;
        assert_eq!(
            State::from_vehicle(&collection, &definitions, 1),
            Err(Error::Shape)
        );
    }

    #[test]
    fn static_class_duplicate_and_index_bounds_fail() {
        let (_, definitions) = fixture();
        let classes = definitions.classes;
        assert_eq!(
            Definitions::new(classes.clone(), [(1u128.to_le_bytes(), 0)]),
            Err(Error::TypeMismatch)
        );
        assert_eq!(
            Definitions::new(classes.clone(), [(2u128.to_le_bytes(), 16)]),
            Err(Error::Bound)
        );
        assert_eq!(
            Definitions::new(classes.clone(), [(2u128.to_le_bytes(), 0); 2]),
            Err(Error::DuplicateObject)
        );
        assert_eq!(
            Definitions::new(classes, [(9u128.to_le_bytes(), 0)]),
            Err(Error::UnknownObject)
        );
    }
}
