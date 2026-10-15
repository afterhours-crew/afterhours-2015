// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The 13-byte routing transaction, independent of any process.
//!
//! The game keeps its backend endpoint in three adjacent fields of one
//! startup object: a 10-byte host string, a 1-byte flag and a 2-byte port.
//! Routing the game to the local server means changing those bytes before
//! the first backend request. This type performs that change as a
//! transaction over an abstract [`Memory`]: it records the original bytes,
//! requires them to be the known startup defaults, writes each field only
//! after confirming nothing changed, verifies every write by reading back,
//! and can roll back a partial change without ever overwriting bytes it did
//! not write itself.
//!
//! It is tested here against constructed memory with injected faults. The
//! Windows lease supplies the real memory, bounded reads and identity
//! guards.

use crate::exit::{Failure, Result};

/// Byte widths of the host, flag and port fields.
pub const FIELD_SIZES: [usize; 3] = [10, 1, 2];

/// Access to the three endpoint fields.
pub trait Memory {
    /// Current bytes of `field`.
    fn read(&mut self, field: usize) -> Result<Vec<u8>>;
    /// Replace the bytes of `field`.
    fn write(&mut self, field: usize, value: &[u8]) -> Result<()>;
    /// Confirm the owner of the memory is still the same process in the
    /// same state. Called before every read.
    fn guard(&mut self) -> Result<()>;
}

type Fields = [Vec<u8>; 3];

/// A prepared, reversible change of the endpoint fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointTransaction {
    original: Fields,
    replacement: Fields,
}

fn refuse(message: &str) -> Failure {
    Failure::session(message)
}

impl EndpointTransaction {
    /// Record the current fields and require them to be the startup
    /// defaults: an empty host, flag `1` and port `0`.
    pub fn new(memory: &mut impl Memory, port: u16) -> Result<Self> {
        if port < 1024 {
            return Err(refuse("Expected an unprivileged loopback port"));
        }
        let original = Self::current(memory)?;
        if original[0][0] != 0 || original[1][0] != 1 || original[2].iter().any(|&b| b != 0) {
            return Err(refuse("Endpoint is not at its startup defaults"));
        }
        let mut host = b"127.0.0.1\0".to_vec();
        host.resize(FIELD_SIZES[0], 0);
        Ok(Self {
            original,
            replacement: [host, vec![0], port.to_le_bytes().to_vec()],
        })
    }

    fn current(memory: &mut impl Memory) -> Result<Fields> {
        memory.guard()?;
        let state = [memory.read(0)?, memory.read(1)?, memory.read(2)?];
        if state
            .iter()
            .zip(FIELD_SIZES)
            .any(|(bytes, size)| bytes.len() != size)
        {
            return Err(refuse("Partial endpoint read"));
        }
        Ok(state)
    }

    fn change(
        memory: &mut impl Memory,
        field: usize,
        value: &[u8],
        expected: &Fields,
    ) -> Result<Fields> {
        if Self::current(memory)? != *expected {
            return Err(refuse("Endpoint changed before write"));
        }
        memory.write(field, value)?;
        let mut next = expected.clone();
        next[field] = value.to_vec();
        if Self::current(memory)? != next {
            return Err(refuse("Endpoint write did not verify"));
        }
        Ok(next)
    }

    /// Write the local endpoint: port, then flag, then host, so the host
    /// string only becomes visible once the other fields are consistent.
    pub fn apply(&self, memory: &mut impl Memory) -> Result<()> {
        let mut current = Self::current(memory)?;
        if current != self.original {
            return Err(refuse("Startup bytes changed"));
        }
        for field in [2, 1, 0] {
            current = Self::change(memory, field, &self.replacement[field], &current)?;
        }
        Ok(())
    }

    /// Put the original bytes back. Refuses if any byte is neither original
    /// nor ours, because then another writer owns that memory.
    pub fn restore(&self, memory: &mut impl Memory) -> Result<()> {
        let mut current = Self::current(memory)?;
        let ours = (0..3).all(|i| {
            current[i]
                .iter()
                .enumerate()
                .all(|(j, &b)| b == self.original[i][j] || b == self.replacement[i][j])
        });
        if !ours {
            return Err(refuse("Unrelated endpoint change; refusing overwrite"));
        }
        for field in 0..3 {
            if current[field] != self.original[field] {
                current = Self::change(memory, field, &self.original[field], &current)?;
            }
        }
        if Self::current(memory)? != self.original {
            return Err(refuse("Endpoint rollback did not verify"));
        }
        Ok(())
    }

    /// Whether the fields currently hold their original bytes.
    pub fn is_original(&self, memory: &mut impl Memory) -> Result<bool> {
        Ok(Self::current(memory)? == self.original)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Constructed memory with an optional injected partial write and an
    /// identity guard that can be tripped.
    struct Fake {
        fields: Fields,
        writes: usize,
        fail_on_write: Option<usize>,
        alive: bool,
    }

    impl Fake {
        fn startup() -> Fields {
            [vec![0, 9, 8, 7, 6, 5, 4, 3, 2, 1], vec![1], vec![0, 0]]
        }
        fn new() -> Self {
            Self {
                fields: Self::startup(),
                writes: 0,
                fail_on_write: None,
                alive: true,
            }
        }
    }

    impl Memory for Fake {
        fn read(&mut self, field: usize) -> Result<Vec<u8>> {
            Ok(self.fields[field].clone())
        }
        fn write(&mut self, field: usize, value: &[u8]) -> Result<()> {
            self.writes += 1;
            if self.fail_on_write == Some(self.writes) {
                // A failing write can still modify memory before reporting.
                let half = (value.len() / 2).max(1);
                self.fields[field][..half].copy_from_slice(&value[..half]);
                self.fail_on_write = None;
                return Err(refuse("Injected partial write"));
            }
            self.fields[field] = value.to_vec();
            Ok(())
        }
        fn guard(&mut self) -> Result<()> {
            if self.alive {
                Ok(())
            } else {
                Err(refuse("Process/root identity changed"))
            }
        }
    }

    #[test]
    fn a_partial_write_at_any_step_rolls_back_to_the_original_padding() {
        for failed_write in 1..=3 {
            let mut memory = Fake::new();
            memory.fail_on_write = Some(failed_write);
            let tx = EndpointTransaction::new(&mut memory, 0x1234).unwrap();
            assert!(tx.apply(&mut memory).is_err());
            tx.restore(&mut memory).unwrap();
            assert_eq!(memory.fields, Fake::startup(), "write {failed_write}");
            let writes = memory.writes;
            tx.restore(&mut memory).unwrap();
            assert_eq!(memory.writes, writes, "rollback is idempotent");
        }
    }

    #[test]
    fn another_writers_bytes_are_never_overwritten() {
        let mut memory = Fake::new();
        let tx = EndpointTransaction::new(&mut memory, 32_000).unwrap();
        tx.apply(&mut memory).unwrap();
        assert_eq!(memory.fields[0][0], b'1', "host starts with ASCII 1");
        assert_eq!(memory.fields[2], 32_000u16.to_le_bytes());
        memory.fields[0][0] = 250;
        let before = memory.writes;
        assert!(tx.restore(&mut memory).is_err());
        assert_eq!(memory.writes, before);
    }

    #[test]
    fn an_identity_failure_precedes_every_write() {
        let mut memory = Fake::new();
        let tx = EndpointTransaction::new(&mut memory, 32_000).unwrap();
        memory.alive = false;
        assert!(tx.apply(&mut memory).is_err());
        assert_eq!(memory.writes, 0);
    }

    #[test]
    fn apply_is_single_use_and_fully_reversible() {
        let mut memory = Fake::new();
        let tx = EndpointTransaction::new(&mut memory, 32_000).unwrap();
        tx.apply(&mut memory).unwrap();
        assert!(!tx.is_original(&mut memory).unwrap());
        assert!(tx.apply(&mut memory).is_err(), "startup bytes changed");
        tx.restore(&mut memory).unwrap();
        assert_eq!(memory.fields, Fake::startup());
        assert!(tx.is_original(&mut memory).unwrap());
    }

    #[test]
    fn privileged_ports_and_non_default_startup_bytes_are_refused() {
        let mut memory = Fake::new();
        assert!(EndpointTransaction::new(&mut memory, 80).is_err());
        memory.fields[1][0] = 2;
        assert!(EndpointTransaction::new(&mut memory, 32_000).is_err());
        let mut short = Fake::new();
        short.fields[2] = vec![0];
        assert!(
            EndpointTransaction::new(&mut short, 32_000).is_err(),
            "partial read"
        );
    }
}
