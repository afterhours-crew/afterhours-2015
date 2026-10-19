// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Traffic vehicles use installed components and explicit AI lifecycle inputs.
//! They have no player inventory, customization attachment or player connection.
use nfs_content::vehicle_profiles::traffic::Definition;
use nfs_world::replication::{Error, vehicle::*};

pub mod population;

pub struct Request<'a> {
    pub pose: initialize::Pose,
    pub speed: f32,
    pub authority_group: u64,
    pub authority_components: &'a [u64],
}

pub fn construct(
    definition: &Definition,
    request: Request<'_>,
    authority: &mut authority::Registry,
) -> Result<Body, Error> {
    if !request.speed.is_finite() || request.speed < 0. {
        return Err(Error::Bound);
    }
    let mut root = definition.root.instantiate()?;
    root.set_team(definition.team)?;
    let custom = Customization {
        value: 0,
        list_a: Vec::new(),
        list_b: Vec::new(),
        index: None,
        values: [0; 2],
        flag: false,
        tuning: tuning::Settings::default().values(),
    };
    let spawn = initialize::Spawn::new(
        request.pose.position,
        request.pose.basis[2],
        Some(request.speed),
    )?;
    let mut pending = authority.clone();
    let creation = initialize::with_inputs(
        &definition.profile,
        &custom,
        nos::State::default(),
        initialize::Lifecycle {
            root: &root,
            pose: request.pose,
            spawn: &spawn,
            setup: None,
            authority_group: request.authority_group,
            authority_components: request.authority_components,
        },
        &mut pending,
    )?;
    let mut chassis = chassis::State::new(
        request.pose.position,
        orientation::quaternion_from_basis(request.pose.basis)?,
        definition.mode,
        custom,
    )?;
    let mut motion = chassis.motion().clone();
    motion.velocity = request.pose.basis[2].map(|v| v * request.speed);
    chassis.set_motion(motion, chassis.controls().clone())?;
    let mut updates = Vec::with_capacity(definition.profile.kinds().len());
    for (ordinal, kind) in definition.profile.kinds().iter().enumerate() {
        updates.push(match kind {
            Kind::Root { .. } => Some(root.update(true)?),
            Kind::Chassis => Some(chassis.update(request.pose.origin, 11)?),
            Kind::Part { variants } => components::Part::new(*variants)?
                .update(*definition.parts.get(&ordinal).ok_or(Error::Shape)?, true),
            Kind::Bool => Some(lifecycle::PlayerEntry::default().update(true)),
            Kind::Wheel => Some(components::Wheel::default().update(true)),
            Kind::Index => Some(nos::Runtime::new(1.)?.update(3)?),
            Kind::Rpc | Kind::FourBit => Some(Update::Noop),
            Kind::GuidFloat => Some(definition.health.update(3)?),
            _ => return Err(Error::Unsupported),
        });
    }
    let body = Body {
        creation: Some(creation),
        updates,
    };
    body.encode(&definition.profile)?;
    *authority = pending;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    pub(super) fn definition() -> Definition {
        let (v, _, _) = crate::vehicle_content::tests::fixture();
        let mut row = v["vehicles"][0].clone();
        for key in ["definitions", "wheel_baseline", "appearance", "meshes"] {
            row.as_object_mut().unwrap().remove(key);
        }
        row["profile"] = json!([
            {"kind":"root","property_owner":true},{"kind":"chassis"},
            {"kind":"part","variants":1},{"kind":"bool"},
            {"kind":"wheel"},{"kind":"wheel"},{"kind":"wheel"},{"kind":"wheel"},
            {"kind":"rpc"},{"kind":"index"},{"kind":"four_bit"},{"kind":"guid_float"}
        ]);
        Definition::from_json(&row).unwrap()
    }
    fn request(group: u64, components: &[u64]) -> Request<'_> {
        Request {
            pose: initialize::Pose {
                position: [4., 8., 12.],
                basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                origin: [0.; 3],
            },
            speed: 4.,
            authority_group: group,
            authority_components: components,
        }
    }
    #[test]
    fn traffic_has_no_player_inventory_and_preserves_spawn_motion() {
        let d = definition();
        let mut a = authority::Registry::default();
        let body = construct(&d, request(8, &[9]), &mut a).unwrap();
        let initial = body.creation.as_ref().unwrap();
        assert_eq!(initial.connection_id, 0);
        let Initial::Chassis(c) = &initial.fields[1] else {
            panic!()
        };
        assert_eq!(c.custom.tuning, [5; 32]);
        assert!(c.custom.list_a.is_empty() && c.custom.list_b.is_empty());
        let Initial::Root(root) = &initial.fields[0] else {
            panic!()
        };
        assert!(!root.flag && !root.fine_flag);
        assert!(root.reference.is_none());
        let Some(Update::Chassis(chassis)) = &body.updates[1] else {
            panic!()
        };
        let motion = chassis.physics.as_ref().unwrap().decode([0.; 3]).unwrap();
        assert_eq!(motion.position, [4., 8., 12.]);
        assert_eq!(motion.velocity, [0., 0., 4.]);
        assert_eq!(chassis.physics.as_ref().unwrap().controls.value3, d.mode);
        assert_eq!(a.index(8, 9), Ok(0));
        let wire = body.encode(&d.profile).unwrap();
        assert_eq!(
            Body::decode(wire.span(), &d.profile, true).unwrap().body,
            body
        );
        let other = construct(&d, request(10, &[11]), &mut a).unwrap();
        assert_eq!(other, body);
        assert_eq!(a.index(10, 11), Ok(0));
    }
    #[test]
    fn late_content_failure_and_invalid_motion_do_not_consume_authority() {
        let mut d = definition();
        let mut a = authority::Registry::default();
        a.register(1, 2).unwrap();
        let before = a.clone();
        d.parts.clear();
        assert_eq!(construct(&d, request(8, &[9]), &mut a), Err(Error::Shape));
        assert_eq!(a, before);
        let mut r = request(8, &[9]);
        r.speed = f32::NAN;
        assert_eq!(construct(&definition(), r, &mut a), Err(Error::Bound));
        assert_eq!(a, before);
    }
}
