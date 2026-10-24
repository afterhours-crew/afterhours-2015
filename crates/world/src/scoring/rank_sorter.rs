// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! IntParticipantPropertySorter's observed small-grid contract. Activation
//! subscribes without enumerating old values. Each property callback changes
//! one row and sorts the current order; equal scores retain that order. The
//! native large-vector partition path is outside this bounded implementation.
use super::{Error, rank_query::Row};

pub const MAX_ROWS: usize = 28;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Order {
    Descending,
    Ascending,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sorter {
    order: Order,
    rows: Vec<Row>,
}

impl Sorter {
    pub fn new(order: Order) -> Self {
        Self {
            order,
            rows: Vec::new(),
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Invoke on a source-property notification. Even an equal notification
    /// publishes the table; source stores, rather than the sorter, suppress
    /// equal writes. A failed insertion leaves the previous table untouched.
    pub fn notify(&mut self, participant: u64, value: i32) -> Result<&[Row], Error> {
        if let Some(row) = self.rows.iter_mut().find(|r| r.participant == participant) {
            row.value = value;
        } else {
            if self.rows.len() == MAX_ROWS {
                return Err(Error::Bound);
            }
            self.rows.push(Row { participant, value });
        }
        self.rows.sort_by(|a, b| match self.order {
            Order::Ascending => a.value.cmp(&b.value),
            Order::Descending => b.value.cmp(&a.value),
        });
        Ok(&self.rows)
    }

    /// Participant destruction, not an event-state departure. Queries must be
    /// notified only when a row was present. Remaining ties keep their order.
    pub fn remove(&mut self, participant: u64) -> bool {
        if let Some(i) = self.rows.iter().position(|r| r.participant == participant) {
            self.rows.remove(i);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ids(s: &Sorter) -> Vec<u64> {
        s.rows().iter().map(|r| r.participant).collect()
    }

    #[test]
    fn changes_keep_previous_tie_order_in_both_directions() {
        for order in [Order::Ascending, Order::Descending] {
            let mut s = Sorter::new(order);
            assert!(s.rows().is_empty());
            for p in [7, 2, 5] {
                s.notify(p, 10).unwrap();
            }
            assert_eq!(ids(&s), [7, 2, 5]);
            let best = if order == Order::Ascending {
                i32::MIN
            } else {
                i32::MAX
            };
            s.notify(5, best).unwrap();
            assert_eq!(ids(&s), [5, 7, 2]);
            s.notify(5, 10).unwrap();
            assert_eq!(ids(&s), [5, 7, 2]);
            s.notify(7, 10).unwrap();
            assert_eq!(ids(&s), [5, 7, 2]);
            assert!(s.remove(7));
            assert!(!s.remove(7));
            s.notify(7, 10).unwrap();
            assert_eq!(ids(&s), [5, 2, 7]);
        }
    }

    #[test]
    fn full_table_allows_updates_and_rejects_new_rows_atomically() {
        let mut s = Sorter::new(Order::Ascending);
        for p in 0..MAX_ROWS as u64 {
            s.notify(p, p as i32).unwrap();
        }
        let previous = s.clone();
        assert_eq!(s.notify(100, 0), Err(Error::Bound));
        assert_eq!(s, previous);
        s.notify(0, i32::MAX).unwrap();
        assert_eq!(s.rows().last().unwrap().participant, 0);
        assert!(s.remove(1));
        s.notify(100, i32::MIN).unwrap();
        assert_eq!(s.rows()[0].participant, 100);
    }
}
