// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Appearance, Error, Paint, Resource, Update, Wrap, parts};
use crate::items::{Catalog, Collection, DefinitionClass, Derived, Guid, MAX_DEFINITIONS};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Defaults {
    pub palettes: [[[f32; 3]; 4]; 2],
    pub static_wrap_counts: Option<[u16; 2]>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Rim { rear: bool },
    Plate(i32),
    Frame([i32; 2]),
    StaticWrap([u32; 2]),
    DynamicWrap { data_id: u64, version: i32 },
}
#[derive(Clone, Debug, PartialEq)]
pub struct Definitions {
    classes: Catalog,
    parts: BTreeMap<Guid, Input>,
}
impl Definitions {
    pub fn new(
        classes: Catalog,
        entries: impl IntoIterator<Item = (Guid, Input)>,
    ) -> Result<Self, Error> {
        let mut parts = BTreeMap::new();
        for (guid, input) in entries {
            if parts.len() >= MAX_DEFINITIONS {
                return Err(Error::Bound);
            }
            let class = classes.class(&guid).map_err(|_| Error::UnknownObject)?;
            match (&input, class) {
                (Input::Rim { .. }, DefinitionClass::RimsItemData)
                | (Input::StaticWrap(_), DefinitionClass::StaticLiveryCustomizationItemData)
                | (Input::DynamicWrap { .. }, DefinitionClass::LiveryCustomizationItemData) => (),
                (Input::Plate(v), DefinitionClass::LicensePlateBackgroundItemData)
                    if (-1..=254).contains(v) => {}
                (Input::Frame(v), DefinitionClass::LicensePlateFrameItemData)
                    if v.iter().all(|v| (-1..=254).contains(v)) => {}
                _ => return Err(Error::TypeMismatch),
            }
            if parts.insert(guid, input).is_some() {
                return Err(Error::DuplicateObject);
            }
        }
        Ok(Self { classes, parts })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    paint: Paint,
    palettes: [[[u8; 3]; 4]; 2],
    wrap: Wrap,
    plate: u8,
    frame: [u8; 2],
}
impl State {
    pub fn from_vehicle(
        collection: &Collection,
        definitions: &Definitions,
        vehicle: u64,
        defaults: &Defaults,
        owner: Resource,
    ) -> Result<Self, Error> {
        if owner.name.len() > 16
            || owner.name.contains(&0)
            || defaults
                .static_wrap_counts
                .is_some_and(|v| v.iter().any(|n| *n > 256))
        {
            return Err(Error::Bound);
        }
        let mut palettes = [[[0; 3]; 4]; 2];
        for (out, input) in palettes.iter_mut().zip(defaults.palettes) {
            for (out, input) in out.iter_mut().zip(input) {
                *out = rgb(input)?;
            }
        }
        let installed = parts::installed(collection, &definitions.classes, vehicle)?;
        let Derived::RaceVehicle {
            default_paint,
            material,
            window_tint,
            default_license_plate_text,
            vehicle_item_flags,
            ..
        } = installed[0].0.derived
        else {
            return Err(Error::TypeMismatch);
        };
        let end = default_license_plate_text
            .iter()
            .position(|v| *v == 0)
            .unwrap_or(8);
        let mut state = Self {
            paint: Paint {
                colors: [
                    rgb(default_paint.map(f32::from_bits))?,
                    rgb(material.map(f32::from_bits))?,
                ],
                value: unit(f32::from_bits(window_tint))?,
                name: default_license_plate_text[..end].to_vec(),
                flag: vehicle_item_flags & 1 != 0,
                resource: owner.clone(),
            },
            palettes,
            wrap: Wrap::Custom {
                words: [0; 2],
                resource: Resource {
                    words: [0; 2],
                    name: vec![],
                },
                flag: false,
                value: -1,
            },
            plate: 0,
            frame: [0; 2],
        };
        let mut livery_seen = false;
        for (item, class) in installed {
            if !matches!(
                class,
                DefinitionClass::RimsItemData
                    | DefinitionClass::LicensePlateBackgroundItemData
                    | DefinitionClass::LicensePlateFrameItemData
                    | DefinitionClass::StaticLiveryCustomizationItemData
                    | DefinitionClass::LiveryCustomizationItemData
            ) {
                continue;
            }
            let input = definitions
                .parts
                .get(&item.definition)
                .ok_or(Error::UnknownObject)?;
            match input {
                Input::Rim { rear } => {
                    let Derived::Rims {
                        primary_paint,
                        primary_material,
                        secondary_paint,
                        secondary_material,
                        ..
                    } = item.derived
                    else {
                        return Err(Error::TypeMismatch);
                    };
                    let values = [
                        primary_paint,
                        primary_material,
                        secondary_paint,
                        secondary_material,
                    ];
                    for (out, input) in state.palettes[usize::from(*rear)].iter_mut().zip(values) {
                        *out = rgb(input.map(f32::from_bits))?;
                    }
                }
                Input::Plate(v) => state.plate = (v + 1) as u8,
                Input::Frame(v) => state.frame = v.map(|v| (v + 1) as u8),
                Input::StaticWrap(v) if !livery_seen => {
                    livery_seen = true;
                    if defaults
                        .static_wrap_counts
                        .is_some_and(|n| (0..2).all(|i| v[i] < u32::from(n[i])))
                    {
                        state.wrap = Wrap::Preset(v.map(|v| v as u8));
                    }
                }
                Input::DynamicWrap { data_id, version } if !livery_seen => {
                    livery_seen = true;
                    let Derived::LiveryCustomization { dynamic_flags, .. } = item.derived else {
                        return Err(Error::TypeMismatch);
                    };
                    if *data_id != 0 {
                        state.wrap = Wrap::Custom {
                            words: [*data_id as u32, (*data_id >> 32) as u32],
                            resource: owner.clone(),
                            flag: dynamic_flags & 1 != 0,
                            value: *version,
                        };
                    }
                }
                Input::StaticWrap(_) | Input::DynamicWrap { .. } => (),
            }
        }
        Ok(state)
    }
    pub fn update(&self, mask: u8) -> Result<Update, Error> {
        if mask & !63 != 0 {
            return Err(Error::Bound);
        }
        Ok(Update::Appearance(Box::new(Appearance {
            paint: (mask & 2 != 0).then(|| self.paint.clone()),
            palette1: (mask & 4 != 0).then_some(self.palettes[0]),
            palette2: (mask & 8 != 0).then_some(self.palettes[1]),
            wrap: (mask & 1 != 0).then(|| self.wrap.clone()),
            index: (mask & 16 != 0).then_some(self.plate),
            indices: (mask & 32 != 0).then_some(self.frame),
        })))
    }
}
fn unit(value: f32) -> Result<u8, Error> {
    if !value.is_finite() {
        return Err(Error::Bound);
    }
    Ok((value.clamp(0., 1.) * 255. + 0.5) as u8)
}
fn rgb(value: [f32; 3]) -> Result<[u8; 3], Error> {
    Ok([unit(value[0])?, unit(value[1])?, unit(value[2])?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{Item, Layout, OWNED};
    fn fixture() -> (Collection, Definitions, Defaults, Resource) {
        let classes = [
            DefinitionClass::RaceVehicleItemData,
            DefinitionClass::RimsItemData,
            DefinitionClass::RimsItemData,
            DefinitionClass::LicensePlateBackgroundItemData,
            DefinitionClass::LicensePlateFrameItemData,
            DefinitionClass::StaticLiveryCustomizationItemData,
            DefinitionClass::LiveryCustomizationItemData,
        ];
        let catalog = Catalog::new(
            classes
                .iter()
                .enumerate()
                .map(|(i, c)| ((i as u128 + 1).to_le_bytes(), *c)),
        )
        .unwrap();
        let mut items = BTreeMap::new();
        for (i, c) in classes.iter().enumerate() {
            let id = i as u64 + 1;
            let n = match c.layout() {
                Layout::RaceVehicle => 68,
                Layout::Rims => 52,
                Layout::LiveryCustomization => 12,
                _ => 0,
            };
            items.insert(
                id,
                Item {
                    id,
                    owner: if id == 1 { 0 } else { 1 },
                    definition: u128::from(id).to_le_bytes(),
                    defaults: vec![],
                    children: vec![],
                    state: OWNED,
                    buy_price: 0,
                    sell_price: 0,
                    derived: Derived::decode(c.layout(), &vec![0; n]).unwrap(),
                },
            );
        }
        items.get_mut(&1).unwrap().children = vec![2, 3, 4, 5, 6, 7];
        if let Derived::RaceVehicle {
            default_paint,
            material,
            window_tint,
            default_license_plate_text,
            vehicle_item_flags,
            ..
        } = &mut items.get_mut(&1).unwrap().derived
        {
            *default_paint = [(-1_f32).to_bits(), 0.5_f32.to_bits(), 2_f32.to_bits()];
            *material = [0.25_f32.to_bits(); 3];
            *window_tint = 0.9_f32.to_bits();
            *default_license_plate_text = *b"TEST\0xxx";
            *vehicle_item_flags = 1;
        }
        if let Derived::Rims {
            primary_paint,
            secondary_paint,
            ..
        } = &mut items.get_mut(&2).unwrap().derived
        {
            *primary_paint = [0.5_f32.to_bits(); 3];
            *secondary_paint = [1_f32.to_bits(); 3];
        }
        let definitions = Definitions::new(
            catalog,
            [
                (2, Input::Rim { rear: false }),
                (3, Input::Rim { rear: true }),
                (4, Input::Plate(254)),
                (5, Input::Frame([-1, 14])),
                (6, Input::StaticWrap([1, 2])),
                (
                    7,
                    Input::DynamicWrap {
                        data_id: 0x123456789abcdef0,
                        version: 17,
                    },
                ),
            ]
            .map(|(i, v)| ((i as u128).to_le_bytes(), v)),
        )
        .unwrap();
        (
            Collection {
                roots: vec![1],
                items,
            },
            definitions,
            Defaults {
                palettes: [[[0.25; 3]; 4]; 2],
                static_wrap_counts: Some([2, 3]),
            },
            Resource {
                words: [1, 2],
                name: b"owner-a".to_vec(),
            },
        )
    }
    fn state(c: &Collection, d: &Definitions, b: &Defaults, o: &Resource) -> State {
        State::from_vehicle(c, d, 1, b, o.clone()).unwrap()
    }
    #[test]
    fn body_rims_plates_and_first_wrap_come_from_current_items() {
        let (c, d, b, o) = fixture();
        let s = state(&c, &d, &b, &o);
        assert_eq!(s.paint.colors, [[0, 128, 255], [64; 3]]);
        assert_eq!(s.paint.value, 230);
        assert_eq!(s.paint.name, b"TEST");
        assert!(s.paint.flag);
        assert_eq!(s.paint.resource, o);
        assert_eq!(s.plate, 255);
        assert_eq!(s.frame, [0, 15]);
        assert_eq!(s.palettes[0], [[128; 3], [0; 3], [255; 3], [0; 3]]);
        assert_eq!(s.palettes[1], [[0; 3]; 4]);
        assert_eq!(s.wrap, Wrap::Preset([1, 2]));
    }
    #[test]
    fn invalid_first_preset_stops_livery_search_and_absence_keeps_static_palette() {
        let (mut c, d, mut b, o) = fixture();
        b.static_wrap_counts = Some([1, 3]);
        c.items.get_mut(&1).unwrap().children = vec![4, 5, 6, 7];
        let s = state(&c, &d, &b, &o);
        assert_eq!(s.palettes, [[[64; 3]; 4]; 2]);
        assert_eq!(
            s.wrap,
            Wrap::Custom {
                words: [0; 2],
                resource: Resource {
                    words: [0; 2],
                    name: vec![]
                },
                flag: false,
                value: -1
            }
        );
        b.static_wrap_counts = None;
        assert_eq!(state(&c, &d, &b, &o), s);
    }
    #[test]
    fn dynamic_wrap_uses_definition_key_current_flag_and_per_player_resource() {
        let (mut c, mut d, b, o) = fixture();
        c.items.get_mut(&1).unwrap().children = vec![7, 6];
        if let Derived::LiveryCustomization { dynamic_flags, .. } =
            &mut c.items.get_mut(&7).unwrap().derived
        {
            *dynamic_flags = 3;
        }
        let s = state(&c, &d, &b, &o);
        assert_eq!(
            s.wrap,
            Wrap::Custom {
                words: [0x9abcdef0, 0x12345678],
                resource: o.clone(),
                flag: true,
                value: 17
            }
        );
        d.parts.insert(
            7_u128.to_le_bytes(),
            Input::DynamicWrap {
                data_id: 0,
                version: 2,
            },
        );
        let zero = state(&c, &d, &b, &o);
        assert!(matches!(
            zero.wrap,
            Wrap::Custom {
                words: [0, 0],
                flag: false,
                value: -1,
                ..
            }
        ));
    }
    #[test]
    fn dirty_fields_are_independent_and_instances_do_not_share_identity() {
        let (c, d, b, o) = fixture();
        let s = state(&c, &d, &b, &o);
        for bit in 0..6 {
            let Update::Appearance(a) = s.update(1 << bit).unwrap() else {
                panic!()
            };
            let flags = [
                a.wrap.is_some(),
                a.paint.is_some(),
                a.palette1.is_some(),
                a.palette2.is_some(),
                a.index.is_some(),
                a.indices.is_some(),
            ];
            assert_eq!(flags.iter().filter(|x| **x).count(), 1);
            assert!(flags[bit]);
        }
        assert_eq!(s.update(64), Err(Error::Bound));
        let different = Resource {
            words: [3, 4],
            name: b"owner-b".to_vec(),
        };
        let other = state(&c, &d, &b, &different);
        assert_ne!(s.paint.resource, other.paint.resource);
        assert_eq!(s, state(&c, &d, &b, &o));
    }
    #[test]
    fn later_rim_item_wins_in_recursive_preorder_and_reconstruction_is_stable() {
        let (mut c, mut d, b, o) = fixture();
        d.parts
            .insert(3_u128.to_le_bytes(), Input::Rim { rear: false });
        c.items.get_mut(&1).unwrap().children = vec![2];
        c.items.get_mut(&2).unwrap().children = vec![3];
        c.items.get_mut(&3).unwrap().owner = 2;
        let s = state(&c, &d, &b, &o);
        assert_eq!(s.palettes[0], [[0; 3]; 4]);
        assert_eq!(s.palettes[1], [[64; 3]; 4]);
        assert_eq!(s, state(&c, &d, &b, &o));
    }
    #[test]
    fn invalid_inputs_fail_without_changing_inventory() {
        let (mut c, d, mut b, mut o) = fixture();
        let before = c.clone();
        o.name = vec![b'x'; 17];
        assert_eq!(
            State::from_vehicle(&c, &d, 1, &b, o.clone()),
            Err(Error::Bound)
        );
        assert_eq!(c, before);
        o.name.clear();
        b.palettes[0][0][0] = f32::NAN;
        assert_eq!(
            State::from_vehicle(&c, &d, 1, &b, o.clone()),
            Err(Error::Bound)
        );
        b.palettes[0][0][0] = 0.;
        b.static_wrap_counts = Some([257, 3]);
        assert_eq!(
            State::from_vehicle(&c, &d, 1, &b, o.clone()),
            Err(Error::Bound)
        );
        b.static_wrap_counts = None;
        c.items.get_mut(&1).unwrap().children.push(2);
        assert_eq!(
            State::from_vehicle(&c, &d, 1, &b, o),
            Err(Error::DuplicateObject)
        );
        assert_eq!(unit(f32::INFINITY), Err(Error::Bound));
        assert_eq!(
            Definitions::new(
                d.classes.clone(),
                [(4_u128.to_le_bytes(), Input::Plate(255))]
            ),
            Err(Error::TypeMismatch)
        );
    }
}
