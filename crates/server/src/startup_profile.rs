// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;
pub struct Profile {
    persona_id: i64,
    branch: Option<crate::startup_branch::SoloProfile>,
}
impl Profile {
    pub fn owned(persona_id: i64) -> Result<Self, Failure> {
        if persona_id <= 0 {
            return Err(Failure::ProfileConfig);
        }
        Ok(Self {
            persona_id,
            branch: None,
        })
    }
    pub fn persona_id(&self) -> i64 {
        self.persona_id
    }
    pub fn branch(&self) -> Option<&crate::startup_branch::SoloProfile> {
        self.branch.as_ref()
    }
    pub fn with_solo_branch(
        mut self,
        branch: crate::startup_branch::SoloProfile,
    ) -> Result<Self, Failure> {
        if branch.persona_id() != self.persona_id {
            return Err(Failure::ProfileConfig);
        }
        self.branch = Some(branch);
        Ok(self)
    }
}
