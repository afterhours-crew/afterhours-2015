// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ItemInfoAccessor publication contract (04712ab0). The host supplies its
//! own participant's current item; no client-supplied item identity is trusted.
use super::value::{Text, Value};
pub const ITEM_ID: u32 = 0xa7c5_fb3d;
pub const ITEM_NAME: u32 = 0xb133_5137;
/// CString at the item definition's native +0x38, distinct from Asset::Name.
pub const TEXT: u32 = 0x0dd1_2f68;
pub const VALID: u32 = 0x834c_afe0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sample {
    pub id: u64,
    pub name: Text,
    pub text: Text,
}

pub fn outputs(sample: Option<&Sample>) -> Vec<(u32, Value)> {
    let mut output = Vec::with_capacity(4);
    if let Some(sample) = sample {
        output.extend([
            (ITEM_ID, Value::Unsigned64(sample.id)),
            (ITEM_NAME, Value::Text(sample.name.clone())),
            (TEXT, Value::Text(sample.text.clone())),
        ]);
    }
    // Detaching retains the old ID/text. Initially they remain unready.
    output.push((VALID, Value::Boolean(sample.is_some())));
    output
}
