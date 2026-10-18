// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::replication::{
    Record, Update,
    section::{Section, Setup},
    vehicle,
};

fn physics(record: &Record) -> bool {
    if record.initial.is_some() {
        return false;
    }
    let Update::Vehicle { fields, .. } = &record.update else {
        return false;
    };
    let mut present = fields.iter().flatten();
    matches!(present.next(), Some(vehicle::Update::Chassis(c))
        if c.physics.is_some() && c.flag.is_none() && c.custom.is_none() && c.pair.is_none())
        && present.next().is_none()
}

/// Adjacent motion for different cars can share a publication. Never combine
/// lifecycle/appearance changes, relative origins or two states of the same car.
/// RPC and deletion barriers are checked by the caller and the section guards.
pub(super) fn combine(previous: &Section, next: &Section) -> Option<Section> {
    for section in [previous, next] {
        if !section.deleted.is_empty()
            || section.records.is_empty()
            || section.setup != Some(Setup::RawEscape([0; 3]))
            || !section.records.iter().all(physics)
        {
            return None;
        }
    }
    if previous
        .records
        .iter()
        .any(|old| next.records.iter().any(|new| old.id == new.id))
    {
        return None;
    }
    let mut combined = previous.clone();
    combined.records.extend(next.records.iter().cloned());
    Some(combined)
}

/// Keep the original pacing phase when a poll runs slightly late. Resetting to
/// each actual send time makes a 50 ms timer miss alternate 50 ms deadlines.
pub(super) fn slot(previous: Option<u64>, now: u64, interval: u64) -> u64 {
    previous.map_or(now, |last| {
        last + now.saturating_sub(last) / interval * interval
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_polls_do_not_accumulate_an_extra_interval_or_catch_up_in_a_burst() {
        let mut previous = None;
        let mut sent = Vec::new();
        for now in [0, 51, 100, 151, 200, 251, 300, 900, 901, 950] {
            if previous.is_none_or(|last| now >= last + 50) {
                previous = Some(slot(previous, now, 50));
                sent.push(now);
            }
        }
        assert_eq!(sent, [0, 51, 100, 151, 200, 251, 300, 900, 950]);
    }
}
