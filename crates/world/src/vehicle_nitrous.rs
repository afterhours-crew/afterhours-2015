// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Acknowledge the authoritative driver's nitrous start/stop transitions.
//!
//! The client waits for a mode update before sending its next transition.
//! This handshake changes only the mode; it does not reset the locally simulated
//! fuel level or derive nitrous activation from accelerator input.
use crate::replication::{Error, Initial, Record, players::Players, vehicle};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub vehicle: u16,
    pub active: bool,
    pub client_seconds: f32,
    pub record: Option<Record>,
}

/// `None` means the call is not a supported nitrous endpoint. The caller must
/// commit the changed Players together with its bounded reliable output queue.
pub fn receive(
    players: &mut Players,
    connection: u8,
    persona: u64,
    body: BitSpan<'_>,
    ready: impl Fn(u16) -> bool,
) -> Result<Option<Report>, Error> {
    receive_with_ai(players, connection, persona, body, ready, &BTreeMap::new())
}

/// The host supplies its currently delegated AI lifetimes separately from owned
/// inventory. Every mapping is checked against the session's exact live car.
pub fn receive_with_ai(
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
        matches!(field, vehicle::Initial::Index { rpc, .. } if rpc.selector == route.selector())
    }) || route.method_index() > 1
    {
        return Ok(None);
    }
    if envelope.references() != [id]
        || envelope.words() != [0, 0]
        || !envelope.remaining().is_empty()
        || route.arguments().len() != 39
    {
        return Err(Error::Shape);
    }
    // The final seven bits align the RPC payload and may be nonzero.
    let client_seconds = f32::from_bits(
        route
            .arguments()
            .read_u32(0, 32)
            .map_err(|_| Error::Shape)?,
    );
    if !client_seconds.is_finite() || client_seconds < 0.0 {
        return Err(Error::Shape);
    }
    let active = route.method_index() == 0;
    let record = if let Some(&key) = delegated.get(&id) {
        if players
            .ai_ids(connection, persona, key)
            .map(|ids| ids.vehicle)
            != Some(id)
        {
            return Err(Error::UnknownObject);
        }
        players.set_ai_vehicle_nitrous_mode(connection, persona, key, active)?
    } else {
        if !ready(id) {
            return Err(Error::UnknownObject);
        }
        players.set_vehicle_nitrous_mode(connection, persona, id, active)?
    };
    Ok(Some(Report {
        vehicle: id,
        active,
        client_seconds,
        record,
    }))
}

#[cfg(test)]
mod tests;
