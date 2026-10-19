// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The driving client requests a single tire burst or reinflation. The host
//! publishes that wheel's flag through its ordinary component replication.
use crate::replication::{Error, Initial, Record, players::Players, vehicle};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub vehicle: u16,
    pub component: usize,
    pub burst: bool,
    pub record: Option<Record>,
}

/// Exact current delegation is separate from owned inventory. The caller commits
/// candidate world state only after all frame processing and queue checks pass.
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
    let Some(component) = creation.fields.iter().position(|field| {
        matches!(field, vehicle::Initial::Wheel { rpc, .. } if rpc.selector == route.selector())
    }) else { return Ok(None) };
    if route.method_index() > 1 {
        return Ok(None);
    }
    // These methods have no arguments; seven bits align the RPC payload.
    if envelope.references() != [id]
        || envelope.words() != [0, 0]
        || !envelope.remaining().is_empty()
        || route.arguments().len() != 7
    {
        return Err(Error::Shape);
    }
    let burst = route.method_index() == 0;
    let record = if let Some(&key) = delegated.get(&id) {
        if players
            .ai_ids(connection, persona, key)
            .map(|ids| ids.vehicle)
            != Some(id)
        {
            return Err(Error::UnknownObject);
        }
        players.set_ai_vehicle_tire_burst(connection, persona, key, component, burst)?
    } else {
        if !ready(id) {
            return Err(Error::UnknownObject);
        }
        players.set_vehicle_tire_burst(connection, persona, id, component, burst)?
    };
    Ok(Some(Report {
        vehicle: id,
        component,
        burst,
        record,
    }))
}
