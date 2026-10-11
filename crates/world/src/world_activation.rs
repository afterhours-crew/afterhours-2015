// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant subscriptions to common world volumes. Eligibility comes from
//! deployment content and owned lifecycle state, never from recorded messages.
use crate::{
    bits::BitWriter,
    participants::Endpoint,
    replication::{Error, Initial, Record, sublevel},
};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_VOLUMES: usize = 512;
pub const MAX_PARTICIPANTS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Site {
    pub scene_key: u32,
    pub serializer: usize,
}

/// Only unconditional world subscriptions belong here. Progression-gated race
/// volumes need their own eligibility model and must not be added wholesale.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalog {
    pub volumes: Vec<Site>,
    pub presence: Site,
    pub streamed: Site,
    pub location: Site,
}
impl Catalog {
    pub fn validate_profiles(&self, content: &sublevel::Content) -> Result<(), Error> {
        self.validate()?;
        for (site, kind) in self
            .volumes
            .iter()
            .map(|site| (*site, sublevel::Kind::Rpc))
            .chain([
                (self.presence, sublevel::Kind::RpcBool),
                (self.streamed, sublevel::Kind::Rpc),
                (self.location, sublevel::Kind::RpcFourReferences),
            ])
        {
            if content
                .profile(site.scene_key)?
                .kinds()
                .get(site.serializer)
                != Some(&kind)
            {
                return Err(Error::TypeMismatch);
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.volumes.len() > MAX_VOLUMES {
            return Err(Error::Bound);
        }
        let mut sites = BTreeSet::new();
        for site in self
            .volumes
            .iter()
            .chain([&self.presence, &self.streamed, &self.location])
        {
            if site.scene_key <= 1
                || site.serializer == 0
                || site.serializer >= sublevel::MAX_SERIALIZERS
            {
                return Err(Error::Bound);
            }
            if !sites.insert(*site) {
                return Err(Error::DuplicateObject);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Call {
    Subscribe { token: u32, inside: bool },
    Unsubscribe,
    StreamedParticipant,
    Presence(bool),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Notification {
    pub endpoint: Endpoint,
    /// Absent only for the scene-wide presence notification.
    pub participant: Option<u16>,
    pub call: Call,
}
impl Notification {
    pub fn encode(self) -> Result<BitWriter, Error> {
        if self.endpoint.scene == 0
            || self.endpoint.scene > 8191
            || self.endpoint.selector > 511
            || self.participant.is_some_and(|id| id == 0 || id > 8191)
        {
            return Err(Error::Bound);
        }
        if matches!(self.call, Call::Presence(_)) != self.participant.is_none() {
            return Err(Error::Shape);
        }
        let mut payload = BitWriter::new();
        let method = match self.call {
            Call::Subscribe { .. } | Call::Presence(true) => 0,
            Call::Unsubscribe | Call::Presence(false) => 1,
            Call::StreamedParticipant => 3,
        };
        payload
            .put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(method, 32);
        if let Call::Subscribe { token, inside } = self.call {
            payload.put(token.into(), 32).put_bool(inside);
        }
        payload.align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(if self.participant.is_some() { 2 } else { 1 }, 8)
            .put(self.endpoint.scene.into(), 13);
        if let Some(id) = self.participant {
            body.put(id.into(), 13);
        }
        body.put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Report {
    Location {
        participant: u16,
        first: bool,
    },
    Crossing {
        endpoint: Endpoint,
        participant: u16,
        token: u32,
        seconds: f64,
        inside: bool,
        changed: bool,
    },
}
#[derive(Clone, Debug, Default, PartialEq)]
struct Member {
    located: bool,
    // Each subscription starts outside until the owner reports a crossing.
    observations: BTreeMap<(u16, u16), (f64, bool)>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Activation {
    volumes: BTreeMap<(u16, u16), Endpoint>,
    presence: Endpoint,
    streamed: Endpoint,
    location: Endpoint,
    members: BTreeMap<u16, Member>,
}
impl Activation {
    pub fn bind(records: &[Record], catalog: &Catalog) -> Result<Self, Error> {
        catalog.validate()?;
        let endpoint = |site: Site, kind: sublevel::Kind| -> Result<Endpoint, Error> {
            let mut candidates =
                records
                    .iter()
                    .filter_map(|record| match (&record.initial, &record.update) {
                        (
                            Some(Initial::SubLevel { prefix, fields }),
                            crate::replication::Update::SubLevel { profile, .. },
                        ) if prefix.content_key == site.scene_key => {
                            Some((record.id, fields, profile))
                        }
                        _ => None,
                    });
            let (scene, fields, profile) = candidates.next().ok_or(Error::UnknownObject)?;
            if candidates.next().is_some() {
                return Err(Error::DuplicateObject);
            }
            if profile.kinds().get(site.serializer) != Some(&kind) {
                return Err(Error::TypeMismatch);
            }
            let rpc = fields
                .get(site.serializer)
                .and_then(sublevel::Initial::rpc)
                .ok_or(Error::TypeMismatch)?;
            if scene == 0 || scene > 8191 || rpc.selector > 511 {
                return Err(Error::Bound);
            }
            Ok(Endpoint {
                scene,
                selector: rpc.selector,
                serial: rpc.serial,
            })
        };
        let mut volumes = BTreeMap::new();
        for site in &catalog.volumes {
            let e = endpoint(*site, sublevel::Kind::Rpc)?;
            if volumes.insert((e.scene, e.selector), e).is_some() {
                return Err(Error::DuplicateObject);
            }
        }
        Ok(Self {
            volumes,
            presence: endpoint(catalog.presence, sublevel::Kind::RpcBool)?,
            streamed: endpoint(catalog.streamed, sublevel::Kind::Rpc)?,
            location: endpoint(catalog.location, sublevel::Kind::RpcFourReferences)?,
            members: BTreeMap::new(),
        })
    }
    pub fn participants(&self) -> impl Iterator<Item = u16> + '_ {
        self.members.keys().copied()
    }
    /// The full transition is committed only if its output fits. A capacity
    /// refusal leaves membership and observations intact for the next poll.
    pub fn set(
        &mut self,
        participant: u16,
        active: bool,
        capacity: usize,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Vec<Notification>, Error> {
        if participant == 0 || participant > 8191 {
            return Err(Error::Bound);
        }
        if !owns(participant) {
            return Err(Error::UnknownObject);
        }
        if self.members.contains_key(&participant) == active {
            return Ok(Vec::new());
        }
        if active && self.members.len() >= MAX_PARTICIPANTS {
            return Err(Error::Bound);
        }
        let presence_changed = self.members.len() == usize::from(!active);
        let needed = self.volumes.len() + usize::from(active) + usize::from(presence_changed);
        if needed > capacity {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(needed);
        if active {
            out.push(Notification {
                endpoint: self.streamed,
                participant: Some(participant),
                call: Call::StreamedParticipant,
            });
        }
        if presence_changed {
            out.push(Notification {
                endpoint: self.presence,
                participant: None,
                call: Call::Presence(active),
            });
        }
        out.extend(self.volumes.values().map(|&endpoint| Notification {
            endpoint,
            participant: Some(participant),
            call: if active {
                Call::Subscribe {
                    token: 0,
                    inside: false,
                }
            } else {
                Call::Unsubscribe
            },
        }));
        // Validate all generated forms before mutating membership.
        for notification in &out {
            notification.encode()?;
        }
        if active {
            self.members.insert(participant, Member::default());
        } else {
            self.members.remove(&participant);
        }
        Ok(out)
    }
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
        let Some(&scene) = envelope.references().first() else {
            return Ok(None);
        };
        let key = (scene, route.selector());
        let location = key == (self.location.scene, self.location.selector);
        let volume = self.volumes.get(&key).copied();
        if !(location && route.method_index() == 0
            || volume.is_some() && matches!(route.method_index(), 2 | 3))
        {
            return Ok(None);
        }
        let &[_, participant] = envelope.references() else {
            return Err(Error::Shape);
        };
        if envelope.words() != [0, 0] || !envelope.remaining().is_empty() {
            return Err(Error::Shape);
        }
        if !owns(participant) {
            return Err(Error::UnknownObject);
        }
        let member = self
            .members
            .get_mut(&participant)
            .ok_or(Error::UnknownObject)?;
        if location {
            if route.arguments().len() != 7 {
                return Err(Error::Shape);
            }
            let first = !member.located;
            member.located = true;
            return Ok(Some(Report::Location { participant, first }));
        }
        if route.arguments().len() != 103 {
            return Err(Error::Shape);
        }
        let bits = route.arguments();
        let high = bits.read_u32(0, 32).map_err(|_| Error::Shape)?;
        let low = bits.read_u32(32, 32).map_err(|_| Error::Shape)?;
        let seconds = f64::from_bits((u64::from(high) << 32) | u64::from(low));
        let token = bits.read_u32(64, 32).map_err(|_| Error::Shape)?;
        if !seconds.is_finite() || seconds < 0. || token != 0 {
            return Err(Error::Shape);
        }
        let inside = route.method_index() == 2;
        let previous = member.observations.get(&key);
        let fresh = previous.is_none_or(|(time, _)| seconds > *time);
        let changed = fresh && previous.map_or(inside, |(_, old)| *old != inside);
        if fresh {
            member.observations.insert(key, (seconds, inside));
        }
        Ok(Some(Report::Crossing {
            endpoint: volume.ok_or(Error::UnknownObject)?,
            participant,
            token,
            seconds,
            inside,
            changed,
        }))
    }
}

#[cfg(test)]
mod tests;
