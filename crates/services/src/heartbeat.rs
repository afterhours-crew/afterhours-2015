// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Fire2 keepalive: the client sends an empty category-4 frame on component
//! and command 0; the server echoes it as category 5. Answered in every
//! session phase; nothing else is a heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A category-4 frame with any routing, metadata or body, or an
    /// incomplete or trailing frame.
    Ineligible,
    Encode,
}

pub const MAX_FRAME: usize = 64 * 1024;

pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(MAX_FRAME, 0, MAX_FRAME - 16).expect("constant limits")
}

/// `Ok(None)` when the frame is not a heartbeat (any other category); the
/// caller then dispatches it to the session phase.
pub fn reply(wire: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Error::Ineligible)?
        .ok_or(Error::Ineligible)?;
    if d.consumed != wire.len() {
        return Err(Error::Ineligible);
    }
    let f = d.frame;
    if f.fields.category != 4 {
        return Ok(None);
    }
    if f.fields.routing_a != 0
        || f.fields.routing_b != 0
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || !f.metadata.is_empty()
        || !f.body.is_empty()
    {
        return Err(Error::Ineligible);
    }
    nfs_fire2::encode(
        nfs_fire2::Frame {
            fields: nfs_fire2::Fields {
                category: 5,
                ..f.fields
            },
            metadata: &[],
            body: &[],
        },
        frame_limits(),
    )
    .map(Some)
    .map_err(|_| Error::Encode)
}
