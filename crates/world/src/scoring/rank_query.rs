// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Server IntParticipantPropertySorterQueryByIndex. The owning mode supplies
//! the linked sorter's ordered snapshot; this query does not sort independently.
//! Native 046ddd70 sends a participant/Int32 update only when either changes,
//! and a reset only when a previously valid index becomes invalid.
use super::Error;
use std::collections::BTreeSet;

pub const INDEX: u32 = 0x0cc9_279b;
pub const MAX_TABLES: usize = 128;
pub const MAX_ROWS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Row {
    pub participant: u64,
    pub value: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Update {
    pub node: u16,
    /// Some uses native updateData; None uses resetData. Client-side authored
    /// DefaultValue/DefaultName are applied by resetData, not by the server.
    pub row: Option<Row>,
}

pub fn validate(rows: &[Row]) -> Result<(), Error> {
    if rows.len() > MAX_ROWS {
        return Err(Error::Bound);
    }
    let mut seen = BTreeSet::new();
    if rows.iter().any(|r| !seen.insert(r.participant)) {
        return Err(Error::InvalidGraph);
    }
    Ok(())
}

pub fn select(rows: &[Row], index: i32) -> Option<Row> {
    usize::try_from(index)
        .ok()
        .and_then(|i| rows.get(i))
        .copied()
}
