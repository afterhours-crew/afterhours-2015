// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The launcher's map of one world session (`--map-feed`), read from the
//! state the listener already owns. Nothing here changes it.
use super::*;
use crate::map_feed::{Ring, Snapshot};

impl PlayerListener {
    pub(in crate::net) fn map_snapshot(&self, world_ms: u64) -> Snapshot {
        let connection = HOST_SELECTOR as u8;
        let mut snapshot = Snapshot::new(world_ms);
        // The cars traffic and rivals focus on (`ai_focus`): a presented
        // free-roam participant's spawned world car.
        for participant in self.participants.in_free_roam() {
            if !self.free_roam_presented.contains(&participant) {
                continue;
            }
            let Some((&car, _)) = self
                .world_cars
                .iter()
                .find(|(_, (owner, ready))| *owner == participant && *ready)
            else {
                continue;
            };
            match self
                .players
                .vehicle_physics(connection, self.persona, car)
                .and_then(|physics| physics.decode([0.; 3]))
            {
                Ok(motion) => {
                    let forward =
                        replication::vehicle::orientation::basis_from_quaternion(motion.quaternion)
                            .map_or([f32::NAN; 3], |basis| basis[2]);
                    snapshot.driver(participant, motion.position, motion.velocity, forward);
                }
                Err(_) => snapshot.omit(),
            }
        }
        if let Some(traffic) = &self.traffic_population {
            for (id, [position, forward]) in traffic.poses(&self.players, connection, self.persona)
            {
                snapshot.traffic(id, position, forward);
            }
            if !self.traffic_failed && traffic.traffic_percent() > 0 {
                snapshot.traffic_ring = Some(Ring {
                    inner: crate::traffic_content::population::INNER,
                    outer: crate::traffic_content::population::OUTER,
                });
            }
        }
        if let Some(rivals) = &self.rival_population {
            for (id, pose) in rivals.poses(&self.players, connection, self.persona) {
                match pose {
                    Some([position, forward]) => snapshot.rival(id, position, forward),
                    None => snapshot.omit(),
                }
            }
        }
        snapshot.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traffic_and_rivals_are_observed_without_changing_the_session() {
        let (mut traffic, mut world, content) =
            crate::traffic_content::population::tests::fixture_with(HOST_SELECTOR as u8);
        crate::traffic_content::population::tests::spawn(
            &mut traffic,
            &mut world,
            &content,
            HOST_SELECTOR as u8,
        );
        let mut listener =
            PlayerListener::new(100).with_traffic(Some(std::sync::Arc::new(content)));
        listener.players = world;
        listener.traffic_population = Some(traffic);
        let before = format!("{listener:?}");
        let objects = listener.players.objects().len();
        let snapshot = listener.map_snapshot(1234);
        assert_eq!(format!("{listener:?}"), before);
        assert_eq!(listener.players.objects().len(), objects);
        assert_eq!(snapshot.world_ms, 1234);
        assert!(snapshot.drivers.is_empty(), "no presented world car");
        let [car] = snapshot.traffic[..] else {
            panic!("one traffic car: {snapshot:?}");
        };
        let expected = listener.traffic_population.as_ref().unwrap().poses(
            &listener.players,
            HOST_SELECTOR as u8,
            100,
        );
        assert_eq!(expected.len(), 1);
        assert_eq!(car.id, expected[0].0);
        assert_eq!(car.pose.position, expected[0].1[0]);
        assert_eq!(
            car.pose.heading,
            crate::map_feed::heading(expected[0].1[1]),
            "heading from the car's forward axis"
        );
        assert!(car.pose.heading.is_some());
        assert_eq!(
            snapshot.traffic_ring,
            Some(Ring {
                inner: 300.,
                outer: 450.
            })
        );
        assert_eq!(snapshot.omitted, 0);

        // Traffic turned off: the cars stay until they retire, the ring goes.
        listener.set_ai_settings(crate::ai_settings::AiSettings::new(0, None).unwrap());
        let off = listener.map_snapshot(1500);
        assert_eq!(off.traffic.len(), 1);
        assert_eq!(off.traffic_ring, None);
        // Disabled after a fault: no ring either.
        listener.set_ai_settings(crate::ai_settings::AiSettings::default());
        assert!(listener.disable_traffic());
        assert_eq!(listener.map_snapshot(1750).traffic_ring, None);
    }

    #[test]
    fn rivals_report_their_slot_and_host_pose() {
        let (mut rivals, mut world, mut registrations, _) =
            crate::rival_content::population::tests::fixture(HOST_SELECTOR as u8);
        crate::rival_content::population::tests::spawn(
            &mut rivals,
            &mut world,
            &mut registrations,
            HOST_SELECTOR as u8,
        );
        let mut listener = PlayerListener::new(100);
        listener.players = world;
        listener.rival_population = Some(rivals);
        let snapshot = listener.map_snapshot(10);
        let poses = listener.rival_population.as_ref().unwrap().poses(
            &listener.players,
            HOST_SELECTOR as u8,
            100,
        );
        assert!(!poses.is_empty(), "an active racer");
        assert_eq!(snapshot.rivals.len(), poses.len());
        for (car, (id, pose)) in snapshot.rivals.iter().zip(&poses) {
            let [position, forward] = pose.unwrap();
            assert_eq!(car.id, *id);
            assert_eq!(car.pose.position, position);
            assert_eq!(car.pose.heading, crate::map_feed::heading(forward));
        }
        assert_eq!(snapshot.traffic_ring, None, "no traffic in this session");
        // Another session's population is not this session's.
        assert!(
            listener
                .rival_population
                .as_ref()
                .unwrap()
                .poses(&listener.players, HOST_SELECTOR as u8, 101)
                .is_empty()
        );
    }

    #[test]
    fn an_empty_session_has_an_empty_map() {
        let snapshot = PlayerListener::new(100).map_snapshot(0);
        assert_eq!(snapshot, Snapshot::new(0));
    }
}
