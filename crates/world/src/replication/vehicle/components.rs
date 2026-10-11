// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Initial, Kind, Profile, Update};
use crate::replication::entity::Rpc;
use nfs_protocol::world::rpc::Serial;

pub fn fresh_rpcs(profile: &Profile) -> Vec<Option<Rpc>> {
    let serial = Serial::new(0)
        .expect("zero is a valid serial")
        .next_initialization();
    let mut selector = 0;
    profile
        .kinds()
        .iter()
        .map(|kind| {
            if matches!(
                kind,
                Kind::Part { .. } | Kind::Bool | Kind::FourBit | Kind::Mesh(_)
            ) {
                None
            } else {
                let rpc = Rpc { selector, serial };
                selector += 1;
                Some(rpc)
            }
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Part {
    variants: u16,
    selected: Option<u8>,
}
impl Part {
    pub fn new(variants: u16) -> Result<Self, Error> {
        if variants > 256 {
            return Err(Error::Bound);
        }
        Ok(Self {
            variants,
            selected: (variants != 0).then_some(0),
        })
    }
    pub fn selected(&self) -> Option<u8> {
        self.selected
    }
    pub fn select(&mut self, selected: Option<u8>) -> Result<(), Error> {
        if selected.is_some_and(|v| v == 255 || u16::from(v) >= self.variants) {
            return Err(Error::Bound);
        }
        self.selected = selected;
        Ok(())
    }
    pub fn initial(&self) -> Initial {
        Initial::Part(self.selected != (self.variants != 0).then_some(0))
    }
    pub fn update(&self, networkable: bool, changed: bool) -> Option<Update> {
        let selected = self.selected?;
        networkable.then(|| Update::Part(changed.then_some(u16::from(selected))))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Wheel {
    pub flag: bool,
}
impl Wheel {
    pub fn initial(self, rpc: Rpc) -> Initial {
        Initial::Wheel {
            rpc,
            flag: self.flag,
        }
    }
    pub fn update(self, changed: bool) -> Update {
        Update::Wheel(changed.then_some(self.flag))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_rpc_order_skips_non_rpc_serializers_and_keeps_serials_separate() {
        let p = Profile::new(vec![
            Kind::Root {
                property_owner: true,
            },
            Kind::Chassis,
            Kind::Part { variants: 2 },
            Kind::Bool,
            Kind::Wheel,
            Kind::FourBit,
            Kind::Index,
        ])
        .unwrap();
        let r = fresh_rpcs(&p);
        assert_eq!(
            r.iter()
                .map(|v| v.as_ref().map(|r| r.selector))
                .collect::<Vec<_>>(),
            vec![Some(0), Some(1), None, None, Some(2), None, Some(3)]
        );
        assert!(
            r.iter()
                .flatten()
                .all(|r| r.serial == Serial::new(0).unwrap().next_initialization())
        );
        assert_eq!(
            Wheel::default().initial(r[4].clone().unwrap()),
            Initial::Wheel {
                rpc: r[4].clone().unwrap(),
                flag: false
            }
        );
        assert_eq!(
            Wheel { flag: true }.initial(r[4].clone().unwrap()),
            Initial::Wheel {
                rpc: r[4].clone().unwrap(),
                flag: true
            }
        );
        assert_eq!(fresh_rpcs(&p), r);
    }
    #[test]
    fn part_constructor_selection_and_removal() {
        let mut p = Part::new(2).unwrap();
        assert_eq!(p.selected(), Some(0));
        assert_eq!(p.initial(), Initial::Part(false));
        p.select(Some(1)).unwrap();
        assert_eq!(p.initial(), Initial::Part(true));
        p.select(None).unwrap();
        assert_eq!(p.initial(), Initial::Part(true));
        p.select(Some(0)).unwrap();
        assert_eq!(p.initial(), Initial::Part(false));
        assert_eq!(Part::new(0).unwrap().initial(), Initial::Part(false));
        assert_eq!(Part::new(1).unwrap().selected(), Some(0));
    }
    #[test]
    fn part_bounds_do_not_mutate_state() {
        let mut p = Part::new(2).unwrap();
        for index in [2, 254, 255] {
            assert_eq!(p.select(Some(index)), Err(Error::Bound));
        }
        assert_eq!(p.selected(), Some(0));
        assert_eq!(Part::new(257), Err(Error::Bound));
        assert_eq!(Part::new(0).unwrap().select(Some(0)), Err(Error::Bound));
        assert_eq!(Part::new(256).unwrap().select(Some(255)), Err(Error::Bound));
    }

    #[test]
    fn part_update_distinguishes_absence_networkability_and_clean_selection() {
        let mut p = Part::new(2).unwrap();
        assert_eq!(p.initial(), Initial::Part(false));
        assert_eq!(p.update(true, true), Some(Update::Part(Some(0))));
        assert_eq!(p.update(true, false), Some(Update::Part(None)));
        assert_eq!(p.update(false, true), None);
        p.select(Some(1)).unwrap();
        assert_eq!(p.update(true, true), Some(Update::Part(Some(1))));
        p.select(None).unwrap();
        assert_eq!(p.update(true, true), None);
        assert_eq!(Part::new(0).unwrap().update(true, true), None);
        assert_eq!(Wheel::default().update(true), Update::Wheel(Some(false)));
        assert_eq!(Wheel { flag: true }.update(false), Update::Wheel(None));
        assert_eq!(Wheel { flag: true }.update(true), Update::Wheel(Some(true)));
    }
}
