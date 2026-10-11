// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, NibblesGuid, Tagged, Update};
use crate::replication::MAX_OBJECTS;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Effects([u8; 3]);
impl Effects {
    pub fn set(&mut self, values: [u8; 3]) -> Result<(), Error> {
        if values.iter().any(|v| *v > 15) {
            return Err(Error::Bound);
        }
        self.0 = values;
        Ok(())
    }
    pub fn update(&self, changed: bool) -> Update {
        Update::TripleNibbles(changed.then_some(self.0))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlayerEntry {
    pub flag: bool,
}
impl PlayerEntry {
    pub fn update(self, changed: bool) -> Update {
        Update::Bool(changed.then_some(self.flag))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockOn(Tagged);
impl Default for LockOn {
    fn default() -> Self {
        Self(Tagged {
            tag: 0,
            reference: None,
            pair: None,
        })
    }
}
impl LockOn {
    pub fn set(
        &mut self,
        tag: u8,
        reference: Option<u16>,
        pair: Option<[f32; 2]>,
    ) -> Result<(), Error> {
        if tag > 7 || reference.is_some_and(|id| usize::from(id) > MAX_OBJECTS) {
            return Err(Error::Bound);
        }
        if (tag == 3) != reference.is_some() || (3..=5).contains(&tag) != pair.is_some() {
            return Err(Error::Shape);
        }
        if pair.is_some_and(|p| p.iter().any(|v| !v.is_finite())) {
            return Err(Error::Bound);
        }
        self.0 = Tagged {
            tag,
            reference,
            pair: pair.map(|p| p.map(f32::to_bits)),
        };
        Ok(())
    }
    pub fn update(&self, changed: bool) -> Update {
        Update::Tagged(changed.then(|| self.0.clone()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Audio(NibblesGuid);
impl Default for Audio {
    fn default() -> Self {
        Self(NibblesGuid {
            head: [0; 3],
            guid: [0; 4],
            tail: [0; 6],
        })
    }
}
impl Audio {
    pub fn set(&mut self, head: [u8; 3], guid: [u32; 4], tail: [u8; 6]) -> Result<(), Error> {
        if head.iter().chain(tail.iter()).any(|v| *v > 15) {
            return Err(Error::Bound);
        }
        self.0 = NibblesGuid { head, guid, tail };
        Ok(())
    }
    pub fn update(&self, changed: bool) -> Update {
        Update::NibblesGuid(changed.then(|| self.0.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::vehicle::{Body, Kind, Profile};
    #[test]
    fn fresh_components_have_native_defaults_and_clean_fields_are_distinct() {
        let profile = Profile::new(vec![
            Kind::Root {
                property_owner: false,
            },
            Kind::TripleNibbles,
            Kind::Bool,
            Kind::Tagged,
            Kind::NibblesGuid,
        ])
        .unwrap();
        let full = Body {
            creation: None,
            updates: vec![
                None,
                Some(Effects::default().update(true)),
                Some(PlayerEntry::default().update(true)),
                Some(LockOn::default().update(true)),
                Some(Audio::default().update(true)),
            ],
        };
        let wire = full.encode(&profile).unwrap();
        assert_eq!(wire.len(), 189);
        assert_eq!(
            Body::decode(wire.span(), &profile, false).unwrap().body,
            full
        );
        assert_eq!(
            Effects::default().update(true),
            Update::TripleNibbles(Some([0; 3]))
        );
        assert_eq!(
            PlayerEntry::default().update(true),
            Update::Bool(Some(false))
        );
        assert_eq!(
            LockOn::default().update(true),
            Update::Tagged(Some(Tagged {
                tag: 0,
                reference: None,
                pair: None
            }))
        );
        assert_eq!(
            Audio::default().update(true),
            Update::NibblesGuid(Some(NibblesGuid {
                head: [0; 3],
                guid: [0; 4],
                tail: [0; 6]
            }))
        );
        assert_eq!(
            Effects::default().update(false),
            Update::TripleNibbles(None)
        );
        assert_eq!(PlayerEntry { flag: true }.update(false), Update::Bool(None));
        assert_eq!(LockOn::default().update(false), Update::Tagged(None));
        assert_eq!(Audio::default().update(false), Update::NibblesGuid(None));
    }
    #[test]
    fn effects_and_audio_reject_partial_changes_and_remain_per_instance() {
        let mut effects = Effects::default();
        effects.set([15, 1, 3]).unwrap();
        assert_eq!(effects.set([4, 5, 16]), Err(Error::Bound));
        assert_eq!(
            effects.update(true),
            Update::TripleNibbles(Some([15, 1, 3]))
        );
        assert_eq!(
            Effects::default().update(true),
            Update::TripleNibbles(Some([0; 3]))
        );
        let mut audio = Audio::default();
        audio
            .set([1, 2, 15], [8, 9, 10, 11], [15, 0, 1, 2, 3, 4])
            .unwrap();
        let saved = audio.clone();
        assert_eq!(audio.set([16, 0, 0], [0; 4], [0; 6]), Err(Error::Bound));
        assert_eq!(
            audio.set([0; 3], [0; 4], [0, 0, 0, 0, 0, 255]),
            Err(Error::Bound)
        );
        assert_eq!(audio, saved);
        assert_ne!(audio, Audio::default());
    }
    #[test]
    fn lock_on_state_branches_preserve_reference_and_pair_shape_transactionally() {
        let mut state = LockOn::default();
        for tag in 0..8 {
            let reference = (tag == 3).then_some(8191);
            let pair = (3..=5).contains(&tag).then_some([-0., 1.25]);
            state.set(tag, reference, pair).unwrap();
            assert_eq!(
                state.update(true),
                Update::Tagged(Some(Tagged {
                    tag,
                    reference,
                    pair: pair.map(|p| p.map(f32::to_bits))
                }))
            );
        }
        let saved = state.clone();
        for (tag, reference, pair) in [
            (8, None, None),
            (3, None, Some([0.; 2])),
            (4, Some(1), Some([0.; 2])),
            (5, None, None),
            (0, None, Some([0.; 2])),
            (3, Some(8192), Some([0.; 2])),
            (4, None, Some([f32::NAN, 0.])),
        ] {
            assert!(state.set(tag, reference, pair).is_err());
            assert_eq!(state, saved);
        }
        state.set(3, Some(0), Some([0.; 2])).unwrap();
        let p = Profile::new(vec![
            Kind::Root {
                property_owner: false,
            },
            Kind::Tagged,
        ])
        .unwrap();
        let b = Body {
            creation: None,
            updates: vec![None, Some(state.update(true))],
        };
        let wire = b.encode(&p).unwrap();
        assert_eq!(Body::decode(wire.span(), &p, false).unwrap().body, b);
        assert_ne!(state, LockOn::default());
    }
}
