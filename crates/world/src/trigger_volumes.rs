// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned observations from enabled client trigger volumes. Crossing callbacks
//! are local to the client; accepting a report does not synthesize a finish event.
use crate::replication::{Error, Initial, Record, Update, sublevel};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use std::collections::BTreeMap;

pub const MAX_ENDPOINTS: usize = 4096;
pub const MAX_OBSERVATIONS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Report {
    pub scene: u16,
    pub selector: u16,
    pub vehicle: u16,
    pub entered: bool,
    pub seconds: f64,
    pub changed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Observation {
    seconds: f64,
    entered: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Volumes {
    endpoints: BTreeMap<(u16, u16), bool>,
    observations: BTreeMap<(u16, u16, u16), Observation>,
}
impl Volumes {
    /// Bind only serializers explicitly classified as trigger volumes. Plain
    /// Boolean endpoints retain their own behavior and are never activated here.
    pub fn bind(&mut self, records: &[Record]) -> Result<(), Error> {
        let mut next = self.clone();
        for record in records {
            let (Some(Initial::SubLevel { fields, .. }), Update::SubLevel { profile, .. }) =
                (&record.initial, &record.update)
            else {
                continue;
            };
            for (index, kind) in profile.kinds().iter().enumerate() {
                if *kind != sublevel::Kind::RpcTriggerVolume {
                    continue;
                }
                let Some(sublevel::Initial::RpcBool { rpc, value }) = fields.get(index) else {
                    return Err(Error::TypeMismatch);
                };
                if record.id == 0 || record.id > 8191 || rpc.selector > 511 {
                    return Err(Error::Bound);
                }
                if next
                    .endpoints
                    .insert((record.id, rpc.selector), *value)
                    .is_some()
                {
                    return Err(Error::DuplicateObject);
                }
                if next.endpoints.len() > MAX_ENDPOINTS {
                    return Err(Error::Bound);
                }
            }
        }
        *self = next;
        Ok(())
    }
    /// Forget observations for removed cars or disconnected owners.
    pub fn retain_vehicles(&mut self, owns: impl Fn(u16) -> bool) {
        self.observations.retain(|(_, _, car), _| owns(*car));
    }
    /// Client methods 2/3 contain the crossing clock followed by seven padding
    /// bits. Padding is not required to be zero. Old/repeated reports are inert.
    pub fn receive(
        &mut self,
        body: BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<Report>, Error> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        let Some(scene) = envelope.references().first().copied() else {
            return Ok(None);
        };
        let Some(enabled) = self.endpoints.get(&(scene, route.selector())) else {
            return Ok(None);
        };
        if !matches!(route.method_index(), 2 | 3) {
            return Ok(None);
        }
        let &[_, vehicle] = envelope.references() else {
            return Err(Error::Shape);
        };
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || route.arguments().len() != 71
        {
            return Err(Error::Shape);
        }
        if !owns(vehicle) {
            return Err(Error::UnknownObject);
        }
        if !enabled {
            return Err(Error::Shape);
        }
        let high = route
            .arguments()
            .read_u32(0, 32)
            .map_err(|_| Error::Shape)?;
        let low = route
            .arguments()
            .read_u32(32, 32)
            .map_err(|_| Error::Shape)?;
        let seconds = f64::from_bits((u64::from(high) << 32) | u64::from(low));
        if !seconds.is_finite() || seconds < 0. {
            return Err(Error::Shape);
        }
        let key = (scene, route.selector(), vehicle);
        let entered = route.method_index() == 2;
        let old = self.observations.get(&key);
        let fresh = old.is_none_or(|old| seconds > old.seconds);
        let changed = fresh && old.map_or(entered, |old| old.entered != entered);
        if fresh {
            if old.is_none() && self.observations.len() >= MAX_OBSERVATIONS {
                return Err(Error::Bound);
            }
            self.observations
                .insert(key, Observation { seconds, entered });
        }
        Ok(Some(Report {
            scene,
            selector: route.selector(),
            vehicle,
            entered,
            seconds,
            changed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::replication::{
        state::World,
        sublevel::{Content, Kind, Profile, ordinary},
    };
    use nfs_protocol::world::rpc::Serial;

    fn bound() -> (Volumes, Record) {
        let content = Content::new(vec![(
            77,
            Profile::new(
                false,
                &[Kind::Noop, Kind::RpcBool, Kind::RpcTriggerVolume, Kind::Rpc],
            )
            .unwrap(),
        )])
        .unwrap();
        let values = ordinary::unpopulated(content.profile(77).unwrap()).unwrap();
        assert_eq!(values[1], ordinary::Value::Boolean(false));
        assert_eq!(values[2], ordinary::Value::Boolean(true));
        let creation =
            ordinary::creation(5, 77, None, Serial::new(9).unwrap(), &values, &content).unwrap();
        let wire = creation.encode(&content).unwrap();
        assert_eq!(
            sublevel::Creation::decode(wire.span(), &content)
                .unwrap()
                .creation,
            creation
        );
        for end in 0..wire.len() {
            assert!(
                sublevel::Creation::decode(wire.span().slice(0, end).unwrap(), &content).is_err()
            );
        }
        let mut world = World::default();
        world.register_levels(&[5]).unwrap();
        let record = world.spawn_scene(creation, &content).unwrap();
        let mut volumes = Volumes::default();
        volumes.bind(std::slice::from_ref(&record)).unwrap();
        (volumes, record)
    }
    fn call(scene: u16, car: u16, selector: u16, method: u32, seconds: f64) -> BitWriter {
        let mut payload = BitWriter::new();
        payload
            .put(selector.into(), 9)
            .put(method.into(), 32)
            .put(seconds.to_bits(), 64)
            .put(93, 7);
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(scene.into(), 13)
            .put(car.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body
    }
    #[test]
    fn active_volume_reports_are_owned_ordered_and_isolated() {
        let (mut v, r) = bound();
        let scene = r.id;
        assert!(
            v.receive(call(scene, 31, 0, 2, 1.).span(), |_| true)
                .unwrap()
                .is_none()
        );
        let enter = call(scene, 31, 1, 2, 1.);
        let before = v.clone();
        assert_eq!(
            v.receive(enter.span(), |_| false),
            Err(Error::UnknownObject)
        );
        assert_eq!(v, before);
        assert!(
            v.receive(enter.span(), |car| car == 31)
                .unwrap()
                .unwrap()
                .changed
        );
        assert!(!v.receive(enter.span(), |_| true).unwrap().unwrap().changed);
        assert!(
            v.receive(call(scene, 32, 1, 2, 1.).span(), |_| true)
                .unwrap()
                .unwrap()
                .changed
        );
        assert!(
            v.receive(call(scene, 31, 1, 3, 2.).span(), |_| true)
                .unwrap()
                .unwrap()
                .changed
        );
        assert!(!v.receive(enter.span(), |_| true).unwrap().unwrap().changed);
        assert!(v.observations[&(scene, 1, 32)].entered);
        assert!(!v.observations[&(scene, 1, 31)].entered);
        v.retain_vehicles(|car| car == 32);
        assert_eq!(v.observations.len(), 1);
        let before = v.clone();
        assert!(v.bind(&[r]).is_err());
        assert_eq!(v, before);
    }
    #[test]
    fn malformed_clocks_lengths_and_capacity_do_not_mutate() {
        let (mut v, r) = bound();
        let valid = call(r.id, 31, 1, 2, 1.);
        for seconds in [f64::NAN, f64::INFINITY, -1.] {
            assert!(
                v.receive(call(r.id, 31, 1, 2, seconds).span(), |_| true)
                    .is_err()
            );
        }
        for end in 0..valid.len() {
            assert!(
                v.receive(valid.span().slice(0, end).unwrap(), |_| true)
                    .is_err()
            );
        }
        assert!(v.observations.is_empty());
        for car in 1..=MAX_OBSERVATIONS as u16 {
            v.receive(call(r.id, car, 1, 2, 1.).span(), |_| true)
                .unwrap();
        }
        let before = v.clone();
        assert_eq!(
            v.receive(call(r.id, 5000, 1, 2, 1.).span(), |_| true),
            Err(Error::Bound)
        );
        assert_eq!(v, before);
        assert!(
            v.receive(call(r.id, 1, 1, 3, 2.).span(), |_| true)
                .unwrap()
                .unwrap()
                .changed
        );
    }
}
