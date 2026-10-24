// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local sprint records share the rewards transaction. They use an account-owned
//! versioned table, separate from the optional read-only imported wall document.
use super::{MAX_ROWS, MAX_VALUE_BYTES, Row};
use crate::{ContentError, persistent::key};
use nfs_storage::{
    AccountId, Error, Op, Snapshot,
    tables::{Table, Value},
};
use std::collections::BTreeMap;

const TABLE: &str = "AfterhoursSprintSpeedwalls";
const COLUMNS: [&str; 7] = [
    "Version",
    "Attempts",
    "REP",
    "Time",
    "Vehicle",
    "SecondaryVehicle",
    "EventId",
];

/// Imported own-row records remain a baseline; owned attempt counts are deltas
/// against that read-only document, so repeated queries do not add them again.
pub(super) fn combine(base: &Row, owned: &Row) -> Result<Row, ContentError> {
    let mut result = owned.clone();
    let get = |row: &Row, name: &str| {
        row.floats
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| f32::from_bits(*v))
    };
    for (stat, vehicle, high) in [
        ("REP", "Vehicle", true),
        ("Time", "SecondaryVehicle", false),
    ] {
        let Some(old) = get(base, stat) else {
            continue;
        };
        let new = get(owned, stat).ok_or(ContentError::Invalid)?;
        if !old.is_finite() || old < 0. {
            return Err(ContentError::Invalid);
        }
        if (high && old >= new) || (!high && old <= new) {
            result
                .floats
                .iter_mut()
                .find(|(k, _)| k == stat)
                .ok_or(ContentError::Invalid)?
                .1 = old.to_bits();
            if let Some((_, name)) = base.strings.iter().flatten().find(|(k, _)| k == vehicle) {
                result
                    .strings
                    .as_mut()
                    .and_then(|s| s.iter_mut().find(|(k, _)| k == vehicle))
                    .ok_or(ContentError::Invalid)?
                    .1 = name.clone();
            }
        }
    }
    let old = base
        .integers
        .iter()
        .find(|(k, _)| k == "Attempts")
        .map_or(0, |(_, v)| *v);
    let attempts = result
        .integers
        .iter_mut()
        .find(|(k, _)| k == "Attempts")
        .ok_or(ContentError::Invalid)?;
    if old < 0 {
        return Err(ContentError::Invalid);
    }
    attempts.1 = attempts.1.checked_add(old).ok_or(ContentError::TooLarge)?;
    Ok(result)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sprint {
    pub time_ms: u32,
    /// Total race REP, including driving credits already banked while racing.
    pub rep: i32,
    pub vehicle: String,
}

#[derive(Clone, Debug)]
pub struct Current {
    pub account: AccountId,
    pub generation: u64,
    pub(super) rows: Vec<Row>,
}

fn valid_name(value: &str) -> bool {
    value.len() <= MAX_VALUE_BYTES && !value.contains('\0')
}
fn integer(row: &BTreeMap<u32, Value>, name: &str) -> Result<i32, Error> {
    match row.get(&key(name)) {
        Some(Value::Int(v)) => Ok(*v),
        _ => Err(Error::Config),
    }
}
fn float(row: &BTreeMap<u32, Value>, name: &str) -> Result<f32, Error> {
    match row.get(&key(name)) {
        Some(Value::Float(bits))
            if f32::from_bits(*bits).is_finite() && f32::from_bits(*bits) >= 0. =>
        {
            Ok(f32::from_bits(*bits))
        }
        _ => Err(Error::Config),
    }
}
fn text<'a>(row: &'a BTreeMap<u32, Value>, name: &str) -> Result<&'a str, Error> {
    match row.get(&key(name)) {
        Some(Value::String(v)) if valid_name(v) => Ok(v),
        _ => Err(Error::Config),
    }
}
fn validate(id: u64, row: &BTreeMap<u32, Value>) -> Result<(), Error> {
    if u32::try_from(id).is_err()
        || row.len() != COLUMNS.len()
        || COLUMNS.iter().any(|c| !row.contains_key(&key(c)))
        || integer(row, "Version")? != 1
        || integer(row, "Attempts")? < 1
        || integer(row, "EventId")? as u32 != id as u32
    {
        return Err(Error::Config);
    }
    float(row, "REP")?;
    float(row, "Time")?;
    text(row, "Vehicle")?;
    text(row, "SecondaryVehicle")?;
    Ok(())
}
fn table(snapshot: &Snapshot) -> Result<Table, Error> {
    let table = snapshot
        .tables
        .get(&key(TABLE))
        .cloned()
        .unwrap_or_default();
    table.validate()?;
    if table.rows.len() > MAX_ROWS / 2 {
        return Err(Error::Bounds);
    }
    for (&id, row) in &table.rows {
        validate(id, row)?;
    }
    Ok(table)
}

impl Sprint {
    /// REP keeps the high result and Time the low result, with their respective
    /// vehicle tokens. The caller puts this operation in the completion batch;
    /// repository retry keys provide exactly-once attempt increments.
    pub fn prepare(&self, snapshot: &Snapshot, event: u32) -> Result<Op, Error> {
        if self.time_ms == 0 || self.rep < 0 || !valid_name(&self.vehicle) {
            return Err(Error::Invalid);
        }
        let mut table = table(snapshot)?;
        let prior = table.rows.get(&u64::from(event));
        let rep = self.rep as f32;
        let time = self.time_ms as f32;
        let (attempts, best_rep, best_time, vehicle, secondary) = match prior {
            None => (1, rep, time, self.vehicle.as_str(), self.vehicle.as_str()),
            Some(old) => {
                let old_rep = float(old, "REP")?;
                let old_time = float(old, "Time")?;
                (
                    integer(old, "Attempts")?
                        .checked_add(1)
                        .ok_or(Error::Bounds)?,
                    rep.max(old_rep),
                    time.min(old_time),
                    if rep > old_rep {
                        self.vehicle.as_str()
                    } else {
                        text(old, "Vehicle")?
                    },
                    if time < old_time {
                        self.vehicle.as_str()
                    } else {
                        text(old, "SecondaryVehicle")?
                    },
                )
            }
        };
        let row = BTreeMap::from([
            (key("Version"), Value::Int(1)),
            (key("EventId"), Value::Int(event as i32)),
            (key("Attempts"), Value::Int(attempts)),
            (key("REP"), Value::Float(best_rep.to_bits())),
            (key("Time"), Value::Float(best_time.to_bits())),
            (key("Vehicle"), Value::String(vehicle.to_owned())),
            (key("SecondaryVehicle"), Value::String(secondary.to_owned())),
        ]);
        table.rows.insert(u64::from(event), row);
        if table.rows.len() > MAX_ROWS / 2 {
            return Err(Error::Bounds);
        }
        table.validate()?;
        Ok(Op::SetTable(key(TABLE), table))
    }
}

impl Current {
    pub fn from_snapshot(account: AccountId, snapshot: &Snapshot) -> Result<Self, ContentError> {
        let table = table(snapshot).map_err(|_| ContentError::Invalid)?;
        let mut rows = Vec::new();
        for (event, row) in table.rows {
            let event = event as u32;
            for stat_type in [0, 1] {
                rows.push(Row {
                    speed_wall_id: event,
                    stat_type,
                    rank: 1,
                    rating_bits: 0,
                    total_count: 1,
                    integers: vec![(
                        "Attempts".into(),
                        integer(&row, "Attempts").map_err(|_| ContentError::Invalid)?,
                    )],
                    floats: vec![
                        (
                            "REP".into(),
                            float(&row, "REP")
                                .map_err(|_| ContentError::Invalid)?
                                .to_bits(),
                        ),
                        (
                            "Time".into(),
                            float(&row, "Time")
                                .map_err(|_| ContentError::Invalid)?
                                .to_bits(),
                        ),
                    ],
                    strings: Some(vec![
                        (
                            "SecondaryVehicle".into(),
                            text(&row, "SecondaryVehicle")
                                .map_err(|_| ContentError::Invalid)?
                                .to_owned(),
                        ),
                        (
                            "Vehicle".into(),
                            text(&row, "Vehicle")
                                .map_err(|_| ContentError::Invalid)?
                                .to_owned(),
                        ),
                    ]),
                });
            }
        }
        Ok(Self {
            account,
            generation: snapshot.generation,
            rows,
        })
    }
}

#[cfg(test)]
mod tests;
