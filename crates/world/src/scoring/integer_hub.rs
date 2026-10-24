// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! IntHub routing (bind 037699b0, change 0376ef40, event 0375ad40).
//! FloatHub uses the same selection rules (03769aa0/0376f0a0/0375ae40).
use super::{Error, integers};

pub const SELECT: u32 = 0x3328_78fb;
pub const MAX_INPUTS: u16 = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Read<T = i32> {
    Unbound,
    Unready,
    Ready(T),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Bind,
    Changed(u32),
    Select(u16),
}

pub fn evaluate(
    count: u16,
    action: Action,
    mut read: impl FnMut(u32) -> Result<Read, Error>,
) -> Result<Option<i32>, Error> {
    if count == 0 || count > MAX_INPUTS {
        return Err(Error::Bound);
    }
    let selector = if matches!(action, Action::Select(_)) {
        Read::Unbound
    } else {
        read(SELECT)?
    };
    evaluate_value(count, action, selector, 0, read)
}

/// Selection is always Int32, independent of the routed property's type.
/// An explicit selection event does not read or update that selector.
pub fn evaluate_value<T: Copy>(
    count: u16,
    action: Action,
    selector: Read,
    zero: T,
    mut read: impl FnMut(u32) -> Result<Read<T>, Error>,
) -> Result<Option<T>, Error> {
    if count == 0 || count > MAX_INPUTS {
        return Err(Error::Bound);
    }
    let selected = |index: i32, read: &mut dyn FnMut(u32) -> Result<Read<T>, Error>| {
        if !(0..i32::from(count)).contains(&index) {
            Ok(Read::Unbound)
        } else {
            read(integers::port("In", index as u16))
        }
    };
    match action {
        Action::Bind => {
            let Read::Ready(index) = selector else {
                return Ok(None);
            };
            Ok(match selected(index, &mut read)? {
                Read::Ready(value) => Some(value),
                _ => None,
            })
        }
        Action::Changed(SELECT) => {
            let Read::Ready(index) = selector else {
                return Ok(None);
            };
            Ok(match selected(index, &mut read)? {
                Read::Ready(value) => Some(value),
                Read::Unbound => Some(zero),
                Read::Unready => None,
            })
        }
        Action::Select(index) => {
            if index >= count {
                return Err(Error::Bound);
            }
            Ok(match selected(i32::from(index), &mut read)? {
                Read::Ready(value) => Some(value),
                Read::Unbound => Some(zero),
                Read::Unready => None,
            })
        }
        Action::Changed(field) => {
            if !(0..count).any(|i| integers::port("In", i) == field) {
                return Err(Error::InvalidGraph);
            }
            if let Read::Ready(index) = selector
                && (!(0..i32::from(count)).contains(&index)
                    || integers::port("In", index as u16) != field)
            {
                return Ok(None);
            }
            Ok(match read(field)? {
                Read::Ready(value) => Some(value),
                _ => None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binding_selection_changes_and_one_shot_events_have_distinct_readiness_rules() {
        let input = integers::port("In", 0);
        for selected_input in [Read::Unbound, Read::Unready, Read::Ready(17)] {
            let read = |field| {
                Ok(if field == SELECT {
                    Read::Ready(0)
                } else {
                    selected_input
                })
            };
            assert_eq!(
                evaluate(2, Action::Bind, read).unwrap(),
                if selected_input == Read::Ready(17) {
                    Some(17)
                } else {
                    None
                }
            );
            for action in [Action::Changed(SELECT), Action::Select(0)] {
                assert_eq!(
                    evaluate(2, action, read).unwrap(),
                    match selected_input {
                        Read::Ready(v) => Some(v),
                        Read::Unbound => Some(0),
                        Read::Unready => None,
                    }
                );
            }
        }
        for select in [Read::Unbound, Read::Unready, Read::Ready(0), Read::Ready(1)] {
            let read = |field| {
                Ok(if field == SELECT {
                    select
                } else {
                    Read::Ready(9)
                })
            };
            assert_eq!(
                evaluate(2, Action::Changed(input), read).unwrap(),
                if select == Read::Ready(1) {
                    None
                } else {
                    Some(9)
                }
            );
        }
        // A SelectIn event does not change InputSelect or latch future routing.
        assert_eq!(
            evaluate(2, Action::Select(1), |field| Ok(if field == SELECT {
                panic!("one-shot selection")
            } else {
                Read::Ready(22)
            }))
            .unwrap(),
            Some(22)
        );
        assert!(evaluate(0, Action::Bind, |_| unreachable!()).is_err());
        assert!(evaluate(2, Action::Select(2), |_| unreachable!()).is_err());
    }
}
