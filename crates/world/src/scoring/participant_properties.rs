// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-owned properties identified by an installed channel link.
use super::{Error, properties::Kind, value::Value};

pub mod dynamic;

pub const MAX_VALUES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct Property {
    pub channel: String,
    pub port: u32,
}
impl Property {
    pub fn validate(&self) -> Result<(), Error> {
        if self.channel.is_empty() || self.channel.len() > 512 || self.port == 0 {
            Err(Error::InvalidGraph)
        } else {
            Ok(())
        }
    }
}

/// The native store keeps separate maps for each scalar type, even when the
/// channel link is the same. Vectors are not part of this accessor contract.
pub fn kind(value: &Value) -> Result<Kind, Error> {
    let kind = match value {
        Value::Integer(_) => Kind::Integer,
        Value::Boolean(_) => Kind::Boolean,
        Value::Float(_) => Kind::Float,
        _ => return Err(Error::InvalidGraph),
    };
    kind.validate(value)?;
    Ok(kind)
}

/// A changed scalar value. Repeated equal writes produce no notification.
#[derive(Clone, Debug, PartialEq)]
pub struct Write {
    pub participant: u64,
    pub property: Property,
    pub value: Value,
}
