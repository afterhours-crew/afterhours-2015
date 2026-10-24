// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Dynamic participant provider and Boolean accessor used by drag's false-start
//! logic. The enclosing owner stores participant values and runs returned writes
//! synchronously; this accessor never duplicates that store.
use super::super::events::Context;

pub const IN: u32 = 0x0059_7302;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Change {
    pub previous: Option<u64>,
    pub current: Option<u64>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Provider {
    participant: Option<u64>,
}
impl Provider {
    pub fn participant(&self) -> Option<u64> {
        self.participant
    }
    /// 04782ac0 accepts only In with a participant event and suppresses the
    /// same selection. The owner moves the destruction subscription accordingly.
    pub fn event(&mut self, event: u32, context: Option<Context>) -> Option<Change> {
        if event != IN {
            return None;
        }
        let Some(Context::Participant(participant)) = context else {
            return None;
        };
        self.select(Some(participant))
    }
    /// 04783d20 clears the selection without the 0472ca70 notification used
    /// by In. Consumers must see the cleared provider on later callbacks, but
    /// must not synthesize an accessor publication or selection notification.
    pub fn destroyed(&mut self, participant: u64) -> bool {
        if self.participant != Some(participant) {
            return false;
        }
        self.participant = None;
        true
    }
    fn select(&mut self, participant: Option<u64>) -> Option<Change> {
        if participant == self.participant {
            return None;
        }
        let previous = self.participant;
        self.participant = participant;
        Some(Change {
            previous,
            current: participant,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    Write { participant: u64, value: bool },
    Publish(bool),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BooleanAccessor {
    default: bool,
    input: Option<bool>,
    participant: Option<u64>,
    active: bool,
    source: bool,
    output: Option<bool>,
}
impl BooleanAccessor {
    pub fn new(default: bool) -> Self {
        Self {
            default,
            input: None,
            participant: None,
            active: false,
            source: false,
            output: None,
        }
    }
    pub fn output(&self) -> Option<bool> {
        self.output
    }
    /// 046dae50/046dc490: a ready input writes to the selected participant;
    /// otherwise an existing stored value is published. An authored fallback
    /// alone is not a reason to write during activation or provider selection.
    pub fn activate(
        &mut self,
        participant: Option<u64>,
        input: Option<bool>,
        stored: Option<bool>,
    ) -> Option<Effect> {
        if self.active {
            return None;
        }
        self.active = true;
        self.source = true;
        self.input = input;
        self.provider_changed(participant, stored)
    }
    pub fn provider_changed(
        &mut self,
        participant: Option<u64>,
        stored: Option<bool>,
    ) -> Option<Effect> {
        self.participant = participant;
        let participant = participant?;
        if !self.active || !self.source {
            return None;
        }
        if let Some(value) = self.input {
            return Some(Effect::Write { participant, value });
        }
        stored.map(|value| self.publish(value))
    }
    /// 046dd2f0 uses the normal Value getter on an actual property callback,
    /// including its authored fallback when that getter is unready.
    pub fn input_changed(&mut self, input: Option<bool>) -> Option<Effect> {
        self.input = input;
        if !self.active || !self.source {
            return None;
        }
        self.participant.map(|participant| Effect::Write {
            participant,
            value: input.unwrap_or(self.default),
        })
    }
    /// 046dc940 compares the changed participant with the current provider.
    /// It publishes the callback value without a second equality filter.
    pub fn stored_changed(&mut self, participant: u64, value: bool) -> Option<Effect> {
        if !self.active || !self.source || self.participant != Some(participant) {
            return None;
        }
        Some(self.publish(value))
    }
    fn publish(&mut self, value: bool) -> Effect {
        self.output = Some(value);
        Effect::Publish(value)
    }
    pub fn source_destroyed(&mut self) {
        self.source = false;
    }
    pub fn provider_destroyed(&mut self) {
        self.participant = None;
    }
    /// Refresh the cached selection after its participant is destroyed. The
    /// native accessor reads the provider on each callback; this is silent.
    pub fn participant_destroyed(&mut self, participant: u64) {
        if self.participant == Some(participant) {
            self.participant = None;
        }
    }
    pub fn retire(&mut self) {
        self.active = false;
        self.source = false;
        self.participant = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drag_driver_selection_writes_only_the_current_driver_and_retains_last_output_on_removal() {
        let mut provider = Provider::default();
        let mut accessor = BooleanAccessor::new(false);
        assert_eq!(accessor.activate(None, None, None), None);
        assert_eq!(provider.event(IN, Some(Context::Integer(7))), None);
        assert_eq!(provider.event(IN + 1, Some(Context::Participant(7))), None);
        assert_eq!(
            provider
                .event(IN, Some(Context::Participant(7)))
                .unwrap()
                .current,
            Some(7)
        );
        assert_eq!(
            accessor.provider_changed(provider.participant(), Some(false)),
            Some(Effect::Publish(false))
        );
        assert_eq!(
            accessor.input_changed(Some(true)),
            Some(Effect::Write {
                participant: 7,
                value: true
            })
        );
        assert_eq!(
            accessor.stored_changed(7, true),
            Some(Effect::Publish(true))
        );
        assert_eq!(provider.event(IN, Some(Context::Participant(7))), None);
        let change = provider.event(IN, Some(Context::Participant(8))).unwrap();
        assert_eq!(change.previous, Some(7));
        assert_eq!(
            accessor.provider_changed(change.current, Some(false)),
            Some(Effect::Write {
                participant: 8,
                value: true
            })
        );
        assert_eq!(accessor.stored_changed(7, false), None);
        assert!(!provider.destroyed(7));
        accessor.participant_destroyed(7);
        assert!(provider.destroyed(8));
        assert_eq!(provider.participant(), None);
        accessor.participant_destroyed(8);
        assert_eq!(accessor.stored_changed(8, false), None);
        assert_eq!(accessor.output(), Some(true));
        assert_eq!(accessor.input_changed(Some(false)), None);
    }
    #[test]
    fn unready_activation_and_missing_value_are_distinct_from_a_false_stored_value() {
        let mut accessor = BooleanAccessor::new(true);
        assert_eq!(accessor.activate(Some(7), None, None), None);
        assert_eq!(accessor.output(), None);
        assert_eq!(
            accessor.provider_changed(Some(8), Some(false)),
            Some(Effect::Publish(false))
        );
        // Only an actual input callback reads the fallback as a write.
        assert_eq!(
            accessor.input_changed(None),
            Some(Effect::Write {
                participant: 8,
                value: true
            })
        );
        assert_eq!(accessor.output(), Some(false));
        assert_eq!(
            accessor.stored_changed(8, true),
            Some(Effect::Publish(true))
        );
        assert_eq!(
            accessor.stored_changed(8, true),
            Some(Effect::Publish(true))
        );
    }
    #[test]
    fn ready_activation_waits_for_the_store_callback_and_detached_links_stop_writes() {
        let mut accessor = BooleanAccessor::new(true);
        assert_eq!(accessor.input_changed(Some(false)), None);
        assert_eq!(
            accessor.activate(Some(7), Some(false), Some(true)),
            Some(Effect::Write {
                participant: 7,
                value: false
            })
        );
        assert_eq!(accessor.output(), None);
        assert_eq!(
            accessor.stored_changed(7, false),
            Some(Effect::Publish(false))
        );
        accessor.source_destroyed();
        assert_eq!(accessor.input_changed(Some(true)), None);
        assert_eq!(accessor.provider_changed(Some(8), Some(true)), None);
        assert_eq!(accessor.stored_changed(8, true), None);
        accessor.provider_destroyed();
        accessor.retire();
        assert_eq!(accessor.output(), Some(false));
        assert_eq!(accessor.input_changed(Some(true)), None);
    }
}
