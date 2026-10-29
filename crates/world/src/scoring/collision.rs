// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One-shot collision query control, independent of REP awards and transport.
use super::{
    Error,
    events::{Dispatch, EntityContext, PropertyHandler},
};

pub const BEGIN: u32 = 0xba8c_9628;
pub const TERMINATE: u32 = 0x5463_1ada;
pub const QUERYING: u32 = 0x0372_2f0c;
pub const VALUE: u32 = 0x6940_c23b;
pub const COLLISION: u32 = 0xb240_dbba;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Begin,
    Terminate,
}
impl Command {
    /// Server-to-client method, with no arguments. Transport framing and the
    /// current installed serializer selector remain the session's responsibility.
    pub fn method(self) -> u32 {
        match self {
            Self::Begin => 1,
            Self::Terminate => 2,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Event(u32),
    /// The caller already validated the query owner and resolved the context
    /// in that session's current world. A report is not a completed REP award.
    Report {
        value: i32,
        context: EntityContext,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Outputs {
    pub querying: bool,
    pub value: bool,
    /// Installed Boolean property binding's equality flag. Do not infer this
    /// from the value or apply it to the integer output, which always publishes.
    pub suppress_equal_querying: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub(crate) outputs: Outputs,
    pub(crate) querying: bool,
}
impl Node {
    /// Fresh construction only. Bound outputs use separately allocated typed
    /// Store cells, initialized in the verified construction order by the caller.
    pub fn new(outputs: Outputs) -> Self {
        Self {
            outputs,
            querying: false,
        }
    }
    pub fn querying(&self) -> bool {
        self.querying
    }
}
pub trait Handler: PropertyHandler {
    fn collision_query(&mut self, node: u16) -> Result<&mut Node, Error>;
    /// Append only to this private candidate's bounded command queue. Do not
    /// send packets here. Commit state and command output together after success.
    fn collision_control(
        &mut self,
        node: u16,
        command: Command,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error>;
}
