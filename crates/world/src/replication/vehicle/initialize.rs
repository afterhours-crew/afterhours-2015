// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{
    Body, ChassisInitial, Creation, Customization, Error, Initial, Kind, Profile, Setup,
    authority::Registry,
    components::{Part, Wheel, fresh_rpcs},
    customization, nos,
    orientation::spawn_vector,
    root,
};
use crate::items::Collection;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub position: [f32; 3],
    pub basis: [[f32; 3]; 3],
    pub origin: [f32; 3],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spawn([[Option<u32>; 3]; 2]);
impl Spawn {
    pub fn new(position: [f32; 3], forward: [f32; 3], speed: Option<f32>) -> Result<Self, Error> {
        if position.iter().any(|v| !v.is_finite()) {
            return Err(Error::Shape);
        }
        Ok(Self([
            position.map(|v| (v != 0.0).then(|| v.to_bits())),
            spawn_vector(forward, speed)?,
        ]))
    }
}

pub struct Items<'a> {
    pub collection: &'a Collection,
    pub vehicle: u64,
    pub customization: &'a customization::Definitions,
    pub nos: &'a nos::Definitions,
}

pub struct Lifecycle<'a> {
    pub root: &'a root::State,
    pub pose: Pose,
    pub spawn: &'a Spawn,
    pub setup: Option<Setup>,
    pub authority_group: u64,
    pub authority_components: &'a [u64],
}

pub fn fresh(
    profile: &Profile,
    items: Items<'_>,
    lifecycle: Lifecycle<'_>,
    authority: &mut Registry,
) -> Result<Creation, Error> {
    if profile
        .kinds()
        .iter()
        .filter(|k| matches!(k, Kind::Chassis))
        .count()
        != 1
        || profile
            .kinds()
            .iter()
            .filter(|k| matches!(k, Kind::Index))
            .count()
            != 1
        || profile
            .kinds()
            .iter()
            .filter(|k| matches!(k, Kind::FourBit))
            .count()
            != lifecycle.authority_components.len()
        || lifecycle
            .authority_components
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != lifecycle.authority_components.len()
    {
        return Err(Error::Shape);
    }
    let custom = Customization::from_vehicle(items.collection, items.customization, items.vehicle)?;
    let nos = nos::State::from_vehicle(items.collection, items.nos, items.vehicle)?;
    let rpcs = fresh_rpcs(profile);
    let mut pending = authority.clone();
    let mut components = lifecycle.authority_components.iter();
    let mut fields = Vec::with_capacity(profile.kinds().len());
    for (ordinal, kind) in profile.kinds().iter().enumerate() {
        let rpc = || rpcs[ordinal].clone().ok_or(Error::Shape);
        fields.push(match kind {
            Kind::Root { .. } => Initial::Root(Box::new(lifecycle.root.initial(
                lifecycle.pose.position,
                lifecycle.pose.basis,
                lifecycle.pose.origin,
                rpc()?,
            )?)),
            Kind::Chassis => Initial::Chassis(Box::new(ChassisInitial {
                rpc: rpc()?,
                vectors: lifecycle.spawn.0,
                custom: custom.clone(),
            })),
            Kind::Part { variants } => Part::new(*variants)?.initial(),
            Kind::Wheel => Wheel::default().initial(rpc()?),
            Kind::Index => Initial::Index {
                rpc: rpc()?,
                index: nos.variant,
                value: nos.setting,
            },
            Kind::FourBit => Initial::FourBit(pending.register(
                lifecycle.authority_group,
                *components.next().ok_or(Error::Shape)?,
            )?),
            Kind::Bool | Kind::Mesh(_) => Initial::Empty,
            Kind::TripleNibbles
            | Kind::Tuning
            | Kind::Tagged
            | Kind::NibblesGuid
            | Kind::Appearance
            | Kind::GuidFloat => Initial::Rpc(rpc()?),
        });
    }
    let creation = Creation {
        setup: lifecycle.setup,
        connection_id: lifecycle.root.connection_id(),
        fields,
    };
    Body {
        creation: Some(creation.clone()),
        updates: vec![None; profile.kinds().len()],
    }
    .encode(profile)?;
    *authority = pending;
    Ok(creation)
}

#[cfg(test)]
mod tests {
    use super::super::{Vector, tuning};
    use super::*;
    use crate::items::{Catalog, DefinitionClass, Derived, Item, Layout, OWNED};
    use std::collections::BTreeMap;

    struct Fixture {
        collection: Collection,
        custom: customization::Definitions,
        nos: nos::Definitions,
        root: root::State,
        spawn: Spawn,
    }
    impl Fixture {
        fn new() -> Self {
            let car = Item {
                id: 1,
                owner: 0,
                definition: [1; 16],
                defaults: vec![],
                children: vec![2],
                state: OWNED,
                buy_price: 0,
                sell_price: 0,
                derived: Derived::decode(Layout::RaceVehicle, &[0; 68]).unwrap(),
            };
            let nos = Item {
                id: 2,
                owner: 1,
                definition: [2; 16],
                defaults: vec![],
                children: vec![],
                state: OWNED,
                buy_price: 0,
                sell_price: 0,
                derived: Derived::NosTuning { nos_setting: 9 },
            };
            let catalog = Catalog::new([
                ([1; 16], DefinitionClass::RaceVehicleItemData),
                ([2; 16], DefinitionClass::NosTuningItemData),
            ])
            .unwrap();
            let tuning = tuning::Definitions::new(
                catalog.clone(),
                [(
                    [2; 16],
                    tuning::PartInput {
                        asset_present: false,
                        transmission_values: None,
                    },
                )],
            )
            .unwrap();
            let custom = customization::Definitions::new(
                tuning,
                [
                    ([1; 16], customization::Input::Other),
                    (
                        [2; 16],
                        customization::Input::Performance {
                            value: 0.,
                            group_a: None,
                            clutch_group: None,
                            induction: None,
                        },
                    ),
                ],
            )
            .unwrap();
            let mut root = root::State::new(Some(1)).unwrap();
            root.set_player(Some(7));
            Self {
                collection: Collection {
                    roots: vec![1],
                    items: BTreeMap::from([(1, car), (2, nos)]),
                },
                custom,
                nos: nos::Definitions::new(catalog, [([2; 16], 3)]).unwrap(),
                root,
                spawn: Spawn::new([1., 2., 3.], [0., 0., 1.], None).unwrap(),
            }
        }
        fn items(&self) -> Items<'_> {
            Items {
                collection: &self.collection,
                vehicle: 1,
                customization: &self.custom,
                nos: &self.nos,
            }
        }
        fn lifecycle(&self) -> Lifecycle<'_> {
            Lifecycle {
                root: &self.root,
                pose: Pose {
                    position: [2., 3., 4.],
                    basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                    origin: [1., 1., 1.],
                },
                spawn: &self.spawn,
                setup: None,
                authority_group: 80,
                authority_components: &[90, 91],
            }
        }
    }
    fn profile() -> Profile {
        Profile::new(vec![
            Kind::Root {
                property_owner: true,
            },
            Kind::Chassis,
            Kind::Part { variants: 2 },
            Kind::Wheel,
            Kind::Index,
            Kind::FourBit,
            Kind::FourBit,
            Kind::Bool,
            Kind::Tuning,
        ])
        .unwrap()
    }
    #[test]
    fn composition_keeps_current_pose_separate_from_spawn_and_round_trips() {
        let f = Fixture::new();
        let p = profile();
        let mut a = Registry::default();
        let c = fresh(&p, f.items(), f.lifecycle(), &mut a).unwrap();
        assert_eq!(c.connection_id, 7);
        let Initial::Root(root) = &c.fields[0] else {
            panic!()
        };
        assert_eq!(
            root.position,
            Vector::Packed {
                mode: 2,
                values: [32, 64, 96]
            }
        );
        let Initial::Chassis(chassis) = &c.fields[1] else {
            panic!()
        };
        assert_eq!(
            chassis.vectors[0],
            [
                Some(1f32.to_bits()),
                Some(2f32.to_bits()),
                Some(3f32.to_bits())
            ]
        );
        assert!(matches!(
            c.fields[4],
            Initial::Index {
                index: Some(3),
                value: 9,
                ..
            }
        ));
        assert_eq!(c.fields[5], Initial::FourBit(0));
        assert_eq!(c.fields[6], Initial::FourBit(1));
        let body = Body {
            creation: Some(c.clone()),
            updates: vec![None; p.kinds().len()],
        };
        let wire = body.encode(&p).unwrap();
        assert_eq!(Body::decode(wire.span(), &p, true).unwrap().body, body);
        assert_eq!(fresh(&p, f.items(), f.lifecycle(), &mut a).unwrap(), c);
    }
    #[test]
    fn current_items_and_owner_override_change_rebuild_without_shared_state() {
        let mut f = Fixture::new();
        let p = profile();
        let mut a = Registry::default();
        let before = fresh(&p, f.items(), f.lifecycle(), &mut a).unwrap();
        f.collection.items.get_mut(&2).unwrap().derived = Derived::NosTuning { nos_setting: 4 };
        f.root.set_connection_override(300);
        let after = fresh(&p, f.items(), f.lifecycle(), &mut a).unwrap();
        assert_eq!(after.connection_id, 300);
        assert!(matches!(after.fields[4], Initial::Index { value: 4, .. }));
        assert!(matches!(before.fields[4], Initial::Index { value: 9, .. }));
        f.collection.items.get_mut(&1).unwrap().children.clear();
        let removed = fresh(&p, f.items(), f.lifecycle(), &mut a).unwrap();
        assert!(matches!(
            removed.fields[4],
            Initial::Index {
                index: None,
                value: 0,
                ..
            }
        ));
    }
    #[test]
    fn late_wire_failure_and_mid_registration_exhaustion_roll_back() {
        let f = Fixture::new();
        let p = profile();
        let mut a = Registry::default();
        let mut l = f.lifecycle();
        l.setup = Some(Setup {
            flags: Some(vec![false; 32]),
        });
        assert_eq!(fresh(&p, f.items(), l, &mut a), Err(Error::Bound));
        assert_eq!(a, Registry::default());
        for i in 0..15 {
            a.register(80, 100 + i).unwrap();
        }
        let before = a.clone();
        assert_eq!(
            fresh(&p, f.items(), f.lifecycle(), &mut a),
            Err(Error::Bound)
        );
        assert_eq!(a, before);
        assert_eq!(a.index(80, 90), Err(Error::UnknownObject));
    }
    #[test]
    fn association_shape_owner_and_invalid_pose_fail_without_registration() {
        let mut f = Fixture::new();
        let p = profile();
        let mut a = Registry::default();
        let mut l = f.lifecycle();
        l.authority_components = &[90, 90];
        assert_eq!(fresh(&p, f.items(), l, &mut a), Err(Error::Shape));
        let mut l = f.lifecycle();
        l.pose.position[1] = f32::NAN;
        assert_eq!(fresh(&p, f.items(), l, &mut a), Err(Error::Shape));
        f.collection.items.get_mut(&2).unwrap().owner = 0;
        assert_eq!(
            fresh(&p, f.items(), f.lifecycle(), &mut a),
            Err(Error::Shape)
        );
        assert_eq!(a, Registry::default());
        assert_eq!(
            Spawn::new([0., f32::INFINITY, 0.], [0., 0., 1.], None),
            Err(Error::Shape)
        );
    }
}
