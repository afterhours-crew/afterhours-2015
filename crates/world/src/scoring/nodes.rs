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
/// `OutputValue` of a participant property accessor (hash of the name).
pub const OUTPUT_VALUE: u32 = 731_416_433;
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
pub const BOOL: u32 = 2_088_812_747;
pub const ON_TRUE: u32 = 2_861_454_674;
pub const ON_FALSE: u32 = 4_213_085_657;
pub const SET_TRUE: u32 = 2_753_900_881;
pub const SET_FALSE: u32 = 668_205_626;
pub const TOGGLE: u32 = 3_236_481_175;
pub const CONDITION: u32 = 1_800_624_758;
pub const VALUE_IF_TRUE: u32 = 88_991_863;
pub const VALUE_IF_FALSE: u32 = 2_915_912_540;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Publish(u32, Value),
    Emit(u32),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Event(u32),
    PropertyChanged(u32),
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
    /// A Boolean participant property read by an accessor. Free roam on the
    /// owned host has no cop pursuit, so the authored `Value` is published
    /// once and never changes (E916 policy; the native accessor reads the
    /// participant property store).
    ParticipantProperty {
        value: bool,
    },
    /// An integer participant property written through an accessor whose
    /// `Value` is connected. The native server accessor stores the bound
    /// Value for its participant and every later change (thunk `0x1446dcc80`
    /// into `0x144055d20`), and mirrors the stored value in `OutputValue`
    /// (`0x1446dcbb0`; E1000). The output is therefore the last written
    /// value; accessors that only read the store are not modeled.
    IntegerParticipantProperty,
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
    Boolean {
        default: bool,
    },
    Integer {
        default: i32,
        step: i32,
    },
    /// The supported LoadOnCreate=true mode; bind the input before seeding.
    IntegerCache,
    /// Fresh Boolean cache. Resolve InValue through its authored fallback when
    /// unbound or unready; LoadOnCreate=false does not seed OutValue at bind.
    BooleanCache {
        load_on_create: bool,
    },
    CompareBoolean {
        on_change: bool,
        on_start: bool,
        always_send: bool,
    },
    ConditionalFloat,
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
    boolean_relation: Option<bool>,
}

fn float(read: &mut impl FnMut(u32) -> Option<Value>, port: u32) -> Result<f32, Error> {
    match read(port).ok_or(Error::MissingInput)? {
        Value::Float(value) => finite(value),
        _ => Err(Error::InvalidGraph),
    }
}
fn integer(read: &mut impl FnMut(u32) -> Option<Value>, port: u32) -> Result<i32, Error> {
    match read(port).ok_or(Error::MissingInput)? {
        Value::Integer(value) => Ok(value),
        _ => Err(Error::InvalidGraph),
    }
}
fn boolean(read: &mut impl FnMut(u32) -> Option<Value>, port: u32) -> Result<bool, Error> {
    match read(port).ok_or(Error::MissingInput)? {
        Value::Boolean(value) => Ok(value),
        _ => Err(Error::InvalidGraph),
    }
}
fn boolean_event(value: bool) -> Effect {
    Effect::Emit(if value { ON_TRUE } else { ON_FALSE })
}
fn conditional(
    read: &mut impl FnMut(u32) -> Option<Value>,
    condition: bool,
) -> Result<Effect, Error> {
    Ok(Effect::Publish(
        OUTPUT,
        Value::Float(float(
            read,
            if condition {
                VALUE_IF_TRUE
            } else {
                VALUE_IF_FALSE
            },
        )?),
    ))
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
            pending: matches!(
                definition,
                Definition::Compare { on_start: true, .. }
                    | Definition::CompareBoolean { on_start: true, .. }
            ),
            relation: None,
            boolean_relation: None,
        })
    }

    /// Seed output properties during binding, without firing events. Cache
    /// initial output is its *bound* input, not necessarily the authored default.
    /// Call once for a fresh node, before activation or event delivery.
    /// Boolean-cache inputs use their authored fallback when unbound or unready;
    /// the caller supplies that resolved value. No-load mode does not read it.
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
            Definition::ParticipantProperty { value } => {
                vec![Effect::Publish(OUTPUT_VALUE, Value::Boolean(value))]
            }
            Definition::IntegerParticipantProperty => vec![Effect::Publish(
                OUTPUT_VALUE,
                Value::Integer(integer(&mut read, FLOAT_VALUE)?),
            )],
            Definition::Float { default, .. } => {
                vec![Effect::Publish(FLOAT_VALUE, Value::Float(default))]
            }
            Definition::Boolean { default } => {
                vec![Effect::Publish(FLOAT_VALUE, Value::Boolean(default))]
            }
            Definition::Integer { default, .. } => {
                vec![Effect::Publish(FLOAT_VALUE, Value::Integer(default))]
            }
            Definition::IntegerCache => vec![Effect::Publish(
                CACHE_OUTPUT,
                Value::Integer(integer(&mut read, CACHE_INPUT)?),
            )],
            Definition::BooleanCache {
                load_on_create: true,
            } => vec![Effect::Publish(
                CACHE_OUTPUT,
                Value::Boolean(boolean(&mut read, CACHE_INPUT)?),
            )],
            Definition::ConditionalFloat => vec![Effect::Publish(OUTPUT, Value::Float(0.0))],
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
            (BooleanCache { .. }, Event(LOAD)) => Ok(vec![
                Publish(CACHE_OUTPUT, Value::Boolean(boolean(read, CACHE_INPUT)?)),
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
            // The output tracks the stored sum plus the current input between
            // events: the official host's category score (fed by the gated
            // drift score) rises every frame while the drift runs and holds
            // the folded sum afterwards (E914 ghost 598). No event is emitted
            // for a tracked change; Accumulate and Reset emit as before.
            (Accumulator, PropertyChanged(INPUT)) => Ok(vec![Publish(
                OUTPUT,
                Value::Float(finite(float(read, INPUT)? + self.sum)?),
            )]),
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
                PropertyChanged(_),
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
            (Boolean { .. }, Event(event @ (SET_TRUE | SET_FALSE | TOGGLE))) => {
                let value = match event {
                    SET_TRUE => true,
                    SET_FALSE => false,
                    _ => !boolean(read, FLOAT_VALUE)?,
                };
                Ok(vec![Publish(FLOAT_VALUE, Value::Boolean(value))])
            }
            (Integer { default, step }, Event(event @ (INCREMENT | DECREMENT | RESET))) => {
                let value = match event {
                    RESET => default,
                    INCREMENT => integer(read, FLOAT_VALUE)?.wrapping_add(step),
                    _ => integer(read, FLOAT_VALUE)?.wrapping_sub(step),
                };
                Ok(vec![Publish(FLOAT_VALUE, Value::Integer(value))])
            }
            (IntegerParticipantProperty, PropertyChanged(FLOAT_VALUE)) => Ok(vec![Publish(
                OUTPUT_VALUE,
                Value::Integer(integer(read, FLOAT_VALUE)?),
            )]),
            (IntegerCache, Event(LOAD)) => Ok(vec![
                Publish(CACHE_OUTPUT, Value::Integer(integer(read, CACHE_INPUT)?)),
                Emit(CACHE_LOADED),
            ]),
            (CompareBoolean { .. }, Event(_)) => Ok(vec![boolean_event(boolean(read, BOOL)?)]),
            (
                CompareBoolean {
                    on_change: true,
                    always_send,
                    ..
                },
                PropertyChanged(_),
            ) => {
                if !self.active {
                    self.pending = true;
                    return Ok(vec![]);
                }
                let current = boolean(read, BOOL)?;
                let previous = self.boolean_relation.replace(current);
                Ok(if always_send || previous != Some(current) {
                    vec![boolean_event(current)]
                } else {
                    vec![]
                })
            }
            (CompareBoolean { .. }, Activate) => {
                self.active = true;
                if !self.pending {
                    return Ok(vec![]);
                }
                let current = boolean(read, BOOL)?;
                self.pending = false;
                self.boolean_relation = Some(current);
                Ok(vec![boolean_event(current)])
            }
            (ConditionalFloat, Activate | PropertyChanged(CONDITION)) => {
                let condition = boolean(read, CONDITION)?;
                Ok(vec![conditional(read, condition)?])
            }
            (ConditionalFloat, PropertyChanged(port @ (VALUE_IF_TRUE | VALUE_IF_FALSE))) => {
                let condition = boolean(read, CONDITION)?;
                Ok(if condition == (port == VALUE_IF_TRUE) {
                    vec![conditional(read, condition)?]
                } else {
                    vec![]
                })
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

/// Fold the ordered *bound* inputs of And/Or. Remove unconnected ports during
/// binding; None here means a bound property is not ready. And aborts without
/// publishing on such a property, unless an earlier false already decided it.
/// Or skips unavailable inputs. Both short-circuit; the empty identities are
/// true/false. No property event is generated by this calculation itself.
pub fn fold_boolean(all: bool, inputs: &[Option<bool>]) -> Result<Option<bool>, Error> {
    fold_boolean_by(all, inputs.len(), |index| Ok(inputs[index]))
}
pub(crate) fn fold_boolean_by(
    all: bool,
    count: usize,
    mut read: impl FnMut(usize) -> Result<Option<bool>, Error>,
) -> Result<Option<bool>, Error> {
    if count > 256 {
        return Err(Error::Bound);
    }
    for index in 0..count {
        match read(index)? {
            Some(value) if value != all => return Ok(Some(!all)),
            None if all => return Ok(None),
            _ => {}
        }
    }
    Ok(Some(all))
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
        Value::Vec3(_) | Value::Vec4(_) | Value::Transform(_) => return Err(Error::InvalidGraph),
    };
    Ok([
        Effect::Publish(936_870_923, Value::Float(float)),
        Effect::Publish(245_372_264, Value::Integer(integer)),
        Effect::Publish(3_802_618_965, Value::Boolean(boolean)),
    ])
}

#[cfg(test)]
mod tests;
