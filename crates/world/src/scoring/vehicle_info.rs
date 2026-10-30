// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{
    Error,
    value::{Text, Value},
};
pub const BUNDLE: u32 = 0x866c_219b;
pub const BUNDLE_SIGNED: u32 = 0x1158_3e68;
pub const SHORT_NAME: u32 = 0x2e54_c585;
pub const CHANGED: u32 = 0x6a11_32b0;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sample {
    pub bundle: u32,
    pub short_name: Text,
}
pub fn outputs(sample: Option<&Sample>) -> Result<[(u32, Value); 3], Error> {
    let bundle = sample.map_or(0, |v| v.bundle);
    let name = match sample {
        Some(v) => v.short_name.clone(),
        None => Text::new("")?,
    };
    Ok([
        (BUNDLE, Value::Unsigned32(bundle)),
        (BUNDLE_SIGNED, Value::Integer(bundle as i32)),
        (SHORT_NAME, Value::Text(name)),
    ])
}
