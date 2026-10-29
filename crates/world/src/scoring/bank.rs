// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! BankTransaction requests, separate from the host's durable ledger. Native
//! 046ac3d0 reads Amount and the bound participant on each event; 046a53c0
//! emits an ordinary success/failure event only after the future completes.
use super::participant_properties::Property;

pub const TRANSACT: u32 = 0xa939_2790;
pub const AMOUNT: u32 = 0x9477_0229;
pub const SUCCESS: u32 = 0xbb06_a967;
pub const FAILURE: u32 = 0x3cdc_0924;
pub const MAX_PENDING: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    Deposit,
    Withdraw,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// Monotonic within this logic instance. A durable host key must also
    /// contain the owning session and logic lifetime; it is not a global ID.
    pub id: u64,
    pub node: u16,
    pub bank: Property,
    pub participant: u64,
    pub kind: Kind,
    /// Native Int32 input, preserved without a narrowing conversion. The host
    /// validates its economy policy before accepting the transaction.
    pub amount: i32,
}
