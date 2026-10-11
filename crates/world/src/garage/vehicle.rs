// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{bits::BitWriter, participants::Endpoint, replication::Error};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    pub endpoint: Endpoint,
    pub participant: u16,
    pub vehicle: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Loaded {
    pub scene: u16,
    pub selector: u16,
    pub participant: u16,
    pub vehicle: u16,
}
fn validate(scene: u16, selector: u16, participant: u16, vehicle: u16) -> Result<(), Error> {
    if [scene, participant, vehicle]
        .into_iter()
        .any(|id| id == 0 || id > 8191)
        || selector > 511
    {
        return Err(Error::Bound);
    }
    Ok(())
}
fn envelope(scene: u16, participant: u16, vehicle: u16, payload: BitWriter) -> BitWriter {
    let mut body = BitWriter::new();
    body.put(0, 32)
        .put(0, 32)
        .put(3, 8)
        .put(scene.into(), 13)
        .put(participant.into(), 13)
        .put(vehicle.into(), 13)
        .put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}
impl Binding {
    pub fn encode(self) -> Result<BitWriter, Error> {
        validate(
            self.endpoint.scene,
            self.endpoint.selector,
            self.participant,
            self.vehicle,
        )?;
        let mut p = BitWriter::new();
        p.put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(0, 32)
            .align();
        Ok(envelope(
            self.endpoint.scene,
            self.participant,
            self.vehicle,
            p,
        ))
    }
    pub fn decode(body: BitSpan<'_>) -> Result<Self, Error> {
        let e = decode(body, 176, 7)?;
        let r = e
            .route(RouteProfile::ClientReceive)
            .map_err(|_| Error::Shape)?;
        if r.method_index() != 0 || r.arguments().len() != 5 {
            return Err(Error::Shape);
        }
        let b = Self {
            endpoint: Endpoint {
                scene: e.references()[0],
                selector: r.selector(),
                serial: r.serial().ok_or(Error::Shape)?,
            },
            participant: e.references()[1],
            vehicle: e.references()[2],
        };
        b.encode()?;
        Ok(b)
    }
    pub fn expected(self) -> Loaded {
        Loaded {
            scene: self.endpoint.scene,
            selector: self.endpoint.selector,
            participant: self.participant,
            vehicle: self.vehicle,
        }
    }
}
fn decode(body: BitSpan<'_>, bits: usize, bytes: usize) -> Result<Envelope<'_>, Error> {
    let e = Envelope::decode(
        body,
        Limits {
            max_input_bits: bits,
            max_references: 3,
            max_payload_bytes: bytes,
        },
    )
    .map_err(|_| Error::Shape)?;
    if e.words() != [0, 0] || e.references().len() != 3 || !e.remaining().is_empty() {
        return Err(Error::Shape);
    }
    Ok(e)
}
impl Loaded {
    pub fn encode(self) -> Result<BitWriter, Error> {
        validate(self.scene, self.selector, self.participant, self.vehicle)?;
        let mut p = BitWriter::new();
        p.put(self.selector.into(), 9).put(1, 32).align();
        Ok(envelope(self.scene, self.participant, self.vehicle, p))
    }
    pub fn decode(body: BitSpan<'_>) -> Result<Self, Error> {
        let e = decode(body, 168, 6)?;
        let r = e
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if r.method_index() != 1 || r.arguments().len() != 7 {
            return Err(Error::Shape);
        }
        let a = Self {
            scene: e.references()[0],
            selector: r.selector(),
            participant: e.references()[1],
            vehicle: e.references()[2],
        };
        a.encode()?;
        Ok(a)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pending {
    entries: BTreeMap<(u16, u16, u16), (Binding, bool)>,
}
impl Pending {
    pub fn has_endpoint(&self, scene: u16, selector: u16) -> bool {
        self.entries
            .keys()
            .any(|&(s, r, _)| (s, r) == (scene, selector))
    }
    pub fn offer(&mut self, b: Binding) -> Result<Option<Binding>, Error> {
        b.encode()?;
        let key = (b.endpoint.scene, b.endpoint.selector, b.participant);
        if let Some((old, _)) = self.entries.get(&key) {
            return if *old == b {
                Ok(None)
            } else {
                Err(Error::DuplicateObject)
            };
        }
        if self
            .entries
            .values()
            .any(|(old, _)| old.participant == b.participant && old.vehicle == b.vehicle)
        {
            return Err(Error::DuplicateObject);
        }
        let participants = self
            .entries
            .keys()
            .map(|k| k.2)
            .collect::<std::collections::BTreeSet<_>>();
        if !participants.contains(&b.participant) && participants.len() >= 128 {
            return Err(Error::Bound);
        }
        if self.entries.len() >= 640
            || self.entries.keys().filter(|k| k.2 == b.participant).count() >= 5
        {
            return Err(Error::Bound);
        }
        self.entries.insert(key, (b, false));
        Ok(Some(b))
    }
    pub fn acknowledge(
        &mut self,
        a: Loaded,
        owns: impl FnOnce(u16) -> bool,
    ) -> Result<bool, Error> {
        a.encode()?;
        if !owns(a.participant) {
            return Err(Error::UnknownObject);
        }
        let (b, done) = self
            .entries
            .get_mut(&(a.scene, a.selector, a.participant))
            .ok_or(Error::UnknownObject)?;
        if b.expected() != a {
            return Err(Error::Shape);
        }
        let changed = !*done;
        *done = true;
        Ok(changed)
    }
    pub fn all_loaded(&self, participant: u16, occupied: usize) -> bool {
        let rows = self
            .entries
            .values()
            .filter(|(b, _)| b.participant == participant)
            .collect::<Vec<_>>();
        occupied <= 5 && rows.len() == occupied && rows.iter().all(|(_, done)| *done)
    }
}

#[cfg(test)]
mod tests;
