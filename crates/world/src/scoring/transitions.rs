// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Requests to the enclosing event's membership/channel owner. The host must
//! resolve these within the same event lifetime and commit them atomically.
use super::participant_properties::Property;

pub const TRIGGER: u32 = 0xf07a_7291;
pub const TRIGGER_ALL: u32 = 0xa55a_80c3;
pub const CHANNEL_IN: u32 = 0x128f_f02e;
pub const CHANNEL_OUT: u32 = 0xfa9e_51f7;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub node: u16,
    pub selection: Selection,
    /// Preserve authored source order and repeats for both selection modes.
    pub sources: Vec<Property>,
    pub target: Option<Property>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Selection {
    FirstCompleted(u64),
    AllMembers,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelEvent {
    /// The channel instance bound by the spawning logic, never a global name.
    pub channel: u64,
    pub event: u32,
    pub participant: u64,
}
