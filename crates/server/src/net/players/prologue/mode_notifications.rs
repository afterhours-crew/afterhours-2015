// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scene Boolean notifications of participant-in-game-mode properties for
//! intro participants. Each is true while the participant is in a mode whose
//! categories satisfy the property (official E1026: events s9 from the event
//! canceller's start, 124.718 s, to its shutdown, 1247.825 s; events s1 and
//! ui s1 during both races). Only changes are sent. The notifications are
//! scene-wide; the prologue runs one intro participant per world.
use super::*;
use nfs_world::participants::HostRpc;

impl PlayerListener {
    pub(in crate::net::players) fn advance_mode_notifications(
        &mut self,
    ) -> Result<(), replication::Error> {
        let Some(content) = self.prologue.clone() else {
            return Ok(());
        };
        if content.mode_notifications.is_empty() || !self.pending_entries.is_empty() {
            return Ok(());
        }
        let world = self.players.objects();
        let mut calls = Vec::new();
        let mut next = self.mode_notified.clone();
        // Intro participants, and any whose last sent value is still true:
        // a participant leaving the intro is switched off before it is dropped.
        let participants: BTreeSet<u16> = self
            .participants
            .intro_participants()
            .chain(next.iter().filter(|(_, on)| **on).map(|(&(p, _), _)| p))
            .collect();
        for participant in participants {
            let held: Vec<&BTreeSet<u32>> = match self.intro_modes.get(&participant) {
                Some((active, canceller, chapter)) => [
                    content
                        .mode(*chapter)
                        .ok()
                        .and_then(|m| active.held_categories(m)),
                    content
                        .driving
                        .as_ref()
                        .and_then(|pair| canceller.held_categories(&pair.1)),
                ]
                .into_iter()
                .flatten()
                .collect(),
                None => Vec::new(),
            };
            for (index, (key, notification)) in content.mode_notifications.iter().enumerate() {
                let wanted = held.iter().any(|c| notification.matches(c));
                let sent = next.get(&(participant, index)).copied().unwrap_or(false);
                if wanted == sent {
                    continue;
                }
                let Some(scene) = world.scene(*key) else {
                    continue;
                };
                let Some(replication::Initial::SubLevel { fields, .. }) =
                    world.get(scene).map(|o| &o.initial)
                else {
                    return Err(replication::Error::TypeMismatch);
                };
                let rpc = fields
                    .get(notification.serializer)
                    .and_then(replication::sublevel::Initial::rpc)
                    .ok_or(replication::Error::TypeMismatch)?;
                let call = HostRpc::ModeNotification(nfs_world::garage::presence::Notification {
                    endpoint: nfs_world::participants::Endpoint {
                        scene,
                        selector: rpc.selector,
                        serial: rpc.serial,
                    },
                    enabled: wanted,
                });
                call.encode().map_err(|_| replication::Error::Shape)?;
                calls.push(call);
                next.insert((participant, index), wanted);
            }
        }
        next.retain(|&(participant, _), on| {
            *on || self.participants.intro_chapter(participant).is_some()
        });
        if calls.len() > nfs_world::session::MAX_RPC_QUEUE.saturating_sub(self.pending_rpcs.len()) {
            return Ok(());
        }
        self.pending_rpcs.extend(calls);
        self.mode_notified = next;
        Ok(())
    }
}
