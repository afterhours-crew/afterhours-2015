// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The scoped burnout tendency override (native 04737100 / 04736760).
//! Vehicle identities are host-resolved chassis instances, never item IDs.
use super::{Error, finite};

pub const ENABLED: u32 = 0x0028_a000;
pub const VALUE: u32 = 0x0d6e_f36e;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    pub vehicle: u32,
    /// None clears both the enabled bit and value, rather than restoring a
    /// guessed previous override. The native component deduplicates the write.
    pub value: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    enabled: bool,
    vehicle: Option<u32>,
}

impl State {
    pub fn bind(
        enabled: bool,
        vehicle: Option<u32>,
        value: f32,
    ) -> Result<(Self, Vec<Change>), Error> {
        let mut state = Self {
            enabled,
            vehicle: None,
        };
        let changes = state.resolve(vehicle, value)?;
        Ok((state, changes))
    }

    /// Resolve the provider before a property callback, or when it reports a
    /// vehicle change. Removing a live enabled vehicle also clears the latch.
    pub fn resolve(&mut self, vehicle: Option<u32>, value: f32) -> Result<Vec<Change>, Error> {
        finite(value)?;
        if vehicle == self.vehicle {
            return Ok(Vec::new());
        }
        let mut changes = Vec::new();
        if self.enabled {
            if let Some(old) = self.vehicle {
                changes.push(Change {
                    vehicle: old,
                    value: None,
                });
                self.enabled = false;
            }
            // The native branch tests enabled once, before releasing the old
            // car. Replacement within the same notification still applies.
            if let Some(new) = vehicle {
                changes.push(Change {
                    vehicle: new,
                    value: Some(value),
                });
                self.enabled = true;
            }
        }
        self.vehicle = vehicle;
        Ok(changes)
    }

    pub fn changed(
        &mut self,
        field: u32,
        enabled: bool,
        value: f32,
        vehicle: Option<u32>,
    ) -> Result<Vec<Change>, Error> {
        let mut changes = self.resolve(vehicle, value)?;
        let Some(vehicle) = self.vehicle else {
            return Ok(changes);
        };
        match field {
            ENABLED => {
                self.enabled = enabled;
                changes.push(Change {
                    vehicle,
                    value: enabled.then_some(value),
                });
            }
            VALUE if self.enabled => changes.push(Change {
                vehicle,
                value: Some(value),
            }),
            _ => (),
        }
        Ok(changes)
    }

    pub fn release(&mut self) -> Option<Change> {
        let result = self.vehicle.filter(|_| self.enabled).map(|vehicle| Change {
            vehicle,
            value: None,
        });
        self.enabled = false;
        self.vehicle = None;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn change(vehicle: u32, value: Option<f32>) -> Change {
        Change { vehicle, value }
    }
    #[test]
    fn replace_clear_reacquire_and_retire_follow_scoped_native_latch() {
        let (mut state, changes) = State::bind(true, Some(10), 0.75).unwrap();
        assert_eq!(changes, [change(10, Some(0.75))]);
        assert!(state.resolve(Some(10), 1.).unwrap().is_empty());
        assert_eq!(
            state.resolve(Some(20), 1.).unwrap(),
            [change(10, None), change(20, Some(1.))]
        );
        assert_eq!(state.resolve(None, 1.).unwrap(), [change(20, None)]);
        assert!(state.resolve(Some(30), 1.).unwrap().is_empty());
        assert!(
            state
                .changed(VALUE, true, 0.5, Some(30))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            state.changed(ENABLED, true, 0.5, Some(30)).unwrap(),
            [change(30, Some(0.5))]
        );
        assert_eq!(state.release(), Some(change(30, None)));
        assert_eq!(state.release(), None);
    }
    #[test]
    fn pending_bind_and_property_callbacks_have_different_missing_car_rules() {
        let (mut state, _) = State::bind(true, None, 2.).unwrap();
        assert!(state.changed(ENABLED, false, 2., None).unwrap().is_empty());
        assert_eq!(state.resolve(Some(10), 2.).unwrap(), [change(10, Some(2.))]);
        assert_eq!(
            state.changed(ENABLED, false, 2., Some(10)).unwrap(),
            [change(10, None)]
        );
        assert!(state.resolve(Some(20), 3.).unwrap().is_empty());
        let before = state.clone();
        assert_eq!(state.resolve(Some(30), f32::NAN), Err(Error::NonFinite));
        assert_eq!(state, before);
    }
}
