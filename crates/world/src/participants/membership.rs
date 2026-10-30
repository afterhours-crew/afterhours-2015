// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Domain operations for the shared scene owner. Wire Leave notifications do
//! not identify a transition target and must never be decoded into these.
use super::{Endpoint, Error};

pub const MAX_PENDING: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Enter {
        participant: u16,
        state: Endpoint,
    },
    Complete {
        participant: u16,
        state: Endpoint,
    },
    Move {
        participant: u16,
        source: Endpoint,
        target: Option<Endpoint>,
    },
    Remove {
        participant: u16,
        state: Endpoint,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_remains_an_error_and_a_rejected_candidate_does_not_consume_the_original() {
        let operation = Operation::Enter {
            participant: 8,
            state: Endpoint {
                scene: 2,
                selector: 3,
                serial: nfs_protocol::world::rpc::Serial::new(1).unwrap(),
            },
        };
        let mut original = Journal::default();
        original.push(operation);
        let mut candidate = original.clone();
        assert_eq!(candidate.take().unwrap(), vec![operation]);
        assert_eq!(original.take().unwrap(), vec![operation]);
        for _ in 0..=MAX_PENDING {
            candidate.push(operation);
        }
        assert_eq!(candidate.take(), Err(Error::Bound));
        assert_eq!(candidate.take(), Err(Error::Bound));
        assert_eq!(candidate.operations.len(), MAX_PENDING);
    }
}

/// Part of the candidate lifecycle transaction. An overflow poisons that
/// candidate until its owner rejects it; taking the journal cannot erase the
/// failure. This bounds even callers which fail to drain between advances.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Journal {
    operations: Vec<Operation>,
    overflow: bool,
}
impl Journal {
    pub fn push(&mut self, operation: Operation) {
        if self.operations.len() == MAX_PENDING {
            self.overflow = true;
        } else if !self.overflow {
            self.operations.push(operation);
        }
    }
    pub fn take(&mut self) -> Result<Vec<Operation>, Error> {
        if self.overflow {
            return Err(Error::Bound);
        }
        Ok(std::mem::take(&mut self.operations))
    }
    pub(super) fn path(&mut self, participant: u16, path: &[Endpoint]) {
        for pair in path.windows(2) {
            self.push(Operation::Move {
                participant,
                source: pair[0],
                target: Some(pair[1]),
            });
        }
    }
}
