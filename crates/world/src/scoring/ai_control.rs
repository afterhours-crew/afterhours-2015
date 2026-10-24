// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scoped actor-control overrides. Only the last acquired override is observed
//! by the actor (03fcb840); older values are retained until it is released
//! (03fc8bd0/03fcbcb0). The host owns actor/participant mapping and lifetime IDs.
use super::Error;

pub const ENABLE: u32 = 0x93ef_b252;
pub const DISABLE: u32 = 0x7332_5ee7;
pub const RELEASE: u32 = 0x72f6_cc1d;
pub const ENABLED: u32 = 0x27d3_69b3;
pub const DISABLED: u32 = 0x4297_e1a6;
pub const MAX_OVERRIDES: usize = 256;

/// Unique for the lifetime of a graph occurrence, not just its blueprint.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Key {
    pub instance: u64,
    pub node: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Inclusion {
    Human,
    Ai,
    All,
}
impl Inclusion {
    pub fn includes(self, ai_member: bool) -> bool {
        match self {
            Self::Human => !ai_member,
            Self::Ai => ai_member,
            Self::All => true,
        }
    }
}

/// One actor's live override stack. The empty stack disables AI control. The
/// caller applies returned values to its actor; Some(false) is a notification,
/// whereas None means the call had no active observer. Equal writes to the top
/// override still notify, just as native Enable/Disable do.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Stack {
    entries: Vec<(Key, bool)>,
}
impl Stack {
    pub fn current(&self) -> bool {
        self.entries.last().is_some_and(|(_, value)| *value)
    }

    /// A new token starts disabled. Reusing it does not move it to the top.
    pub fn acquire(&mut self, key: Key) -> Result<Option<bool>, Error> {
        if self.entries.iter().any(|(existing, _)| *existing == key) {
            return Ok(None);
        }
        if self.entries.len() >= MAX_OVERRIDES {
            return Err(Error::Bound);
        }
        self.entries.push((key, false));
        Ok(Some(false))
    }

    pub fn set(&mut self, key: Key, enabled: bool) -> Result<Option<bool>, Error> {
        let Some(index) = self
            .entries
            .iter()
            .position(|(existing, _)| *existing == key)
        else {
            return Err(Error::MissingInput);
        };
        self.entries[index].1 = enabled;
        Ok((index + 1 == self.entries.len()).then_some(enabled))
    }

    /// Stable erasure preserves acquisition order. Native release reapplies the
    /// surviving top value even when the removed token was below it.
    pub fn release(&mut self, key: Key) -> Option<bool> {
        let index = self
            .entries
            .iter()
            .position(|(existing, _)| *existing == key)?;
        self.entries.remove(index);
        Some(self.current())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(instance: u64) -> Key {
        Key { instance, node: 6 }
    }

    #[test]
    fn overlapping_instances_keep_hidden_values_and_restore_in_order() {
        let mut stack = Stack::default();
        assert_eq!(stack.acquire(key(1)), Ok(Some(false)));
        assert_eq!(stack.set(key(1), true), Ok(Some(true)));
        assert_eq!(stack.acquire(key(2)), Ok(Some(false)));
        assert!(!stack.current());
        assert_eq!(stack.set(key(1), false), Ok(None));
        assert_eq!(stack.set(key(1), true), Ok(None));
        assert_eq!(stack.acquire(key(1)), Ok(None));
        assert!(!stack.current());
        assert_eq!(stack.release(key(2)), Some(true));
        assert_eq!(stack.release(key(1)), Some(false));
        assert_eq!(stack.release(key(1)), None);
    }

    #[test]
    fn repeats_and_middle_removal_preserve_notifications_and_order() {
        let mut stack = Stack::default();
        for id in 0..3 {
            stack.acquire(key(id)).unwrap();
        }
        assert_eq!(stack.set(key(2), true), Ok(Some(true)));
        assert_eq!(stack.set(key(2), true), Ok(Some(true)));
        assert_eq!(stack.release(key(1)), Some(true));
        stack.set(key(0), true).unwrap();
        assert_eq!(stack.release(key(2)), Some(true));
        assert_eq!(stack.release(key(0)), Some(false));
    }

    #[test]
    fn resource_limit_and_unknown_token_leave_state_unchanged() {
        let mut stack = Stack::default();
        for id in 0..MAX_OVERRIDES as u64 {
            stack.acquire(key(id)).unwrap();
        }
        let old = stack.clone();
        assert_eq!(stack.acquire(key(999)), Err(Error::Bound));
        assert_eq!(stack.set(key(999), true), Err(Error::MissingInput));
        assert_eq!(stack, old);
        stack.release(key(0));
        assert_eq!(stack.acquire(key(999)), Ok(Some(false)));
    }

    #[test]
    fn inclusion_is_ai_membership_not_current_control_state() {
        assert!(Inclusion::All.includes(true));
        assert!(Inclusion::All.includes(false));
        assert!(Inclusion::Ai.includes(true));
        assert!(!Inclusion::Ai.includes(false));
        assert!(Inclusion::Human.includes(false));
        assert!(!Inclusion::Human.includes(true));
    }
}
