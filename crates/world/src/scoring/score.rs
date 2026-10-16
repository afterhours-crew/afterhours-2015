// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Score events address a context and its activated keeper list. They are not
//! wire requests or durable REP writes. All identifiers belong to owned state.
use super::{
    Error,
    events::{Dispatch, PropertyHandler},
    value::Value,
};

pub const AWARD: u32 = 0x0c44_fa44;
pub const SCORE: u32 = 0x0dc8_374d;
pub const CONTEXT: u32 = 0x04d3_44d2;
pub const KEEPERS: u32 = 0x3fff_0633;
pub const PARTICIPANT: u32 = 0xb68b_0dbb;
pub const MAX_KEEPERS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    pub context: u16,
    /// Used when Score is unbound OR its bound cell is not ready. This must be
    /// read from installed data; it is not a fixed default driving award.
    pub default: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Context {
    /// Opaque session-owned participant identity; never supplied by an RPC.
    pub participant: u64,
    /// Resolve at context activation, preserving native provider traversal order
    /// and duplicates. Empty before activation. Keep stable for this transaction;
    /// dynamic provider rebinding during a callback is outside this model.
    pub keepers: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub keeper: u64,
    pub participant: u64,
    /// Preserve the signed amount, including zero and negative values. The
    /// durable earned-REP policy separately rejects unsupported negative credit.
    pub amount: i32,
}

pub trait Handler: PropertyHandler {
    fn score_binding(&self, node: u16) -> Result<Binding, Error>;
    /// None means no ready input, so the authored default applies. Unknown or
    /// invalid wiring must return an error, never masquerade as an unready cell.
    fn score_input(&self, node: u16) -> Result<Option<Value>, Error>;
    fn score_context(&self, context: u16) -> Result<Context, Error>;
    /// Apply to the private candidate only, completing any synchronous observers
    /// before the next keeper. No external I/O. Bound queued output and commit
    /// state, network output and any durable award together at the caller edge.
    fn score_request(&mut self, request: Request, dispatch: &mut Dispatch<'_>)
    -> Result<(), Error>;
}
