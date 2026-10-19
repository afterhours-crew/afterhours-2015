// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::replication::{
    Record, Update,
    section::{Section, Setup},
    vehicle,
};

fn snapshot(record: &Record) -> bool {
    if record.initial.is_some() {
        return false;
    }
    let Update::Vehicle { fields, .. } = &record.update else {
        return matches!(&record.update, Update::ParticipantLocationMap(map) if map.groups.is_some());
    };
    let mut present = fields.iter().flatten();
    matches!(present.next(), Some(vehicle::Update::Chassis(c))
        if c.physics.is_some() && c.flag.is_none() && c.custom.is_none() && c.pair.is_none())
        && present.next().is_none()
}

/// Coalesce unsent absolute snapshots, retaining the newest sample per car.
/// Location snapshots can share the frame while their participant set is stable.
/// Lifecycle/appearance changes, relative origins and RPCs remain barriers.
pub(super) fn combine(previous: &Section, next: &Section) -> Option<Section> {
    for section in [previous, next] {
        if !section.deleted.is_empty()
            || section.records.is_empty()
            || section.setup != Some(Setup::RawEscape([0; 3]))
            || !section.records.iter().all(snapshot)
        {
            return None;
        }
    }
    let mut combined = previous.clone();
    for new in &next.records {
        if let Some(old) = combined.records.iter_mut().find(|old| old.id == new.id) {
            if !supersedes(old, new) {
                return None;
            }
            *old = new.clone();
        } else {
            combined.records.push(new.clone());
        }
    }
    Some(combined)
}

fn supersedes(old: &Record, new: &Record) -> bool {
    match (&old.update, &new.update) {
        (
            Update::Vehicle {
                binding: a,
                fields: af,
            },
            Update::Vehicle {
                binding: b,
                fields: bf,
            },
        ) if a == b => {
            let revision = |fields: &[Option<vehicle::Update>]| {
                fields.iter().flatten().find_map(|field| match field {
                    vehicle::Update::Chassis(c) => c.physics.as_ref().map(|p| p.identity_words[2]),
                    _ => None,
                })
            };
            matches!((revision(af), revision(bf)), (Some(a), Some(b)) if b > a)
        }
        (Update::ParticipantLocationMap(a), Update::ParticipantLocationMap(b)) => {
            // A map is a complete replacement, but membership changes can
            // accompany vehicle transitions and must preserve their ordering.
            let membership = |map: &crate::replication::location_map::Update| {
                map.groups.as_ref().map(|groups| {
                    groups
                        .iter()
                        .map(|g| {
                            (
                                g.value,
                                g.entries.iter().map(|e| e.participant).collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
            };
            membership(a) == membership(b)
        }
        _ => false,
    }
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
