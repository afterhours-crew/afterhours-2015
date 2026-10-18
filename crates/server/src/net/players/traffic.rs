// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::traffic_content::population::{Focus, Input};
use nfs_world::application::Random;

impl PlayerListener {
    pub(in crate::net) fn advance_traffic(
        &mut self,
        now_ms: u64,
    ) -> Result<(), replication::Error> {
        if !self.pending_entries.is_empty()
            || !self.pending.is_empty()
            || !self.pending_rpcs.is_empty()
            || self.session_backlog > 1
        {
            return Ok(());
        }
        let (Some(content), Some(population)) = (&self.traffic_content, &self.traffic_population)
        else {
            return Ok(());
        };
        let mut population = population.clone();
        let mut focus = Vec::new();
        for participant in self.participants.in_free_roam() {
            if self.traffic_failed || !self.free_roam_presented.contains(&participant) {
                continue;
            }
            let Some((&car, _)) = self
                .world_cars
                .iter()
                .find(|(_, (owner, ready))| *owner == participant && *ready)
            else {
                continue;
            };
            let physics = self
                .players
                .vehicle_physics(HOST_SELECTOR as u8, self.persona, car)?;
            let motion = physics.decode([0.; 3])?;
            focus.push(Focus {
                id: participant,
                position: motion.position,
                velocity: motion.velocity,
                forward: replication::vehicle::orientation::basis_from_quaternion(
                    motion.quaternion,
                )?[2],
            });
        }
        let mut players = self.players.clone();
        let changes = population.advance(
            &mut players,
            HOST_SELECTOR as u8,
            self.persona,
            content,
            Input {
                now: now_ms,
                focus: &focus,
                random: [
                    crate::seeds::OsRandom.next_u32(),
                    crate::seeds::OsRandom.next_u32(),
                ],
            },
        )?;
        let mut entries: Vec<_> = split_scenes(
            changes.records,
            nfs_world::application::OUTBOUND_FRAME_BITS - 104,
        )?
        .into_iter()
        .map(|mut section| {
            // Traffic motion is encoded in absolute world coordinates.
            // Reserve the larger raw setup before splitting. An explicit
            // origin also permits bounded co-publication
            // with player chassis updates at the session edge.
            section.setup = Some(Setup::RawEscape([0; 3]));
            (section, Vec::new())
        })
        .collect();
        // Manager RPCs reference newly created ghosts. queue_replication_after
        // orders them after all preceding creation sections; deletions likewise
        // follow the relation-clearing updates instead of preceding them on wire.
        if !changes.calls.is_empty() || !changes.deleted.is_empty() {
            let mut section = returning::empty_section();
            section.deleted = changes.deleted;
            entries.push((section, changes.calls));
        }
        if entries.len() > MAX_PENDING.saturating_sub(self.pending_entries.len()) {
            return Ok(());
        }
        if let Some(ids) = changes.spawned {
            tracing::info!(
                participant = ids.participant,
                actor = ids.actor,
                vehicle = ids.vehicle,
                count = population.count(),
                "owned traffic spawn queued"
            );
        }
        self.players = players;
        self.traffic_population = Some(population);
        self.pending_entries.extend(entries);
        Ok(())
    }

    pub(in crate::net) fn disable_traffic(&mut self) -> bool {
        // Stop spawning while retaining the normal transactional retirement
        // path on subsequent ticks. Do not strand already published AI cars.
        !std::mem::replace(&mut self.traffic_failed, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backpressure_retains_traffic_and_disable_orders_detach_before_deletion() {
        let (mut population, mut world, content) =
            crate::traffic_content::population::tests::fixture_with(HOST_SELECTOR as u8);
        crate::traffic_content::population::tests::spawn(
            &mut population,
            &mut world,
            &content,
            HOST_SELECTOR as u8,
        );
        let mut listener =
            PlayerListener::new(100).with_traffic(Some(std::sync::Arc::new(content)));
        listener.players = world;
        listener.traffic_population = Some(population);
        let delegated = listener.movement_objects();
        assert_eq!(delegated.len(), 1);
        assert!(
            listener
                .players
                .owned_vehicles(HOST_SELECTOR as u8, 100)
                .is_empty()
        );
        listener.session_backlog = 2;
        assert!(listener.disable_traffic());
        assert!(!listener.disable_traffic());
        listener.advance_traffic(1002).unwrap();
        assert_eq!(listener.traffic_population.as_ref().unwrap().count(), 1);
        assert!(listener.pending_entries.is_empty());
        listener.session_backlog = 0;
        listener.advance_traffic(1003).unwrap();
        assert_eq!(listener.traffic_population.as_ref().unwrap().count(), 0);
        assert!(listener.movement_objects().is_empty());
        assert_eq!(listener.pending_entries.len(), 2);
        let (detach, before) = &listener.pending_entries[0];
        assert!(before.is_empty() && detach.deleted.is_empty());
        assert_eq!(detach.records.len(), 2);
        let (delete, before) = &listener.pending_entries[1];
        assert!(before.is_empty() && delete.records.is_empty());
        assert_eq!(delete.deleted.len(), 4);
    }
}
