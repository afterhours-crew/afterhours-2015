// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Query measurements, never completed REP awards. Registration belongs to the
//! installed graph and the participant lifecycle; receiving has no side effects.
use super::math::Value;
use crate::replication::{Error, Initial, entity, players::Players};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    NearMiss,
    AccumulatedCollisions,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    /// Four native scalar properties in wire order. Only the first property's
    /// RelativeSpeed name is independently established by the scoring graph.
    NearMiss([f32; 4]),
    CollisionWeight(i32),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Publication {
    /// Publish in this order, then emit event. These are query node port IDs.
    pub properties: Vec<(u32, Value)>,
    pub event: u32,
}
impl Report {
    pub fn publication(&self) -> Publication {
        match self {
            Self::NearMiss(values) => Publication {
                properties: [0xad59_4576, 0x445f_266b, 0xd70a_7f0c, 0xd887_31d6]
                    .into_iter()
                    .zip(values.iter().copied().map(Value::Float))
                    .collect(),
                event: 1_552_797_432,
            },
            Self::CollisionWeight(value) => Publication {
                properties: vec![(1_977_761_241, Value::Integer(*value))],
                event: 1_190_559_846,
            },
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    entity: u16,
    participant: u16,
    serializer: usize,
    rpc: entity::Rpc,
    root: entity::Rpc,
    asset: entity::creation::Asset,
    kind: Kind,
}
impl Binding {
    /// Bind an installed query serializer on a newly created participant logic.
    /// Kind must come from its installed class, not a packet or serializer number.
    /// Drop bindings when retiring that logic or disconnecting the session.
    pub fn new(players: &Players, id: u16, serializer: usize, kind: Kind) -> Result<Self, Error> {
        let Some(Initial::Entity { prefix, fields }) =
            players.objects().get(id).map(|o| &o.initial)
        else {
            return Err(Error::UnknownObject);
        };
        let Some(entity::Initial::Root {
            rpc: root,
            reference: participant,
            ..
        }) = fields.first()
        else {
            return Err(Error::TypeMismatch);
        };
        let Some(entity::Initial::Rpc(rpc)) = fields.get(serializer) else {
            return Err(Error::TypeMismatch);
        };
        if rpc.selector == root.selector || *participant == 0 {
            return Err(Error::Shape);
        }
        Ok(Self {
            entity: id,
            participant: *participant,
            serializer,
            rpc: rpc.clone(),
            root: root.clone(),
            asset: prefix.asset,
            kind,
        })
    }

    /// None leaves a different target/selector/method for other dispatchers.
    /// A matching report requires the current participant owner and readiness.
    /// Transport must reject repeated reliable messages before this method;
    /// these native query arguments do not carry an independent award/retry ID.
    pub fn receive(
        &self,
        players: &Players,
        connection: u8,
        persona: u64,
        body: BitSpan<'_>,
        ready: impl Fn(u16) -> bool,
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
        if envelope.references().first() != Some(&self.entity)
            || route.selector() != self.rpc.selector
            || route.method_index() != 0
        {
            return Ok(None);
        }
        if envelope.references() != [self.entity]
            || envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
        {
            return Err(Error::Shape);
        }
        if !players.owns_participant(connection, persona, self.participant) || !ready(self.entity) {
            return Err(Error::UnknownObject);
        }
        let Some(Initial::Entity { prefix, fields }) =
            players.objects().get(self.entity).map(|o| &o.initial)
        else {
            return Err(Error::UnknownObject);
        };
        if prefix.asset != self.asset
            || !matches!(fields.first(), Some(entity::Initial::Root { rpc, reference, .. }) if rpc == &self.root && *reference == self.participant)
            || fields.get(self.serializer) != Some(&entity::Initial::Rpc(self.rpc.clone()))
        {
            return Err(Error::UnknownObject);
        }
        let arguments = route.arguments();
        let count = match self.kind {
            Kind::NearMiss => 4,
            Kind::AccumulatedCollisions => 1,
        };
        // Byte-aligned RPC payload includes seven opaque final padding bits.
        if arguments.len() != count * 32 + 7 {
            return Err(Error::Shape);
        }
        let word = |index| arguments.read_u32(index * 32, 32).map_err(|_| Error::Shape);
        let report = match self.kind {
            Kind::NearMiss => {
                let mut values = [0.0; 4];
                for (i, value) in values.iter_mut().enumerate() {
                    *value = f32::from_bits(word(i)?);
                    if !value.is_finite() {
                        return Err(Error::Shape);
                    }
                }
                Report::NearMiss(values)
            }
            Kind::AccumulatedCollisions => Report::CollisionWeight(word(0)? as i32),
        };
        Ok(Some(report))
    }
}

#[cfg(test)]
mod tests;
