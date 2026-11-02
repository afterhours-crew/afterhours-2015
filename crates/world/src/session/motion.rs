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

/// How early a poll may run and still send: a quarter of the interval. The
/// world loop's own timer has the same period as the send interval, so its
/// ticks land a millisecond either side of the pacing grid. Refusing the
/// early side waits a whole extra interval each time (E1278 owner run: one
/// third of the sends under backlog arrived 100 ms apart, the queue pinned
/// at its bound and client frames were refused).
pub(super) fn tolerance(interval: u64) -> u64 {
    interval / 4
}

/// Whether a send is due: on or after the next grid point, or within the
/// tolerance before it.
pub(super) fn due(previous: Option<u64>, now: u64, interval: u64) -> bool {
    previous.is_none_or(|last| now + tolerance(interval) >= last + interval)
}

/// The grid point to record for a send at `now`. A late send keeps its phase
/// as [`slot`] does; an early send (within the tolerance) advances the grid
/// one interval, so the following send cannot come sooner than an interval
/// less the tolerance.
pub(super) fn next_slot(previous: Option<u64>, now: u64, interval: u64) -> u64 {
    match previous {
        Some(last) if now < last + interval => last + interval,
        _ => slot(previous, now, interval),
    }
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

    fn drive(polls: impl IntoIterator<Item = u64>) -> Vec<u64> {
        let mut previous = None;
        let mut sent = Vec::new();
        for now in polls {
            if due(previous, now, 50) {
                previous = Some(next_slot(previous, now, 50));
                sent.push(now);
            }
        }
        sent
    }

    #[test]
    fn ticks_a_millisecond_early_send_every_interval() {
        // A 50 ms timer whose phase sits just before the grid: without the
        // tolerance every other tick is refused and the sends alternate
        // 50 ms and 100 ms apart.
        let polls = [0, 49, 99, 149, 199, 249, 299, 349, 399];
        assert_eq!(drive(polls), polls);
    }

    #[test]
    fn jittered_ticks_send_once_per_interval_without_bursts() {
        let polls = [0, 51, 99, 152, 198, 251, 299, 348, 402, 449, 501];
        assert_eq!(drive(polls), polls);
        // Polls every 5 ms (one per datagram) never send twice within an
        // interval less the tolerance.
        let sent = drive((0..2_000).step_by(5));
        assert!(sent.windows(2).all(|w| w[1] - w[0] >= 50 - tolerance(50)));
        let mean = (sent.last().unwrap() - sent[0]) as f64 / (sent.len() - 1) as f64;
        assert!((mean - 50.).abs() < 1., "mean spacing {mean}");
    }

    #[test]
    fn late_polls_keep_their_phase_and_do_not_catch_up_in_a_burst() {
        assert_eq!(
            drive([0, 51, 100, 151, 200, 251, 300, 900, 901, 950]),
            [0, 51, 100, 151, 200, 251, 300, 900, 950]
        );
        // Late keeps the phase; early (within the tolerance) advances it.
        assert_eq!(next_slot(Some(900), 951, 50), 950);
        assert_eq!(next_slot(Some(900), 949, 50), 950);
        assert_eq!(next_slot(Some(900), 950, 50), 950);
        assert_eq!(next_slot(None, 7, 50), 7);
        assert!(!due(Some(900), 937, 50));
        assert!(due(Some(900), 938, 50));
    }
}
