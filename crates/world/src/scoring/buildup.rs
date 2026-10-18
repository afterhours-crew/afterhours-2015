// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Input ports and event counters for the nitrous buildup observer.
pub const DRIFTING: u32 = 0xe1c2_4248;
pub const DRAFTING: u32 = 0x823d_3f7f;
pub const ONCOMING: u32 = 0xb7de_e723;
pub const AIRBORNE: u32 = 0xed61_5b6b;
pub const SPEED: u32 = 0x0dc3_0e82;
pub const PACING: u32 = 0xc8fe_4f57;
pub const NEAR_MISS: u32 = 0x3abf_4718;
pub const COP_SLAM: u32 = 0x685f_6106;
