// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    data: movies::Movie,
    before: Authored,
    after: Authored,
    before_seconds: f64,
    after_seconds: f64,
}
impl From<Content> for Source {
    fn from(v: Content) -> Self {
        Self {
            data: v.data,
            before: v.before,
            after: v.after,
            before_seconds: v.before_seconds,
            after_seconds: v.after_seconds,
        }
    }
}
impl TryFrom<Source> for Content {
    type Error = String;
    fn try_from(v: Source) -> Result<Self, Self::Error> {
        v.data
            .video
            .validate()
            .map_err(|e| format!("prepared movie video: {e:?}"))?;
        if [v.before_seconds, v.after_seconds]
            .iter()
            .any(|s| !s.is_finite() || !(0. ..=300.).contains(s))
        {
            return Err("prepared movie sequence timer".into());
        }
        Ok(Self {
            data: v.data,
            before: v.before,
            after: v.after,
            before_seconds: v.before_seconds,
            after_seconds: v.after_seconds,
        })
    }
}
