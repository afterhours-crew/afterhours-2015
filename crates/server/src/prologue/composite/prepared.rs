// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    data: composite::Composite,
    sequence: Authored,
    parent: Asset,
    fallback_seconds: f64,
}
impl From<Content> for Source {
    fn from(v: Content) -> Self {
        Self {
            data: v.data,
            sequence: v.sequence,
            parent: v.parent,
            fallback_seconds: v.fallback_seconds,
        }
    }
}
impl TryFrom<Source> for Content {
    type Error = String;
    fn try_from(v: Source) -> Result<Self, Self::Error> {
        if !v.fallback_seconds.is_finite()
            || !(0. ..=600.).contains(&v.fallback_seconds)
            || v.fallback_seconds == 0.
        {
            return Err("prepared composite timer".into());
        }
        Ok(Self {
            data: v.data,
            sequence: v.sequence,
            parent: v.parent,
            fallback_seconds: v.fallback_seconds,
        })
    }
}
