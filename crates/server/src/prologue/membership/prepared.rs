// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    key: u32,
    scope: nfs_content::events::lifecycle::Scope,
    presentation: Vec<presentation::Definition>,
}
impl From<Content> for Source {
    fn from(value: Content) -> Self {
        Self {
            key: value.key,
            scope: value.plan.data().clone(),
            presentation: value.presentation,
        }
    }
}
impl TryFrom<Source> for Content {
    type Error = String;
    fn try_from(value: Source) -> Result<Self, Self::Error> {
        if value.presentation.len() != presentation::BLUEPRINTS.len() {
            return Err("prepared membership presentation count".into());
        }
        let plan = Plan::scene(&value.scope).map_err(|e| format!("prepared membership: {e:?}"))?;
        Ok(Self {
            key: value.key,
            plan: Arc::new(plan),
            presentation: value.presentation,
        })
    }
}
