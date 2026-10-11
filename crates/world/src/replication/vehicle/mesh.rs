// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{
    Error, MAX_MESHES, Mesh, MeshCustomization, MeshExtra, MeshKind, MeshValue, Update, parts,
};
use crate::items::{Catalog, Collection, DefinitionClass, Guid, MAX_DEFINITIONS};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variant {
    pub guid: Option<Guid>,
    pub bundle: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Component {
    pub kind: MeshKind,
    pub variants: Vec<Variant>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Ignore,
    Part {
        guids: Vec<Guid>,
        rim: Option<[f32; 3]>,
    },
    Tire {
        rear: bool,
        alternatives: Vec<Vec<Guid>>,
    },
    Unsupported,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Definitions {
    classes: Catalog,
    inputs: BTreeMap<Guid, Input>,
}
impl Definitions {
    pub fn new(
        classes: Catalog,
        entries: impl IntoIterator<Item = (Guid, Input)>,
    ) -> Result<Self, Error> {
        let mut inputs = BTreeMap::new();
        for (guid, input) in entries {
            if inputs.len() >= MAX_DEFINITIONS {
                return Err(Error::Bound);
            }
            let class = classes.class(&guid).map_err(|_| Error::UnknownObject)?;
            match &input {
                Input::Part { guids, rim } => {
                    if guids.len() > 256 {
                        return Err(Error::Bound);
                    }
                    if let Some(r) = rim {
                        if class != DefinitionClass::RimsItemData {
                            return Err(Error::TypeMismatch);
                        }
                        rim_value(*r)?;
                    }
                }
                Input::Tire { alternatives, .. } => {
                    if class != DefinitionClass::TiresItemData {
                        return Err(Error::TypeMismatch);
                    }
                    if alternatives.len() > 256 || alternatives.iter().any(|v| v.len() > 256) {
                        return Err(Error::Bound);
                    }
                }
                Input::Ignore | Input::Unsupported => (),
            }
            if inputs.insert(guid, input).is_some() {
                return Err(Error::DuplicateObject);
            }
        }
        Ok(Self { classes, inputs })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State(MeshCustomization);
impl State {
    pub fn from_vehicle(
        collection: &Collection,
        definitions: &Definitions,
        vehicle: u64,
        components: &[Component],
        tire_indices: [usize; 2],
        mut resolve: impl FnMut(&str) -> Option<u16>,
    ) -> Result<Self, Error> {
        validate_components(components)?;
        if components.iter().any(|c| c.kind.extra == MeshExtra::Light) {
            return Err(Error::Unsupported);
        }
        if tire_indices.into_iter().any(|v| v >= 256) {
            return Err(Error::Bound);
        }
        let mut meshes: Vec<_> = components
            .iter()
            .map(|c| Mesh {
                index: -1,
                extra: match c.kind.extra {
                    MeshExtra::None => MeshValue::None,
                    MeshExtra::Rim => MeshValue::Rim {
                        first: 0,
                        middle: 0,
                        last: 0,
                    },
                    MeshExtra::Light => MeshValue::Light(0),
                },
            })
            .collect();
        let mut bundles = vec![None; components.len()];
        for (item, _) in parts::installed(collection, &definitions.classes, vehicle)? {
            let input = definitions
                .inputs
                .get(&item.definition)
                .ok_or(Error::UnknownObject)?;
            let (mut guids, rim) = match input {
                Input::Ignore => continue,
                Input::Unsupported => return Err(Error::Unsupported),
                Input::Part { guids, rim } => (guids.clone(), *rim),
                Input::Tire { rear, alternatives } => {
                    let Some(guids) = alternatives.get(tire_indices[usize::from(*rear)]) else {
                        continue;
                    };
                    (guids.clone(), None)
                }
            };
            for (i, c) in components.iter().enumerate() {
                let found = c.variants.iter().enumerate().find_map(|(index, v)| {
                    let at = guids.iter().position(|g| Some(*g) == v.guid)?;
                    Some((index, at, v))
                });
                if let Some((index, at, v)) = found {
                    guids.remove(at);
                    meshes[i].index = index as i16;
                    bundles[i] = v.bundle.as_deref();
                    if c.kind.extra == MeshExtra::Rim
                        && let Some(rim) = rim
                    {
                        meshes[i].extra = rim_value(rim)?;
                    }
                }
            }
        }
        let mut assets = Vec::new();
        for bundle in bundles.into_iter().flatten() {
            let id = resolve(bundle)
                .filter(|v| *v != 0)
                .ok_or(Error::UnknownObject)?;
            if !assets.contains(&id) {
                assets.push(id);
            }
        }
        Ok(Self(MeshCustomization { meshes, assets }))
    }
    pub fn update(&self, changed: bool) -> Update {
        Update::Mesh(changed.then(|| self.0.clone()))
    }
}
fn validate_components(components: &[Component]) -> Result<(), Error> {
    if components.is_empty() || components.len() > MAX_MESHES {
        return Err(Error::Bound);
    }
    for c in components {
        if !(1..=16).contains(&c.kind.index_bits)
            || c.variants.len() > 256
            || c.variants.len() > (1usize << (c.kind.index_bits - 1))
        {
            return Err(Error::Bound);
        }
        for v in &c.variants {
            if v.bundle
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 512 || !s.is_ascii() || s.contains('\0'))
            {
                return Err(Error::Bound);
            }
        }
    }
    Ok(())
}
fn rim_value(values: [f32; 3]) -> Result<MeshValue, Error> {
    let values = [values[0], values[1] * 0.5, values[2]];
    let mut q = [0i32; 3];
    for (out, v) in q.iter_mut().zip(values) {
        let scaled = v * 1000.;
        if !scaled.is_finite() || !(-2147483648.0..2147483648.0).contains(&scaled) {
            return Err(Error::Bound);
        }
        *out = scaled as i32;
    }
    let signed = |v: i32| ((v << 23) >> 23) as i16;
    Ok(MeshValue::Rim {
        first: signed(q[0]),
        middle: (q[1] & 511) as u16,
        last: signed(q[2]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{Derived, Item, Layout, OWNED};
    fn guid(n: u8) -> Guid {
        [n; 16]
    }
    fn fixture() -> (Collection, Definitions) {
        let classes = [
            DefinitionClass::RaceVehicleItemData,
            DefinitionClass::RimsItemData,
            DefinitionClass::TiresItemData,
            DefinitionClass::FendersItemData,
        ];
        let catalog = Catalog::new(
            classes
                .iter()
                .enumerate()
                .map(|(i, c)| (guid(i as u8 + 1), *c)),
        )
        .unwrap();
        let mut items = BTreeMap::new();
        for (i, c) in classes.iter().enumerate() {
            let id = i as u64 + 1;
            let size = match c.layout() {
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
                    definition: guid(id as u8),
                    defaults: vec![],
                    children: vec![],
                    state: OWNED,
                    buy_price: 0,
                    sell_price: 0,
                    derived: Derived::decode(c.layout(), &vec![0; size]).unwrap(),
                },
            );
        }
        items.get_mut(&1).unwrap().children = vec![2, 3, 4];
        let entries = [
            (guid(1), Input::Ignore),
            (
                guid(2),
                Input::Part {
                    guids: vec![guid(12), guid(11)],
                    rim: Some([0.02, 0.46, -0.03]),
                },
            ),
            (
                guid(3),
                Input::Tire {
                    rear: false,
                    alternatives: vec![vec![guid(20)], vec![guid(21)]],
                },
            ),
            (
                guid(4),
                Input::Part {
                    guids: vec![],
                    rim: None,
                },
            ),
        ];
        (
            Collection {
                roots: vec![1],
                items,
            },
            Definitions::new(catalog, entries).unwrap(),
        )
    }
    fn component(extra: MeshExtra, variants: &[(u8, Option<&str>)]) -> Component {
        Component {
            kind: MeshKind {
                index_bits: 3,
                extra,
            },
            variants: variants
                .iter()
                .map(|(g, b)| Variant {
                    guid: Some(guid(*g)),
                    bundle: b.map(str::to_owned),
                })
                .collect(),
        }
    }
    fn value(state: State) -> MeshCustomization {
        state.0
    }
    #[test]
    fn variants_precede_input_order_and_matching_consumes_only_one_guid() {
        let (c, d) = fixture();
        let components = [
            component(MeshExtra::Rim, &[(11, Some("b")), (12, Some("a"))]),
            component(MeshExtra::None, &[(11, None), (12, Some("a"))]),
            component(MeshExtra::None, &[(12, None)]),
        ];
        let s = value(
            State::from_vehicle(&c, &d, 1, &components, [0, 0], |n| {
                Some(if n == "b" { 9 } else { 7 })
            })
            .unwrap(),
        );
        assert_eq!(
            s.meshes.iter().map(|m| m.index).collect::<Vec<_>>(),
            [0, 1, -1]
        );
        assert_eq!(s.assets, [9, 7]);
        assert_eq!(
            s.meshes[0].extra,
            MeshValue::Rim {
                first: 20,
                middle: 230,
                last: -30
            }
        );
    }
    #[test]
    fn effective_tire_selection_and_last_installed_override_are_owned() {
        let (mut c, mut d) = fixture();
        let components = [component(
            MeshExtra::None,
            &[(20, None), (21, Some("a")), (11, None)],
        )];
        let run = |c: &Collection, d: &Definitions, tires| {
            value(State::from_vehicle(c, d, 1, &components, tires, |_| Some(71)).unwrap())
        };
        assert_eq!(run(&c, &d, [1, 0]).meshes[0].index, 1);
        assert_eq!(run(&c, &d, [0, 0]).meshes[0].index, 0);
        d.inputs.insert(
            guid(4),
            Input::Part {
                guids: vec![guid(11)],
                rim: None,
            },
        );
        assert_eq!(run(&c, &d, [1, 0]).meshes[0].index, 2);
        c.items.get_mut(&1).unwrap().children = vec![4, 2, 3];
        assert_eq!(run(&c, &d, [1, 0]).meshes[0].index, 1);
    }
    #[test]
    fn level_references_deduplicate_and_remain_isolated_across_sessions() {
        let (c, d) = fixture();
        let components = [
            component(MeshExtra::None, &[(11, Some("parts"))]),
            component(MeshExtra::None, &[(12, Some("parts"))]),
        ];
        assert_eq!(
            State::from_vehicle(&c, &d, 1, &components, [0, 0], |_| None),
            Err(Error::UnknownObject)
        );
        for id in [7, 91, 7] {
            let state = State::from_vehicle(&c, &d, 1, &components, [0, 0], |_| Some(id)).unwrap();
            assert_eq!(state.0.assets, [id]);
            assert_eq!(state.update(false), Update::Mesh(None));
            assert_eq!(state.update(true), Update::Mesh(Some(state.0.clone())));
        }
        assert_eq!(
            State::from_vehicle(&c, &d, 1, &components, [0, 0], |_| Some(0)),
            Err(Error::UnknownObject)
        );
    }
    #[test]
    fn millimetres_truncate_and_preserve_native_low_nine_bits() {
        assert_eq!(
            rim_value([0.0209, 0.4619, -0.0309]).unwrap(),
            MeshValue::Rim {
                first: 20,
                middle: 230,
                last: -30
            }
        );
        assert_eq!(
            rim_value([0.3, 1.2, -0.3]).unwrap(),
            MeshValue::Rim {
                first: -212,
                middle: 88,
                last: 212
            }
        );
        assert_eq!(rim_value([f32::NAN, 1., 0.]), Err(Error::Bound));
    }
    #[test]
    fn invalid_content_ownership_and_unresolved_light_profiles_fail() {
        let (mut c, mut d) = fixture();
        let mut component = component(MeshExtra::Light, &[(11, None)]);
        assert_eq!(
            State::from_vehicle(&c, &d, 1, &[component.clone()], [0, 0], |_| None),
            Err(Error::Unsupported)
        );
        component.kind.extra = MeshExtra::None;
        component.kind.index_bits = 0;
        assert_eq!(validate_components(&[component.clone()]), Err(Error::Bound));
        component.kind.index_bits = 1;
        component.variants.push(Variant {
            guid: None,
            bundle: None,
        });
        assert_eq!(validate_components(&[component.clone()]), Err(Error::Bound));
        component.kind.index_bits = 3;
        c.items.get_mut(&2).unwrap().owner = 99;
        assert_eq!(
            State::from_vehicle(&c, &d, 1, &[component.clone()], [0, 0], |_| None),
            Err(Error::Shape)
        );
        c.items.get_mut(&2).unwrap().owner = 1;
        d.inputs.remove(&guid(2));
        assert_eq!(
            State::from_vehicle(&c, &d, 1, &[component], [0, 0], |_| None),
            Err(Error::UnknownObject)
        );
    }
}
