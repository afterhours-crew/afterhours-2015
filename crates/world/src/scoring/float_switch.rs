// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Named float selection. Property notifications and explicit selection events
//! use distinct native getter keys and fallbacks; selection does not latch.
use super::{Error, finite, value::Value};

pub const OUTPUT: u32 = 0x666e_571b;
pub const SELECTED: u32 = 0x95cb_806b;
pub const MAX_ENTRIES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub property: u32,
    pub event: u32,
    pub fallback: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub entries: Vec<Entry>,
    pub on_change: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Bind,
    Changed(u32),
    Event(u32),
}
impl Definition {
    pub fn validate(&self) -> Result<(), Error> {
        if self.entries.is_empty() || self.entries.len() > MAX_ENTRIES {
            return Err(Error::Bound);
        }
        for entry in &self.entries {
            finite(entry.fallback)?;
        }
        Ok(())
    }
    /// Return the selected value and whether to emit the selection event after
    /// publication. Duplicate names retain the native first-match behavior.
    pub fn evaluate(
        &self,
        action: Action,
        mut read: impl FnMut(u32) -> Result<Option<Value>, Error>,
    ) -> Result<Option<(f32, bool)>, Error> {
        self.validate()?;
        let (field, fallback, emit) = match action {
            Action::Bind => (self.entries[0].property, 0.0, false),
            Action::Changed(field) if self.on_change => {
                if !self.entries.iter().any(|entry| entry.property == field) {
                    return Ok(None);
                }
                (field, 0.0, false)
            }
            Action::Event(event) => {
                let Some(entry) = self.entries.iter().find(|entry| entry.event == event) else {
                    return Ok(None);
                };
                (entry.event, entry.fallback, true)
            }
            _ => return Ok(None),
        };
        let value = match read(field)? {
            Some(Value::Float(value)) => value,
            None => fallback,
            _ => return Err(Error::InvalidGraph),
        };
        Ok(Some((finite(value)?, emit)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constructor_changes_and_selection_use_distinct_getters_and_fallbacks() {
        let mut definition = Definition {
            entries: vec![
                Entry {
                    property: 10,
                    event: 20,
                    fallback: 1.0,
                },
                Entry {
                    property: 11,
                    event: 21,
                    fallback: 2.0,
                },
                Entry {
                    property: 10,
                    event: 20,
                    fallback: 3.0,
                },
            ],
            on_change: true,
        };
        for action in [Action::Bind, Action::Changed(10)] {
            assert_eq!(
                definition.evaluate(action, |_| Ok(None)).unwrap(),
                Some((0.0, false))
            );
            assert_eq!(
                definition
                    .evaluate(action, |key| {
                        assert_eq!(key, 10);
                        Ok(Some(Value::Float(7.0)))
                    })
                    .unwrap(),
                Some((7.0, false))
            );
        }
        assert_eq!(
            definition
                .evaluate(Action::Event(20), |_| Ok(None))
                .unwrap(),
            Some((1.0, true))
        );
        assert_eq!(
            definition
                .evaluate(Action::Event(21), |key| {
                    assert_eq!(key, 21);
                    Ok(Some(Value::Float(9.0)))
                })
                .unwrap(),
            Some((9.0, true))
        );
        // Selecting another entry does not mask later changes on the first.
        assert_eq!(
            definition
                .evaluate(Action::Changed(10), |_| Ok(None))
                .unwrap(),
            Some((0.0, false))
        );
        for action in [Action::Changed(20), Action::Event(10), Action::Event(99)] {
            assert_eq!(
                definition.evaluate(action, |_| unreachable!()).unwrap(),
                None
            );
        }
        definition.on_change = false;
        assert_eq!(
            definition
                .evaluate(Action::Changed(10), |_| unreachable!())
                .unwrap(),
            None
        );
        assert!(
            definition
                .evaluate(Action::Bind, |_| Ok(Some(Value::Integer(1))))
                .is_err()
        );
        assert!(
            definition
                .evaluate(Action::Bind, |_| Ok(Some(Value::Float(f32::NAN))))
                .is_err()
        );
        definition.entries.clear();
        assert!(definition.validate().is_err());
    }
}
