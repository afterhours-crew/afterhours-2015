// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Driver callbacks from the car's chassis endpoint. Horn changes replicate
//! only its Boolean field. Drift exit is an event for an active gameplay hook;
//! receiving it does not itself award points or generate a client reply.
use crate::replication::{Error, Initial, Record, players::Players, vehicle};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    Horn {
        vehicle: u16,
        sounding: bool,
        record: Option<Box<Record>>,
    },
    DriftExit {
        vehicle: u16,
        value: f32,
    },
}

/// The host's current delegation map grants driving callbacks, never inventory
/// ownership. Commit mutated Players only with the frame's reliable queue.
pub fn receive(
    players: &mut Players,
    connection: u8,
    persona: u64,
    body: BitSpan<'_>,
    ready: impl Fn(u16) -> bool,
    delegated: &BTreeMap<u16, u64>,
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
    let Some(&id) = envelope.references().first() else {
        return Ok(None);
    };
    let Some(Initial::Vehicle { creation, .. }) = players.objects().get(id).map(|o| &o.initial)
    else {
        return Ok(None);
    };
    if !creation.fields.iter().any(|field| {
        matches!(field, vehicle::Initial::Chassis(chassis) if chassis.rpc.selector == route.selector())
    }) || !matches!(route.method_index(), 0 | 2)
    {
        return Ok(None);
    }
    let bits = if route.method_index() == 0 { 7 } else { 39 };
    if envelope.references() != [id]
        || envelope.words() != [0, 0]
        || !envelope.remaining().is_empty()
        || route.arguments().len() != bits
    {
        return Err(Error::Shape);
    }
    let ai = delegated.get(&id).copied();
    if let Some(key) = ai {
        if players
            .ai_ids(connection, persona, key)
            .map(|ids| ids.vehicle)
            != Some(id)
        {
            return Err(Error::UnknownObject);
        }
    } else if !ready(id) || !players.owned_vehicles(connection, persona).contains(&id) {
        return Err(Error::UnknownObject);
    }
    if route.method_index() == 2 {
        let value = f32::from_bits(
            route
                .arguments()
                .read_u32(0, 32)
                .map_err(|_| Error::Shape)?,
        );
        if !value.is_finite() {
            return Err(Error::Shape);
        }
        return Ok(Some(Report::DriftExit { vehicle: id, value }));
    }
    // One Boolean followed by six payload-alignment bits; padding may be nonzero.
    let sounding = route.arguments().read_u32(0, 1).map_err(|_| Error::Shape)? != 0;
    let record = if let Some(key) = ai {
        players.set_ai_vehicle_horn(connection, persona, key, sounding)?
    } else {
        players.set_vehicle_horn(connection, persona, id, sounding)?
    };
    Ok(Some(Report::Horn {
        vehicle: id,
        sounding,
        record: record.map(Box::new),
    }))
}

#[cfg(test)]
mod tests;
