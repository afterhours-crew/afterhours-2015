// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::progression::{Branch, entities::Role};
use nfs_world::replication::{Initial, entity};

impl PlayerListener {
    pub(super) fn bind_progression_launchers(
        progression: Option<&crate::progression::Content>,
        players: &Players,
        sections: &[Section],
        launchers: &mut nfs_world::launchers::Launchers,
        persona: u64,
    ) -> Result<(), replication::Error> {
        let Some(progression) = progression else {
            return Ok(());
        };
        for record in sections.iter().flat_map(|s| &s.records) {
            let Some(Initial::Entity { prefix, fields }) = &record.initial else {
                continue;
            };
            let Some(blueprint) = [Branch::Main, Branch::Intro]
                .into_iter()
                .filter_map(|branch| progression.construction.blueprint(branch))
                .find(|blueprint| blueprint.asset() == prefix.asset)
            else {
                continue;
            };
            let indices = blueprint
                .roles()
                .iter()
                .enumerate()
                .filter_map(|(i, role)| (*role == Role::Launcher).then_some(i))
                .collect::<Vec<_>>();
            if indices.is_empty() {
                continue;
            }
            let Some(entity::Initial::Root {
                reference: participant,
                ..
            }) = fields.first()
            else {
                return Err(replication::Error::TypeMismatch);
            };
            if !players.owns_participant(HOST_SELECTOR as u8, persona, *participant)
                || players
                    .objects()
                    .get(record.id)
                    .is_none_or(|o| o.initial != *record.initial.as_ref().unwrap())
            {
                return Err(replication::Error::UnknownObject);
            }
            let Some(Initial::Participant(owner)) =
                players.objects().get(*participant).map(|o| &o.initial)
            else {
                return Err(replication::Error::TypeMismatch);
            };
            launchers.bind_entity(record, &indices, owner.player)?;
        }
        Ok(())
    }
}
