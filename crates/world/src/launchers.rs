// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    bits::BitWriter,
    replication::{self, Initial, Record, sublevel},
};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile, Serial},
};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_ENDPOINTS: usize = 32768;
pub const MAX_ENABLED: usize = 32768;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Catalog(BTreeMap<u32, Vec<usize>>);
impl Catalog {
    pub fn new(
        entries: Vec<(u32, Vec<usize>)>,
        content: &sublevel::Content,
    ) -> Result<Self, replication::Error> {
        if entries.len() > sublevel::MAX_PROFILES {
            return Err(replication::Error::Bound);
        }
        let mut result = BTreeMap::new();
        let mut count = 0;
        for (key, indices) in entries {
            let profile = content.profile(key)?;
            count += indices.len();
            if count > MAX_ENDPOINTS {
                return Err(replication::Error::Bound);
            }
            let mut unique = BTreeSet::new();
            for &index in &indices {
                if !matches!(
                    profile.kinds().get(index),
                    Some(sublevel::Kind::Rpc | sublevel::Kind::RpcPursuitMaps)
                ) || !unique.insert(index)
                {
                    return Err(replication::Error::Shape);
                }
            }
            if result.insert(key, indices).is_some() {
                return Err(replication::Error::DuplicateObject);
            }
        }
        Ok(Self(result))
    }
    pub fn entries(&self) -> impl Iterator<Item = (u32, &[usize])> {
        self.0
            .iter()
            .map(|(key, indices)| (*key, indices.as_slice()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Enabled {
    pub scene: u16,
    pub selector: u16,
    pub serial: Serial,
}
impl Enabled {
    pub fn encode(self) -> Result<BitWriter, replication::Error> {
        if self.scene == 0 || self.scene > 8191 || self.selector > 511 {
            return Err(replication::Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload
            .put(self.selector.into(), 9)
            .put(self.serial.value().into(), 10)
            .put(0, 32);
        payload.align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(1, 8)
            .put(self.scene.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Launchers {
    endpoints: BTreeMap<(u16, u16), Serial>,
    entity_players: BTreeMap<(u16, u16), u16>,
    enabled: BTreeSet<(u16, u16, u16)>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Unsupported,
    Repeated(Enabled),
    Enabled(Enabled),
}
impl Launchers {
    pub fn bind_entity(
        &mut self,
        record: &Record,
        indices: &[usize],
        player: u16,
    ) -> Result<(), replication::Error> {
        use crate::replication::entity;
        if player == 0
            || player > 8191
            || record.id == 0
            || record.id > 8191
            || indices.len() > entity::MAX_SERIALIZERS
        {
            return Err(replication::Error::Bound);
        }
        let Some(Initial::Entity { fields, .. }) = &record.initial else {
            return Err(replication::Error::TypeMismatch);
        };
        let mut next = self.clone();
        for &index in indices {
            let Some(entity::Initial::Rpc(rpc)) = fields.get(index) else {
                return Err(replication::Error::TypeMismatch);
            };
            if rpc.selector > 511 {
                return Err(replication::Error::Bound);
            }
            if next.endpoints.len() >= MAX_ENDPOINTS {
                return Err(replication::Error::Bound);
            }
            if next
                .endpoints
                .insert((record.id, rpc.selector), rpc.serial)
                .is_some()
            {
                return Err(replication::Error::DuplicateObject);
            }
            next.entity_players
                .insert((record.id, rpc.selector), player);
        }
        *self = next;
        Ok(())
    }
    pub fn bind(
        &mut self,
        records: &[Record],
        catalog: &Catalog,
    ) -> Result<(), replication::Error> {
        let mut next = self.endpoints.clone();
        for record in records {
            let Some(Initial::SubLevel { prefix, fields }) = &record.initial else {
                continue;
            };
            if let Some(indices) = catalog.0.get(&prefix.content_key) {
                for &index in indices {
                    let Some(sublevel::Initial::Rpc(rpc)) = fields.get(index) else {
                        return Err(replication::Error::TypeMismatch);
                    };
                    if next.len() >= MAX_ENDPOINTS {
                        return Err(replication::Error::Bound);
                    }
                    if next.insert((record.id, rpc.selector), rpc.serial).is_some() {
                        return Err(replication::Error::DuplicateObject);
                    }
                }
            }
        }
        self.endpoints = next;
        Ok(())
    }
    pub fn enabled_count(&self) -> usize {
        self.enabled.len()
    }
    pub fn receive(
        &mut self,
        body: BitSpan<'_>,
        owns_player: impl Fn(u16) -> bool,
    ) -> Result<Outcome, replication::Error> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 8192,
                max_references: 255,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| replication::Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| replication::Error::Shape)?;
        let Some(&scene) = envelope.references().first() else {
            return Err(replication::Error::Shape);
        };
        let Some(&serial) = self.endpoints.get(&(scene, route.selector())) else {
            return Ok(Outcome::Unsupported);
        };
        if route.method_index() != 2 {
            return Ok(Outcome::Unsupported);
        }
        if envelope.words() != [0, 0]
            || envelope.references().len() != 2
            || !envelope.remaining().is_empty()
            || route.arguments().len() != 7
        {
            return Err(replication::Error::Shape);
        }
        let player = envelope.references()[1];
        if player == 0 || !owns_player(player) {
            return Err(replication::Error::UnknownObject);
        }
        if self
            .entity_players
            .get(&(scene, route.selector()))
            .is_some_and(|owner| *owner != player)
        {
            return Err(replication::Error::UnknownObject);
        }
        let key = (scene, route.selector(), player);
        let reply = Enabled {
            scene,
            selector: route.selector(),
            serial,
        };
        if self.enabled.contains(&key) {
            return Ok(Outcome::Repeated(reply));
        }
        if self.enabled.len() >= MAX_ENABLED {
            return Err(replication::Error::Bound);
        }
        self.enabled.insert(key);
        Ok(Outcome::Enabled(reply))
    }
}

#[cfg(test)]
mod tests;
