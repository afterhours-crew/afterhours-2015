// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Fresh property gates. Bind outputs before initialization; resolve each input
//! at its publication turn, since earlier callbacks can change later inputs.
use super::{Error, events::PropertyHandler, properties::Kind, value::Value};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Channel {
    Boolean,
    Integer,
    Float,
    Vec3,
    Vec4,
    Transform,
}
pub const CHANNELS: [Channel; 6] = [
    Channel::Boolean,
    Channel::Integer,
    Channel::Float,
    Channel::Vec3,
    Channel::Vec4,
    Channel::Transform,
];
impl Channel {
    pub const fn input(self) -> u32 {
        match self {
            Self::Boolean => 0x9fb4_26cc,
            Self::Integer => 0x0cc9_71f1,
            Self::Float => 0x7f39_a652,
            Self::Vec3 => 0xbb8e_9f01,
            Self::Vec4 => 0xbb8e_9a06,
            Self::Transform => 0xa539_19ae,
        }
    }
    pub const fn output(self) -> u32 {
        match self {
            Self::Boolean => 0x9638_f8c5,
            Self::Integer => 0xa5f7_8658,
            Self::Float => 0x666e_571b,
            Self::Vec3 => 0x2d62_7a28,
            Self::Vec4 => 0x2d62_06cf,
            Self::Transform => 0x4c5c_7927,
        }
    }
    pub const fn kind(self) -> Kind {
        match self {
            Self::Boolean => Kind::Boolean,
            Self::Integer => Kind::Integer,
            Self::Float => Kind::Float,
            Self::Vec3 => Kind::Vec3,
            Self::Vec4 => Kind::Vec4,
            Self::Transform => Kind::Transform,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Definition {
    pub open: bool,
    pub write_on_open: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Gate {
    open: bool,
    write_on_open: bool,
    bound: [bool; 6],
    initialized: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Fresh output binding, not engine save-state restoration. Exactly once.
    Initialize,
    Event(u32),
    PropertyChanged(u32),
}
impl Gate {
    pub fn new(definition: Definition, outputs: &[Channel]) -> Result<Self, Error> {
        let mut bound = [false; 6];
        for channel in outputs {
            if std::mem::replace(&mut bound[*channel as usize], true) {
                return Err(Error::InvalidGraph);
            }
        }
        Ok(Self {
            open: definition.open,
            write_on_open: definition.write_on_open,
            bound,
            initialized: false,
        })
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub(crate) fn prepare(&mut self, action: Action) -> Result<Vec<Channel>, Error> {
        let flush = match action {
            Action::Initialize => {
                if self.initialized {
                    return Err(Error::InvalidGraph);
                }
                self.initialized = true;
                self.open
            }
            _ if !self.initialized => return Err(Error::InvalidGraph),
            Action::Event(event) => {
                // Native input handler treats every non-Open event as Close.
                let open = event == super::nodes::OPEN;
                let changed = self.open != open;
                self.open = open;
                changed && open && self.write_on_open
            }
            Action::PropertyChanged(input) => {
                return Ok(CHANNELS
                    .into_iter()
                    .filter(|c| self.open && self.bound[*c as usize] && c.input() == input)
                    .collect());
            }
        };
        Ok(CHANNELS
            .into_iter()
            .filter(|c| flush && self.bound[*c as usize])
            .collect())
    }
}
pub trait Handler: PropertyHandler {
    fn property_gate(&mut self, node: u16) -> Result<&mut Gate, Error>;
    /// Return a resolved bound input or authored fallback; None is unready.
    fn property_gate_input(&self, node: u16, channel: Channel) -> Result<Option<Value>, Error>;
}

#[cfg(test)]
mod tests;
