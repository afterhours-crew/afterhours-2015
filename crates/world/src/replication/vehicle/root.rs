// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Quaternion25, RootInitial, Update, Vector};
use crate::replication::{MAX_OBJECTS, entity::Rpc};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bus {
    pub path: Vec<u16>,
    pub flags: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Blueprint(Vec<Bus>);
impl Blueprint {
    pub fn new(buses: Vec<Bus>) -> Result<Self, Error> {
        if buses.is_empty() || buses.len() > MAX_OBJECTS {
            return Err(Error::Bound);
        }
        if !buses[0].path.is_empty()
            || buses.iter().any(|b| b.path.len() >= 32)
            || buses.windows(2).any(|v| v[0].path >= v[1].path)
        {
            return Err(Error::Shape);
        }
        if buses[0].flags & 1 == 0 {
            return Err(Error::Unsupported);
        }
        Ok(Self(buses))
    }
    pub fn instantiate(&self) -> Result<State, Error> {
        let mut state = State::new(Some(1))?;
        for (index, bus) in self.0.iter().enumerate().skip(1) {
            if bus.flags & 1 != 0 {
                state.register_bus(index as u64 + 1)?;
            }
        }
        Ok(state)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PropertyOwner {
    buses: BTreeMap<u64, u16>,
}
impl PropertyOwner {
    pub fn new(root_bus: u64) -> Result<Self, Error> {
        let mut state = Self {
            buses: BTreeMap::new(),
        };
        state.register(root_bus)?;
        Ok(state)
    }
    pub fn register(&mut self, bus: u64) -> Result<u16, Error> {
        if bus == 0 {
            return Err(Error::Shape);
        }
        if let Some(&index) = self.buses.get(&bus) {
            return Ok(index);
        }
        if self.buses.len() >= MAX_OBJECTS {
            return Err(Error::Bound);
        }
        let index = u16::try_from(self.buses.len() + 1).map_err(|_| Error::Bound)?;
        self.buses.insert(bus, index);
        Ok(index)
    }
    pub fn count(&self) -> u16 {
        self.buses.len() as u16
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    property: Option<PropertyOwner>,
    player_selector: Option<u8>,
    connection_override: u16,
    entry_restriction_bound: bool,
    customization_attached: bool,
    team: Option<u8>,
}
impl State {
    pub fn new(property_root_bus: Option<u64>) -> Result<Self, Error> {
        Ok(Self {
            property: property_root_bus.map(PropertyOwner::new).transpose()?,
            player_selector: None,
            connection_override: 0,
            entry_restriction_bound: false,
            customization_attached: false,
            team: None,
        })
    }
    pub fn register_bus(&mut self, bus: u64) -> Result<u16, Error> {
        self.property
            .as_mut()
            .ok_or(Error::Unsupported)?
            .register(bus)
    }
    pub fn set_player(&mut self, selector: Option<u8>) {
        self.player_selector = selector;
    }
    pub fn set_connection_override(&mut self, connection: u16) {
        self.connection_override = connection;
    }
    pub fn set_team(&mut self, team: u8) -> Result<(), Error> {
        if team > 16 {
            return Err(Error::Bound);
        }
        self.team = Some(team);
        Ok(())
    }
    pub fn update(&self, changed: bool) -> Result<Update, Error> {
        Ok(Update::Root(if changed {
            Some(self.team.ok_or(Error::Unsupported)?)
        } else {
            None
        }))
    }
    pub fn connection_id(&self) -> u16 {
        if self.connection_override != 0 {
            self.connection_override
        } else {
            self.player_selector.map_or(0, u16::from)
        }
    }
    pub fn bind_entry_restriction(&mut self) {
        self.entry_restriction_bound = true;
    }
    pub fn attach_customization(&mut self) {
        self.customization_attached = true;
    }
    pub fn initial(
        &self,
        position: [f32; 3],
        basis: [[f32; 3]; 3],
        origin: [f32; 3],
        rpc: Rpc,
    ) -> Result<RootInitial, Error> {
        let rotation = Quaternion25::from_basis(basis)?;
        Ok(RootInitial {
            property_value: self.property.as_ref().map(PropertyOwner::count),
            position: Vector::from_position(position, origin, 5)?,
            rotation: rotation.clone(),
            flag: self.entry_restriction_bound,
            reference: None,
            rpc,
            fine_position: Vector::from_position(position, origin, 10)?,
            fine_rotation: rotation,
            fine_flag: self.customization_attached,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_protocol::world::rpc::Serial;
    const BASIS: [[f32; 3]; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
    fn rpc() -> Rpc {
        Rpc {
            selector: 0,
            serial: Serial::new(1).unwrap(),
        }
    }

    #[test]
    fn team_requires_static_initialization_and_is_independent_of_connection() {
        let mut a = State::new(Some(1)).unwrap();
        let b = a.clone();
        assert_eq!(a.update(false), Ok(Update::Root(None)));
        assert_eq!(a.update(true), Err(Error::Unsupported));
        a.set_team(16).unwrap();
        a.set_player(Some(3));
        a.set_connection_override(500);
        assert_eq!(a.update(true), Ok(Update::Root(Some(16))));
        assert_eq!(a.connection_id(), 500);
        assert_eq!(a.set_team(17), Err(Error::Bound));
        assert_eq!(a.update(true), Ok(Update::Root(Some(16))));
        assert_eq!(b.update(true), Err(Error::Unsupported));
        a.set_team(0).unwrap();
        assert_eq!(a.update(true), Ok(Update::Root(Some(0))));
    }

    #[test]
    fn blueprint_uses_flags_and_instance_paths_without_global_bus_ids() {
        let b = Blueprint::new(vec![
            Bus {
                path: vec![],
                flags: 15,
            },
            Bus {
                path: vec![0, 1],
                flags: 14,
            },
            Bus {
                path: vec![0, 2],
                flags: 15,
            },
            Bus {
                path: vec![0, 3],
                flags: 15,
            },
        ])
        .unwrap();
        let mut s = b.instantiate().unwrap();
        assert_eq!(
            s.initial([0.; 3], BASIS, [0.; 3], rpc())
                .unwrap()
                .property_value,
            Some(3)
        );
        assert_eq!(s.register_bus(3), Ok(2));
        assert_eq!(b.instantiate().unwrap().connection_id(), 0);
        assert_eq!(Blueprint::new(vec![]), Err(Error::Bound));
        assert_eq!(
            Blueprint::new(vec![Bus {
                path: vec![],
                flags: 14
            }]),
            Err(Error::Unsupported)
        );
        assert_eq!(
            Blueprint::new(vec![Bus {
                path: vec![0],
                flags: 15
            }]),
            Err(Error::Shape)
        );
        assert_eq!(
            Blueprint::new(vec![
                Bus {
                    path: vec![],
                    flags: 15
                },
                Bus {
                    path: vec![],
                    flags: 15
                }
            ]),
            Err(Error::Shape)
        );
    }

    #[test]
    fn registration_is_bounded_retry_safe_and_owner_local() {
        let mut p = PropertyOwner::new(10).unwrap();
        assert_eq!(p.register(10), Ok(1));
        assert_eq!(p.register(20), Ok(2));
        assert_eq!(p.count(), 2);
        assert_eq!(PropertyOwner::new(20).unwrap().count(), 1);
        assert_eq!(PropertyOwner::new(0), Err(Error::Shape));
        for bus in 21..21 + MAX_OBJECTS as u64 - 2 {
            p.register(bus).unwrap();
        }
        let before = p.clone();
        assert_eq!(p.register(u64::MAX), Err(Error::Bound));
        assert_eq!(p.register(0), Err(Error::Shape));
        assert_eq!(p, before);
    }
    #[test]
    fn connection_tracks_current_player_and_override_precedence() {
        let mut s = State::new(None).unwrap();
        assert_eq!(s.connection_id(), 0);
        s.set_player(Some(7));
        assert_eq!(s.connection_id(), 7);
        s.set_connection_override(900);
        s.set_player(Some(8));
        assert_eq!(s.connection_id(), 900);
        s.set_connection_override(0);
        assert_eq!(s.connection_id(), 8);
        s.set_player(None);
        assert_eq!(s.connection_id(), 0);
        assert_eq!(s.register_bus(1), Err(Error::Unsupported));
    }
    #[test]
    fn root_flags_latch_and_both_positions_share_current_pose() {
        let mut s = State::new(Some(1)).unwrap();
        let before = s.initial([1., 2., 3.], BASIS, [0.; 3], rpc()).unwrap();
        assert_eq!(before.property_value, Some(1));
        assert!(!before.flag && !before.fine_flag);
        s.register_bus(2).unwrap();
        s.bind_entry_restriction();
        s.attach_customization();
        s.attach_customization();
        let v = s.initial([2., 3., 4.], BASIS, [1.; 3], rpc()).unwrap();
        assert_eq!(v.property_value, Some(2));
        assert!(v.flag && v.fine_flag);
        assert_eq!(v.position, before.position);
        assert_eq!(v.fine_position, before.fine_position);
        assert_eq!(v.rotation, v.fine_rotation);
        assert_eq!(v.reference, None);
        assert!(
            s.initial([f32::NAN, 0., 0.], BASIS, [0.; 3], rpc())
                .is_err()
        );
        assert!(s.initial([0.; 3], [[0.; 3]; 3], [0.; 3], rpc()).is_err());
        assert_eq!(
            State::new(None)
                .unwrap()
                .initial([0.; 3], BASIS, [0.; 3], rpc())
                .unwrap()
                .property_value,
            None
        );
    }
}
