// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded session-owned traffic. Placement is host policy over installed roads;
//! vehicle/manager/goal wire shapes are independent of that policy.
use nfs_content::traffic::Content;
use nfs_world::{
    content::bindings::{Bindings, name_key},
    participants::HostRpc,
    replication::{
        Error, Record,
        entity::creation::{Asset, Catalog, Content as EntityContent, Parent, Prefix},
        players::{Players, ai},
        traffic, vehicle,
    },
};
use std::collections::BTreeMap;

#[cfg(test)]
pub(crate) mod tests;

pub const MAX_CARS: usize = 8;
const SPAWN_INTERVAL_MS: u64 = 1000;
// The native population uses 300/450 metre eligibility/retention distances.
// Its spawn region also depends on player motion and orientation, and its
// retirement gate checks total age. This host policy conservatively excludes
// a circle and requires two continuous seconds outside every retention region;
// it does not reproduce the entire native population policy.
const SPAWN_DISTANCE: f32 = 300.;
const RETAIN_DISTANCE: f32 = 450.;
const RETIRE_GRACE_MS: u64 = 2000;

#[derive(Clone, Debug)]
struct Car {
    anchor: [f32; 3],
    outside_since: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Population {
    bindings: Bindings,
    authority: vehicle::authority::Registry,
    cars: BTreeMap<u64, Car>,
    next: u64,
    due: u64,
    clock: u64,
    owner: Option<(u8, u64)>,
}

#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub records: Vec<Record>,
    pub calls: Vec<HostRpc>,
    pub deleted: Vec<u16>,
    pub spawned: Option<ai::Ids>,
}

#[derive(Clone, Debug)]
struct Placement {
    spline: u16,
    persona: usize,
    pose: vehicle::initialize::Pose,
}

impl Population {
    pub fn new(bindings: Bindings, content: &Content) -> Result<Self, Error> {
        let gameplay = bindings
            .root_child(content.managers.scene)
            .map_err(|_| Error::Shape)?;
        if gameplay.id >= 2048 {
            return Err(Error::Bound);
        }
        for definition in content.vehicles.values() {
            let key = name_key(definition.bundle.as_bytes()).map_err(|_| Error::Shape)?;
            let bundle = bindings
                .root_child(key)
                .map_err(|_| Error::Shape)?
                .bundle_id;
            asset_bundle(bundle)?;
        }
        Ok(Self {
            bindings,
            authority: vehicle::authority::Registry::default(),
            cars: BTreeMap::new(),
            next: 1,
            due: 0,
            clock: 0,
            owner: None,
        })
    }

    pub fn count(&self) -> usize {
        self.cars.len()
    }

    pub fn delegated(&self, world: &Players, connection: u8, persona: u64) -> BTreeMap<u16, u64> {
        if self.owner != Some((connection, persona)) {
            return BTreeMap::new();
        }
        self.cars
            .keys()
            .filter_map(|&key| {
                world
                    .ai_ids(connection, persona, key)
                    .map(|ids| (ids.vehicle, key))
            })
            .collect()
    }

    /// The caller commits this transaction only after reserving its outbound
    /// sections. Regressing time, incomplete content and late manager errors
    /// preserve both the world namespace and population counters.
    pub fn advance(
        &mut self,
        players: &mut Players,
        connection: u8,
        persona: u64,
        content: &Content,
        now: u64,
        focus: &[[f32; 3]],
    ) -> Result<Changes, Error> {
        if now < self.clock || focus.len() > 128 || focus.iter().flatten().any(|v| !v.is_finite()) {
            return Err(Error::Bound);
        }
        if self
            .owner
            .is_some_and(|owner| owner != (connection, persona))
        {
            return Err(Error::UnknownObject);
        }
        let mut next = self.clone();
        let mut world = players.clone();
        let mut changes = Changes::default();
        for (&key, car) in &mut next.cars {
            car.anchor = world
                .ai_vehicle_physics(connection, persona, key)?
                .decode([0.; 3])?
                .position;
            if focus
                .iter()
                .any(|p| horizontal_distance_squared(*p, car.anchor) <= RETAIN_DISTANCE.powi(2))
            {
                car.outside_since = None;
            } else {
                car.outside_since.get_or_insert(now);
            }
        }
        let retire: Vec<_> = next
            .cars
            .iter()
            .filter(|(_, car)| {
                focus.is_empty()
                    || car
                        .outside_since
                        .is_some_and(|since| now.saturating_sub(since) >= RETIRE_GRACE_MS)
            })
            .map(|(&key, _)| key)
            .collect();
        for key in retire {
            let removed = world.remove_ai_vehicle(connection, persona, key)?;
            changes.records.extend(removed.records);
            changes.deleted.extend(removed.deleted);
            next.cars.remove(&key);
            next.authority.remove_group(key);
        }
        if !focus.is_empty() && next.cars.len() < MAX_CARS && now >= next.due {
            if let Some(placement) = next.select(content, focus) {
                next.spawn(
                    &mut world,
                    connection,
                    persona,
                    content,
                    placement,
                    &mut changes,
                )?;
            }
            next.due = now.saturating_add(SPAWN_INTERVAL_MS);
        }
        next.clock = now;
        next.owner = Some((connection, persona));
        *self = next;
        *players = world;
        Ok(changes)
    }

    fn select(&self, content: &Content, focus: &[[f32; 3]]) -> Option<Placement> {
        if content.density <= 0. {
            return None;
        }
        let mut best: Option<(f32, Placement)> = None;
        for spline in &content.splines {
            if spline.density == 0 {
                continue;
            }
            // This is an explicit host selection policy, not a claim about the
            // stock server's random distribution or tag predicate.
            let compatible: Vec<_> = content
                .personas
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    spline.tags.is_empty() || p.tags.iter().any(|tag| spline.tags.contains(tag))
                })
                .map(|(index, _)| index)
                .collect();
            if compatible.is_empty() {
                continue;
            }
            let persona = compatible[((self.next - 1) % compatible.len() as u64) as usize];
            for segment in &spline.segments {
                for (point_index, pair) in segment.points.windows(2).enumerate() {
                    let position = pair[0];
                    // Exclude the close region of every active player, even
                    // when a different floor makes them ineligible as a focus.
                    if focus.iter().any(|p| {
                        horizontal_distance_squared(*p, position) <= SPAWN_DISTANCE.powi(2)
                    }) {
                        continue;
                    }
                    let distance = focus
                        .iter()
                        .filter(|p| (p[1] - position[1]).abs() <= 8.)
                        .map(|p| horizontal_distance_squared(*p, position))
                        .min_by(f32::total_cmp);
                    let Some(distance) = distance else {
                        continue;
                    };
                    if distance >= RETAIN_DISTANCE.powi(2)
                        || self
                            .cars
                            .values()
                            .any(|car| distance_squared(car.anchor, position) < 30. * 30.)
                    {
                        continue;
                    }
                    if best.as_ref().is_some_and(|(d, _)| *d <= distance) {
                        continue;
                    }
                    let Some((_, tangent)) = segment
                        .curve
                        .distance_at_point(point_index)
                        .and_then(|d| segment.curve.sample(d))
                    else {
                        continue;
                    };
                    let Some(basis) = road_basis([0.; 3], tangent) else {
                        continue;
                    };
                    best = Some((
                        distance,
                        Placement {
                            spline: spline.index,
                            persona,
                            pose: vehicle::initialize::Pose {
                                position,
                                basis,
                                origin: [0.; 3],
                            },
                        },
                    ));
                }
            }
        }
        best.map(|(_, p)| p)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &mut self,
        world: &mut Players,
        connection: u8,
        persona: u64,
        content: &Content,
        placement: Placement,
        changes: &mut Changes,
    ) -> Result<(), Error> {
        let selected = content
            .personas
            .get(placement.persona)
            .ok_or(Error::Shape)?;
        let definition = content
            .vehicles
            .get(&selected.vehicle)
            .ok_or(Error::Shape)?;
        let content_key = name_key(definition.bundle.as_bytes()).map_err(|_| Error::Shape)?;
        let registration = self
            .bindings
            .root_child(content_key)
            .map_err(|_| Error::Shape)?;
        let parent = world
            .objects()
            .scene(content_key)
            .ok_or(Error::UnknownObject)?;
        let blueprint = world
            .objects()
            .scene(content.managers.scene)
            .ok_or(Error::UnknownObject)?;
        let key = self.next;
        let components = definition
            .profile
            .kinds()
            .iter()
            .enumerate()
            .filter(|(_, kind)| matches!(kind, vehicle::Kind::FourBit))
            .map(|(i, _)| {
                key.checked_mul(512)
                    .and_then(|v| v.checked_add(i as u64 + 1))
                    .ok_or(Error::Bound)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let body = super::construct(
            definition,
            super::Request {
                pose: placement.pose,
                speed: 0.,
                authority_group: key,
                authority_components: &components,
            },
            &mut self.authority,
        )?;
        let asset = Asset {
            bundle: asset_bundle(registration.bundle_id)?,
            type_id: definition.asset_type,
            local_index: definition.asset_index,
        };
        let entity_content = EntityContent::new(
            vec![Catalog::new(asset.bundle, &definition.catalog)?],
            vec![],
        )?
        .with_vehicles(vec![(asset, definition.profile.clone())])?;
        let spawned = world
            .spawn_ai_vehicle(
                connection,
                persona,
                ai::Request {
                    key,
                    name: selected.name.as_bytes().to_vec(),
                    flags: [true, false],
                    words: [selected.index, u16::MAX],
                    mode: 0,
                },
                vehicle::EntityCreation {
                    prefix: Prefix {
                        parent: Some(Parent {
                            reference: Some(parent),
                        }),
                        blueprint,
                        sub_id: 2,
                        owner: None,
                        asset,
                    },
                    body,
                },
                &entity_content,
            )?
            .ok_or(Error::DuplicateObject)?;
        changes.records.extend(spawned.records);
        let bound = world.bind_ai_managers(connection, persona, key, content.managers)?;
        changes.calls.extend(bound.add.map(HostRpc::Participant));
        changes.calls.extend(bound.bind.map(HostRpc::Actor));
        let level = self
            .bindings
            .root_child(content.managers.scene)
            .map_err(|_| Error::Shape)?;
        let (goal, assigned) = world.start_ai_traffic_goal(
            connection,
            persona,
            key,
            traffic::Initial {
                asset: Asset {
                    bundle: level.id,
                    type_id: content.spline_class,
                    local_index: placement.spline,
                },
                catalog: Catalog::new(
                    level.id,
                    &[(content.spline_class, content.splines.len() as u16)],
                )?,
                allocation_serial: 0,
                asset_mode: 2,
            },
        )?;
        changes.records.extend([goal, assigned]);
        changes.spawned = Some(spawned.ids);
        self.next = key.checked_add(1).ok_or(Error::Bound)?;
        self.cars.insert(
            key,
            Car {
                anchor: placement.pose.position,
                outside_since: None,
            },
        );
        Ok(())
    }
}

fn asset_bundle(stream: u16) -> Result<u16, Error> {
    let asset = stream.checked_sub(3).ok_or(Error::Shape)?;
    if asset >= 2048 || asset == 2003 {
        return Err(Error::Bound);
    }
    Ok(asset)
}
pub(super) fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}
fn horizontal_distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)
}
pub(super) fn road_basis(a: [f32; 3], b: [f32; 3]) -> Option<[[f32; 3]; 3]> {
    let mut forward = std::array::from_fn::<_, 3, _>(|i| b[i] - a[i]);
    let length = forward.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !length.is_finite() || length < 0.01 {
        return None;
    }
    forward = forward.map(|v| v / length);
    let horizontal = (forward[0].powi(2) + forward[2].powi(2)).sqrt();
    if horizontal < 0.1 {
        return None;
    }
    let right = [forward[2] / horizontal, 0., -forward[0] / horizontal];
    let up = [
        forward[1] * right[2],
        forward[2] * right[0] - forward[0] * right[2],
        -forward[1] * right[0],
    ];
    Some([right, up, forward])
}
