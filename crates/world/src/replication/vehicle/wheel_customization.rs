// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Tuning, Update, parts};
use crate::items::{Catalog, Collection, DefinitionClass, Derived, Guid, MAX_DEFINITIONS};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Rim {
    pub diameter: f32,
    pub width: f32,
    pub scale: f32,
    pub sizes: Vec<f32>,
    pub flag: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    Rim(Rim),
    Tire(Vec<[f32; 3]>),
    Fender { width: f32, enabled: bool },
}
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub rear: bool,
    pub geometry: Geometry,
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
            let finite = |v: f32| v.is_finite();
            match &input.geometry {
                Geometry::Rim(r) if class == DefinitionClass::RimsItemData => {
                    if r.sizes.len() > 256
                        || ![r.diameter, r.width, r.scale].into_iter().all(finite)
                        || r.sizes.iter().any(|v| !finite(*v))
                    {
                        return Err(Error::Bound);
                    }
                }
                Geometry::Tire(v) if class == DefinitionClass::TiresItemData => {
                    if v.len() > 256 || v.iter().flatten().any(|v| !finite(*v)) {
                        return Err(Error::Bound);
                    }
                }
                Geometry::Fender { width, .. } if class == DefinitionClass::FendersItemData => {
                    if !finite(*width) {
                        return Err(Error::Bound);
                    }
                }
                _ => return Err(Error::TypeMismatch),
            }
            if parts.insert(guid, input).is_some() {
                return Err(Error::DuplicateObject);
            }
        }
        Ok(Self { classes, parts })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Axle {
    diameter: f32,
    width: f32,
    delta: f32,
    tire: [f32; 3],
    fender_width: f32,
    fender_enabled: bool,
    flag: bool,
    tail: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct State([Axle; 2], [usize; 2]);
impl State {
    pub fn from_vehicle(
        collection: &Collection,
        definitions: &Definitions,
        vehicle: u64,
        baseline: [f32; 2],
    ) -> Result<Self, Error> {
        if baseline
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0 || !(1.0 / v).is_finite())
        {
            return Err(Error::Bound);
        }
        let installed = parts::installed(collection, &definitions.classes, vehicle)?;
        let mut rims = [None; 2];
        let mut tires = [None; 2];
        let mut inputs = Vec::new();
        for (item, class) in installed {
            if !matches!(
                class,
                DefinitionClass::RimsItemData
                    | DefinitionClass::TiresItemData
                    | DefinitionClass::FendersItemData
            ) {
                continue;
            }
            let input = definitions
                .parts
                .get(&item.definition)
                .ok_or(Error::UnknownObject)?;
            let axle = usize::from(input.rear);
            match &input.geometry {
                Geometry::Rim(r) => {
                    let Derived::Rims { rim_selection, .. } = item.derived else {
                        return Err(Error::TypeMismatch);
                    };
                    rims[axle] = Some((r, rim_selection));
                }
                Geometry::Tire(t) => tires[axle] = Some(t),
                Geometry::Fender { .. } => (),
            }
            inputs.push(input);
        }
        let mut rim_index = [0; 2];
        let mut tire_index = [0; 2];
        let mut tail = [0.; 2];
        for a in 0..2 {
            let (r, saved) = rims[a].ok_or(Error::Unsupported)?;
            let t = tires[a].ok_or(Error::Unsupported)?;
            if t.is_empty() {
                return Err(Error::Unsupported);
            }
            rim_index[a] = select_rim(r, saved, baseline[a]);
            tire_index[a] = r.sizes.get(rim_index[a]).map_or(0, |size| {
                let ratio = size / baseline[a];
                (usize::from(ratio < 0.755_f32) + usize::from(ratio < 0.665_f32)).min(t.len() - 1)
            });
            tail[a] = r.scale * r.sizes.get(rim_index[a]).copied().unwrap_or(r.diameter) * 2.5;
            if !tail[a].is_finite() {
                return Err(Error::Bound);
            }
        }
        let mut state = Self([Axle::default(); 2], tire_index);
        for (a, base) in state.0.iter_mut().zip(baseline) {
            a.tire[1] = base;
        }
        for input in inputs {
            let i = usize::from(input.rear);
            let a = &mut state.0[i];
            match &input.geometry {
                Geometry::Rim(r) => {
                    a.diameter = r.diameter;
                    a.width = r.width;
                    a.delta = r.sizes.get(rim_index[i]).map_or(0., |v| v - r.diameter);
                    a.flag = r.flag;
                    a.tail = tail[i];
                }
                Geometry::Tire(t) => {
                    if let Some(values) = t.get(tire_index[i]) {
                        a.tire = *values;
                    }
                }
                Geometry::Fender { width, enabled } => {
                    a.fender_width = *width;
                    a.fender_enabled = *enabled;
                }
            }
        }
        if state
            .0
            .iter()
            .any(|a| !a.delta.is_finite() || !(a.fender_width - a.width).is_finite())
        {
            return Err(Error::Bound);
        }
        Ok(state)
    }

    pub fn tire_indices(&self) -> [usize; 2] {
        self.1
    }

    pub fn update(&self, changed: bool) -> Update {
        Update::Tuning(changed.then(|| {
            let [f, r] = self.0;
            let mut values = [0; 14];
            for (out, v) in values[..10].iter_mut().zip([
                f.diameter, f.tire[0], f.tire[1], f.width, f.tire[2], r.diameter, r.tire[0],
                r.tire[1], r.width, r.tire[2],
            ]) {
                *out = unsigned(v, 1., 1023.);
            }
            values[10..].copy_from_slice(&[
                signed(f.delta),
                signed(if f.fender_enabled {
                    f.fender_width - f.width
                } else {
                    0.
                }),
                signed(r.delta),
                signed(if r.fender_enabled {
                    r.fender_width - r.width
                } else {
                    0.
                }),
            ]);
            Tuning {
                values,
                flags: [f.flag, r.flag],
                tail: [unsigned(f.tail, 2., 1023.), unsigned(r.tail, 2., 1023.)],
            }
        }))
    }
}

fn select_rim(rim: &Rim, saved: u32, baseline: f32) -> usize {
    let reciprocal = 1.0_f32 / baseline;
    let (mut lower, mut count) = (0, 0);
    for size in &rim.sizes {
        let ratio = reciprocal * size;
        if ratio < 0.6_f32 {
            lower += 1;
        } else if ratio <= 0.84_f32 {
            count += 1;
        }
    }
    if saved < lower || saved >= lower + count {
        (lower + count / 2) as usize
    } else {
        saved as usize
    }
}
fn unsigned(value: f32, scale: f32, maximum: f32) -> u16 {
    ((value.clamp(0., scale) / scale) * maximum + 0.5) as u16
}
fn signed(value: f32) -> u16 {
    (u16::from(value.is_sign_negative()) << 9) | unsigned(value.abs(), 1., 511.)
}

#[cfg(test)]
mod tests {
    use super::super::{Body, Kind, Profile};
    use super::*;
    use crate::items::{Item, Layout, OWNED};

    fn fixture() -> (Collection, Definitions) {
        let classes = [
            DefinitionClass::RaceVehicleItemData,
            DefinitionClass::RimsItemData,
            DefinitionClass::RimsItemData,
            DefinitionClass::TiresItemData,
            DefinitionClass::TiresItemData,
            DefinitionClass::FendersItemData,
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
            let bytes = match c.layout() {
                Layout::RaceVehicle => 68,
                Layout::Rims => 52,
                Layout::TireComposition => 4,
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
                    derived: Derived::decode(c.layout(), &vec![0; bytes]).unwrap(),
                },
            );
        }
        items.get_mut(&1).unwrap().children = vec![6, 2, 3, 4, 5];
        let rim = Rim {
            diameter: 0.5,
            width: 0.25,
            scale: 0.5,
            sizes: vec![0.5, 0.625, 0.75, 0.875],
            flag: true,
        };
        let entries = [
            (
                2,
                Input {
                    rear: false,
                    geometry: Geometry::Rim(rim.clone()),
                },
            ),
            (
                3,
                Input {
                    rear: true,
                    geometry: Geometry::Rim(rim),
                },
            ),
            (
                4,
                Input {
                    rear: false,
                    geometry: Geometry::Tire(vec![[0.4; 3], [0.5; 3], [0.6; 3]]),
                },
            ),
            (
                5,
                Input {
                    rear: true,
                    geometry: Geometry::Tire(vec![[0.4; 3], [0.5; 3], [0.6; 3]]),
                },
            ),
            (
                6,
                Input {
                    rear: false,
                    geometry: Geometry::Fender {
                        width: 0.375,
                        enabled: true,
                    },
                },
            ),
        ];
        (
            Collection {
                roots: vec![1],
                items,
            },
            Definitions::new(
                catalog,
                entries
                    .into_iter()
                    .map(|(i, v)| ((i as u128).to_le_bytes(), v)),
            )
            .unwrap(),
        )
    }
    fn wire(s: &State) -> Tuning {
        let Update::Tuning(Some(t)) = s.update(true) else {
            panic!()
        };
        t
    }
    #[test]
    fn effective_selection_drives_tires_and_fenders_independently_of_saved_tire_index() {
        let (mut c, d) = fixture();
        c.items.get_mut(&4).unwrap().derived = Derived::TireComposition {
            tire_setting: u32::MAX,
        };
        let before = c.clone();
        let state = State::from_vehicle(&c, &d, 1, [1.; 2]).unwrap();
        assert_eq!(state.0[0].delta, 0.25);
        assert_eq!(state.0[0].tire, [0.5; 3]);
        assert_eq!(wire(&state).values[10..], [128, 64, 128, 0]);
        assert_eq!(wire(&state).tail, [480; 2]);
        assert_eq!(c, before);
        assert_eq!(state.update(false), Update::Tuning(None));
        let p = Profile::new(vec![
            Kind::Root {
                property_owner: false,
            },
            Kind::Tuning,
        ])
        .unwrap();
        let b = Body {
            creation: None,
            updates: vec![None, Some(state.update(true))],
        };
        let raw = b.encode(&p).unwrap();
        assert_eq!(Body::decode(raw.span(), &p, false).unwrap().body, b);
    }
    #[test]
    fn rim_window_includes_both_boundaries_and_preserves_valid_saved_selection() {
        let r = Rim {
            diameter: 0.5,
            width: 0.25,
            scale: 1.,
            sizes: vec![0.5, 0.6, 0.7, 0.84, 0.9],
            flag: false,
        };
        assert_eq!(select_rim(&r, 1, 1.), 1);
        assert_eq!(select_rim(&r, 3, 1.), 3);
        assert_eq!(select_rim(&r, 0, 1.), 2);
        assert_eq!(select_rim(&r, u32::MAX, 1.), 2);
        let (mut c, d) = fixture();
        let original = State::from_vehicle(&c, &d, 1, [1.; 2]).unwrap();
        if let Derived::Rims { rim_selection, .. } = &mut c.items.get_mut(&2).unwrap().derived {
            *rim_selection = 1;
        }
        let changed = State::from_vehicle(&c, &d, 1, [1.; 2]).unwrap();
        assert_eq!(changed.0[0].delta, 0.125);
        assert_eq!(changed.0[0].tire, [0.6; 3]);
        assert_eq!(changed.0[1], original.0[1]);
    }
    #[test]
    fn missing_parts_fail_but_empty_rim_sizes_use_native_base_geometry() {
        let (mut c, mut d) = fixture();
        if let Geometry::Rim(r) = &mut d.parts.get_mut(&2u128.to_le_bytes()).unwrap().geometry {
            r.sizes.clear();
        }
        let s = State::from_vehicle(&c, &d, 1, [1.; 2]).unwrap();
        assert_eq!(s.0[0].delta, 0.);
        assert_eq!(s.0[0].tire, [0.4; 3]);
        assert_eq!(s.0[0].tail, 0.625);
        c.items.get_mut(&1).unwrap().children.retain(|id| *id != 2);
        c.items.get_mut(&1).unwrap().defaults.push(2);
        assert_eq!(
            State::from_vehicle(&c, &d, 1, [1.; 2]),
            Err(Error::Unsupported)
        );
    }
    #[test]
    fn fender_enable_and_order_changes_rebuild_without_leaking_between_vehicles() {
        let (mut c, mut d) = fixture();
        let a = State::from_vehicle(&c, &d, 1, [1.; 2]).unwrap();
        c.items.get_mut(&1).unwrap().children = vec![2, 3, 4, 5, 6];
        assert_eq!(State::from_vehicle(&c, &d, 1, [1.; 2]).unwrap(), a);
        d.parts.get_mut(&6u128.to_le_bytes()).unwrap().geometry = Geometry::Fender {
            width: 0.125,
            enabled: true,
        };
        let b = State::from_vehicle(&c, &d, 1, [1.; 2]).unwrap();
        assert_eq!(wire(&b).values[11], 576);
        d.parts.get_mut(&6u128.to_le_bytes()).unwrap().geometry = Geometry::Fender {
            width: 0.125,
            enabled: false,
        };
        assert_eq!(
            wire(&State::from_vehicle(&c, &d, 1, [1.; 2]).unwrap()).values[11],
            0
        );
        assert_eq!(wire(&a).values[11], 64);
    }
    #[test]
    fn quantization_uses_sign_first_including_negative_zero_and_clamps() {
        for (v, expected) in [
            (0., 0),
            (-0., 512),
            (1., 511),
            (-1., 1023),
            (2., 511),
            (-2., 1023),
            (0.5, 256),
            (-0.5, 768),
        ] {
            assert_eq!(signed(v), expected);
        }
        assert_eq!(unsigned(3., 2., 1023.), 1023);
        assert_eq!(unsigned(-1., 2., 1023.), 0);
        assert_eq!(unsigned(1., 2., 1023.), 512);
    }
    #[test]
    fn content_and_current_graph_bounds_fail_before_state_is_exposed() {
        let (c, d) = fixture();
        for base in [0., -1., f32::NAN, f32::INFINITY, f32::from_bits(1)] {
            assert_eq!(
                State::from_vehicle(&c, &d, 1, [base, 1.]),
                Err(Error::Bound)
            );
        }
        let key = 2u128.to_le_bytes();
        let mut input = d.parts[&key].clone();
        if let Geometry::Rim(r) = &mut input.geometry {
            r.sizes = vec![0.; 257];
        }
        assert_eq!(
            Definitions::new(d.classes.clone(), [(key, input)]),
            Err(Error::Bound)
        );
        assert_eq!(
            Definitions::new(d.classes.clone(), vec![(key, d.parts[&key].clone()); 2]),
            Err(Error::DuplicateObject)
        );
        let mut broken = d.clone();
        broken.parts.remove(&key);
        assert_eq!(
            State::from_vehicle(&c, &broken, 1, [1.; 2]),
            Err(Error::UnknownObject)
        );
        let mut c = c;
        c.items.get_mut(&4).unwrap().owner = 2;
        assert_eq!(State::from_vehicle(&c, &d, 1, [1.; 2]), Err(Error::Shape));
    }
}
