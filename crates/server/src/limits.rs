// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

pub const CONTROL_FRAME: usize = 64 * 1024;
pub const LOOKUP_BODY: usize = 16 * 1024;
pub fn control_frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(CONTROL_FRAME, 1024, CONTROL_FRAME - 16).expect("constant limits")
}
pub fn control_body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: CONTROL_FRAME - 16,
        max_depth: 8,
        max_values: 2048,
        max_collection: 128,
        max_byte_string: 1024,
    }
}
pub fn lookup_frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(LOOKUP_BODY + 128 + 16, 128, LOOKUP_BODY).expect("constant limits")
}
pub fn lookup_body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: LOOKUP_BODY,
        max_depth: 6,
        max_values: 2048,
        max_collection: 128,
        max_byte_string: 1024,
    }
}
