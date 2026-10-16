// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Transient scalar node state. Effects must be applied synchronously in order,
//! after retaining the new node state, inside the enclosing graph transaction.
//! These nodes neither propagate properties nor commit REP on their own.
use super::{Error, finite, math::Value};
use std::cmp::Ordering;

pub const IN: u32 = 5_862_146;
pub const OUT: u32 = 193_453_899;
pub const LOAD: u32 = 2_089_019_715;
pub const CACHE_INPUT: u32 = 1_658_462_601;
pub const CACHE_OUTPUT: u32 = 993_810_272;
pub const CACHE_LOADED: u32 = 170_242_381;
pub const ACCUMULATE: u32 = 1_034_803_477;
pub const RESET: u32 = 229_946_160;
pub const INPUT: u32 = 214_522_259;
pub const OUTPUT: u32 = 2_895_736_442;
pub const OUTPUT_CHANGED: u32 = 2_625_023_801;
pub const INCREMENT: u32 = 0x6788_1204;
pub const DECREMENT: u32 = 0xe86a_d562;
pub const FLOAT_VALUE: u32 = 225_375_086;
pub const OPEN: u32 = 0x7c83_beb1;
pub const CLOSE: u32 = 0x0cac_8853;
pub const SPLITTER_OUTPUT: u32 = 4_157_476_991;
pub const A: u32 = 177_636;
pub const B: u32 = 177_639;
pub const GREATER: u32 = 0x0b87_bdd8;
pub const GREATER_EQUAL: u32 = 0x7c7f_72c5;
pub const EQUAL: u32 = 0x0b87_bd3b;
pub const NOT_EQUAL: u32 = 0x7c7f_159a;
pub const LESS_EQUAL: u32 = 0x7c7f_6a47;
pub const LESS: u32 = 0x0b87_bd1a;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Publish(u32, Value),
    Emit(u32),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Event(u32),
    PropertyChanged,
    Activate,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Comparison {
    Float,
    Integer,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Definition {
    Cache,
    Accumulator,
    Float {
        default: f32,
        step: f32,
    },
    Gate {
        open: bool,
    },
    Splitter {
        run_once: bool,
    },
    Compare {
        kind: Comparison,
        on_change: bool,
        on_start: bool,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    definition: Definition,
    sum: f32,
    open: bool,
    fired: bool,
    active: bool,
    pending: bool,
    relation: Option<Ordering>,
}

fn float(read: &mut impl FnMut(u32) -> Option<Value>, port: u32) -> Result<f32, Error> {
    match read(port).ok_or(Error::MissingInput)? {
        Value::Float(value) => finite(value),
        _ => Err(Error::InvalidGraph),
    }
}
fn relation(
    kind: Comparison,
    read: &mut impl FnMut(u32) -> Option<Value>,
) -> Result<Ordering, Error> {
    match kind {
        Comparison::Float => Ok(float(read, A)?
            .partial_cmp(&float(read, B)?)
            .ok_or(Error::NonFinite)?),
        Comparison::Integer => match (read(A), read(B)) {
            (Some(Value::Integer(a)), Some(Value::Integer(b))) => Ok(a.cmp(&b)),
            (None, _) | (_, None) => Err(Error::MissingInput),
            _ => Err(Error::InvalidGraph),
        },
    }
}
fn predicates(kind: Comparison, relation: Ordering) -> Vec<u32> {
    use Ordering::*;
    let mut events = Vec::with_capacity(3);
    if relation == Greater {
        events.push(GREATER);
    }
    if relation != Less {
        events.push(GREATER_EQUAL);
    }
    if kind == Comparison::Integer {
        events.push(if relation == Equal { EQUAL } else { NOT_EQUAL });
    }
    if relation != Greater {
        events.push(LESS_EQUAL);
    }
    if relation == Less {
        events.push(LESS);
    }
    events
}

impl Node {
    /// Fresh composition state, without an engine save-state restore. Binding
    /// and activation are separate: startup comparisons run after inputs bind.
    pub fn new(definition: Definition) -> Result<Self, Error> {
        if let Definition::Float { default, step } = definition {
            finite(default)?;
            finite(step)?;
        }
        Ok(Self {
            definition,
            sum: 0.0,
            open: matches!(definition, Definition::Gate { open: true }),
            fired: false,
            active: false,
            pending: matches!(definition, Definition::Compare { on_start: true, .. }),
            relation: None,
        })
    }

    /// Seed output properties during binding, without firing events. Cache
    /// initial output is its *bound* input, not necessarily the authored default.
    /// Call once for a fresh node, before activation or event delivery.
    pub fn initial_outputs(
        &self,
        mut read: impl FnMut(u32) -> Option<Value>,
    ) -> Result<Vec<Effect>, Error> {
        Ok(match self.definition {
            Definition::Cache => vec![Effect::Publish(
                CACHE_OUTPUT,
                Value::Float(float(&mut read, CACHE_INPUT)?),
            )],
            Definition::Accumulator => vec![Effect::Publish(OUTPUT, Value::Float(0.0))],
            Definition::Float { default, .. } => {
                vec![Effect::Publish(FLOAT_VALUE, Value::Float(default))]
            }
            _ => vec![],
        })
    }

    /// At most three ordered effects. Missing bindings, wrong types and numeric
    /// errors leave this node unchanged. The caller must also discard the whole
    /// graph candidate if applying any effect fails. Never coalesce equal-value
    /// publications or repeated explicit events here.
    pub fn apply(
        &mut self,
        action: Action,
        mut read: impl FnMut(u32) -> Option<Value>,
    ) -> Result<Vec<Effect>, Error> {
        let mut candidate = self.clone();
        let effects = candidate.apply_inner(action, &mut read)?;
        *self = candidate;
        Ok(effects)
    }

    fn apply_inner(
        &mut self,
        action: Action,
        read: &mut impl FnMut(u32) -> Option<Value>,
    ) -> Result<Vec<Effect>, Error> {
        use Action::*;
        use Definition::*;
        use Effect::*;
        match (self.definition, action) {
            (Cache, Event(LOAD)) => Ok(vec![
                Publish(CACHE_OUTPUT, Value::Float(float(read, CACHE_INPUT)?)),
                Emit(CACHE_LOADED),
            ]),
            (Accumulator, Event(event @ (ACCUMULATE | RESET))) => {
                self.sum = if event == RESET {
                    0.0
                } else {
                    finite(float(read, INPUT)? + self.sum)?
                };
                Ok(vec![
                    Publish(OUTPUT, Value::Float(self.sum)),
                    Emit(OUTPUT_CHANGED),
                ])
            }
            (Float { default, step }, Event(event @ (INCREMENT | DECREMENT | RESET))) => {
                let value = match event {
                    RESET => default,
                    INCREMENT => finite(float(read, FLOAT_VALUE)? + step)?,
                    _ => finite(float(read, FLOAT_VALUE)? - step)?,
                };
                Ok(vec![Publish(FLOAT_VALUE, Value::Float(value))])
            }
            (Gate { .. }, Event(OPEN)) => {
                self.open = true;
                Ok(vec![])
            }
            (Gate { .. }, Event(CLOSE)) => {
                self.open = false;
                Ok(vec![])
            }
            (Gate { .. }, Event(IN)) if self.open => Ok(vec![Emit(OUT)]),
            (Splitter { run_once }, Event(_)) if !run_once || !self.fired => {
                self.fired = true;
                Ok(vec![Emit(SPLITTER_OUTPUT)])
            }
            (Compare { kind, .. }, Event(_)) => Ok(predicates(kind, relation(kind, read)?)
                .into_iter()
                .map(Emit)
                .collect()),
            (
                Compare {
                    kind,
                    on_change: true,
                    ..
                },
                PropertyChanged,
            ) => {
                let current = relation(kind, read)?;
                if self.active {
                    Ok(self.transition(kind, current))
                } else {
                    self.pending = true;
                    Ok(vec![])
                }
            }
            (Compare { kind, .. }, Activate) => {
                self.active = true;
                if !self.pending {
                    return Ok(vec![]);
                }
                let current = relation(kind, read)?;
                self.pending = false;
                Ok(self.transition(kind, current))
            }
            _ => Ok(vec![]),
        }
    }

    fn transition(&mut self, kind: Comparison, current: Ordering) -> Vec<Effect> {
        let previous = self
            .relation
            .replace(current)
            .map(|old| predicates(kind, old))
            .unwrap_or_default();
        predicates(kind, current)
            .into_iter()
            .filter(|event| !previous.contains(event))
            .map(Effect::Emit)
            .collect()
    }
}

/// Numeric PropertyCast publication order only. String input/output and its
/// formatting are unsupported; this is not a complete PropertyCast interpreter.
/// Reject the native conversion's indefinite/out-of-range results before any
/// publication so invalid values cannot turn into a score.
pub fn cast_numeric(input: Value) -> Result<[Effect; 3], Error> {
    let (float, integer, boolean) = match input {
        Value::Integer(value) => (value as f32, value, value != 0),
        Value::Boolean(value) => (u8::from(value) as f32, i32::from(value), value),
        Value::Float(value) => {
            finite(value)?;
            if !(-2_147_483_648.0..2_147_483_648.0).contains(&value) {
                return Err(Error::ConversionRange);
            }
            (value, value as i32, value != 0.0)
        }
    };
    Ok([
        Effect::Publish(936_870_923, Value::Float(float)),
        Effect::Publish(245_372_264, Value::Integer(integer)),
        Effect::Publish(3_802_618_965, Value::Boolean(boolean)),
    ])
}

#[cfg(test)]
mod tests;
