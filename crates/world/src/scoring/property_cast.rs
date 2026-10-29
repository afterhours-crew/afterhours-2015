// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Numeric PropertyCast binding. String input/output require a separate model.
use super::{Error, events::PropertyHandler, properties::Kind};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Channel {
    Boolean,
    Integer,
    Float,
}
/// Activation checks ready inputs in this order and stops at the first one.
pub const CHANNELS: [Channel; 3] = [Channel::Boolean, Channel::Integer, Channel::Float];
impl Channel {
    pub const fn input(self) -> u32 {
        match self {
            Self::Boolean => 0x0a7e_28e0,
            Self::Integer => 0x0056_c23d,
            Self::Float => 0xba31_493e,
        }
    }
    pub const fn output(self) -> u32 {
        match self {
            Self::Boolean => 3_802_618_965,
            Self::Integer => 245_372_264,
            Self::Float => 936_870_923,
        }
    }
    pub const fn kind(self) -> Kind {
        match self {
            Self::Boolean => Kind::Boolean,
            Self::Integer => Kind::Integer,
            Self::Float => Kind::Float,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bindings {
    inputs: [bool; 3],
    outputs: [bool; 3],
}
impl Bindings {
    /// Supply all connected ports; unknown/string ports and duplicates fail.
    /// This does not seed outputs, initialize inputs or register callbacks.
    pub fn new(inputs: &[u32], outputs: &[u32]) -> Result<Self, Error> {
        fn resolve(ports: &[u32], input: bool) -> Result<[bool; 3], Error> {
            if ports.len() > 3 {
                return Err(Error::Bound);
            }
            let mut bound = [false; 3];
            for &port in ports {
                let channel = CHANNELS
                    .into_iter()
                    .find(|c| {
                        if input {
                            c.input() == port
                        } else {
                            c.output() == port
                        }
                    })
                    .ok_or(Error::InvalidGraph)?;
                if std::mem::replace(&mut bound[channel as usize], true) {
                    return Err(Error::InvalidGraph);
                }
            }
            Ok(bound)
        }
        Ok(Self {
            inputs: resolve(inputs, true)?,
            outputs: resolve(outputs, false)?,
        })
    }
    pub(crate) fn input(self, channel: Channel) -> bool {
        self.inputs[channel as usize]
    }
    pub(crate) fn output(self, channel: Channel) -> bool {
        self.outputs[channel as usize]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Activate,
    PropertyChanged(u32),
}
pub trait Handler: PropertyHandler {
    /// Fixed per-session bindings validated before invoking the node.
    fn property_cast(&self, node: u16) -> Result<Bindings, Error>;
}
