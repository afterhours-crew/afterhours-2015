// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The server's map frames are the lines the launcher reads: one contract
//! across the two crates the bundle links.
use nfs_afterhours::{events::Body, map_feed as launcher};
use nfs_server::map_feed::{self as server, Board, Ring, Snapshot};

#[test]
fn both_sides_share_their_bounds() {
    assert_eq!(server::FORMAT, launcher::SERVER_FORMAT);
    assert_eq!(server::MAX_WORLDS, launcher::MAX_WORLDS);
    assert_eq!(server::MAX_DRIVERS, launcher::MAX_DRIVERS);
    assert_eq!(server::MAX_CARS, launcher::MAX_CARS);
}

#[test]
fn the_launcher_reads_what_the_server_writes() {
    let (board, mut reader) = Board::new();
    let first = board.publisher(2, 1_000_000_007);
    let second = board.publisher(5, 42);
    let mut snapshot = Snapshot::new(1250);
    snapshot.driver(1, [-8191.99, 35.2, 8191.99], [6., 0., 8.], [1., 0., 0.]);
    snapshot.traffic(7, [12.345, 0., -45.678], [0., 0., 1.]);
    snapshot.traffic(3, [-100_000., 0., 100_000.], [-1., 0., 0.]);
    snapshot.rival(1, [0.; 3], [0., 1., 0.]);
    snapshot.traffic_ring = Some(Ring {
        inner: 300.,
        outer: 450.,
    });
    first.publish(snapshot);
    second.publish(Snapshot::new(0));
    let line = reader.next_line().unwrap().unwrap();
    let frame = launcher::parse(&line).unwrap();
    assert_eq!(frame.frame, 1);
    let [one, two] = &frame.worlds[..] else {
        panic!("two worlds: {line}")
    };
    assert_eq!(
        (one.connection, one.persona_id.as_str(), one.world_ms),
        (2, "1000000007", 1250)
    );
    assert_eq!((two.connection, two.persona_id.as_str()), (5, "42"));
    let driver = &one.drivers[0];
    assert_eq!(
        (driver.x, driver.z, driver.heading, driver.speed),
        (-8191.99, 8191.99, Some(90.), 10.)
    );
    assert_eq!(
        one.traffic
            .iter()
            .map(|c| (c.id, c.heading))
            .collect::<Vec<_>>(),
        [(3, Some(270.)), (7, Some(180.))],
        "ordered by id"
    );
    assert_eq!((one.traffic[1].x, one.traffic[1].z), (12.35, -45.68));
    assert_eq!(one.rivals[0].heading, None);
    assert_eq!(
        one.traffic_ring,
        Some(launcher::TrafficRing {
            inner: 300.,
            outer: 450.
        })
    );
    let mut relay = launcher::Relay::default();
    assert!(matches!(
        relay.accept(&line),
        Some(Body::MapFeed {
            frame: 1,
            skipped: 0,
            ..
        })
    ));
}

#[test]
fn a_full_board_fits_the_launcher_line_bound() {
    let (board, mut reader) = Board::new();
    let publishers: Vec<_> = (0..server::MAX_WORLDS as u32)
        .map(|connection| board.publisher(connection, u64::MAX))
        .collect();
    let far = -99_999.99;
    for publisher in &publishers {
        let mut snapshot = Snapshot::new(u64::MAX);
        for participant in 0..server::MAX_DRIVERS as u16 {
            snapshot.driver(
                u16::MAX - participant,
                [far; 3],
                [1e3, 1e3, 1e3],
                [-0.1, 0., 1.],
            );
        }
        for id in 0..server::MAX_CARS as u64 {
            // Spawn keys stay below the rival key range.
            snapshot.traffic((1 << 40) - 1 - id, [far; 3], [-0.1, 0., 1.]);
            snapshot.rival(u64::from(u32::MAX) - id, [far; 3], [-0.1, 0., 1.]);
        }
        snapshot.traffic_ring = Some(Ring {
            inner: 300.,
            outer: 450.,
        });
        publisher.publish(snapshot);
    }
    let line = reader.next_line().unwrap().unwrap();
    assert!(
        line.len() < launcher::MAX_LINE_BYTES,
        "{} bytes",
        line.len()
    );
    let frame = launcher::parse(&line).unwrap();
    assert_eq!(frame.worlds.len(), launcher::MAX_WORLDS);
    assert!(frame.worlds.iter().all(|w| w.omitted == 0
        && w.drivers.len() == launcher::MAX_DRIVERS
        && w.traffic.len() == launcher::MAX_CARS
        && w.rivals.len() == launcher::MAX_CARS));
}
