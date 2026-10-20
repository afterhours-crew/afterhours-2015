// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::replication::vehicle::orientation::{basis_from_quaternion, quaternion_from_basis};
use serde_json::{Value, json};
use std::sync::{Mutex, mpsc};

fn line(reader: &mut Reader) -> Value {
    let text = reader.next_line().unwrap().expect("a changed board");
    serde_json::from_str(&text).unwrap()
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn heading_is_clockwise_from_up_the_map() {
    // The map shows +z down, so up the map is -z and right is +x.
    for (forward, expected) in [
        ([0., 0., -1.], 0.),
        ([1., 0., -1.], 45.),
        ([1., 0., 0.], 90.),
        ([0., 0., 1.], 180.),
        ([-1., 0., 0.], 270.),
        ([-1., 0., -1.], 315.),
        // Climbing or descending changes nothing on the map.
        ([0., 0.8, -0.6], 0.),
    ] {
        let heading = heading(forward).unwrap();
        assert!((heading - expected).abs() < 1e-3, "{forward:?}: {heading}");
    }
    // Just left of up stays below 360.
    let left = heading([-1e-7, 0., -1.]).unwrap();
    assert!((0. ..360.).contains(&left), "{left}");
    for unusable in [
        [0., 1., 0.],
        [0., 0., 0.],
        [f32::NAN, 0., 1.],
        [0., 0., f32::INFINITY],
    ] {
        assert_eq!(heading(unusable), None, "{unusable:?}");
    }
}

#[test]
fn heading_follows_the_forward_axis_of_a_car_rotation() {
    // A car turned to face the map's right: its basis (right, up, forward)
    // as the server builds one for a car placed along a road.
    let forward = [1., 0., 0.];
    let basis = [[forward[2], 0., -forward[0]], [0., 1., 0.], forward];
    let rotation = quaternion_from_basis(basis).unwrap();
    let decoded = basis_from_quaternion(rotation).unwrap();
    let heading = heading(decoded[2]).unwrap();
    assert!((heading - 90.).abs() < 1e-3, "{heading}");
}

#[test]
fn a_pose_needs_a_finite_position_within_the_world() {
    assert!(Pose::new([LIMIT, -LIMIT, 0.], [0., 0., -1.]).is_some());
    for position in [
        [LIMIT * 1.01, 0., 0.],
        [0., 0., -LIMIT * 1.01],
        [f32::NAN, 0., 0.],
        [0., f32::INFINITY, 0.],
    ] {
        assert_eq!(Pose::new(position, [0., 0., -1.]), None, "{position:?}");
    }
    // A usable position without a usable heading is still shown.
    assert_eq!(
        Pose::new([1., 2., 3.], [0., 1., 0.]),
        Some(Pose {
            position: [1., 2., 3.],
            heading: None
        })
    );
}

#[test]
fn drivers_report_speed_and_unusable_entries_are_counted() {
    let mut snapshot = Snapshot::new(5);
    snapshot.driver(2, [10., 1., -20.], [3., 0., 4.], [0., 0., -1.]);
    snapshot.driver(3, [f32::NAN, 0., 0.], [0.; 3], [0., 0., -1.]);
    snapshot.driver(4, [0.; 3], [f32::INFINITY, 0., 0.], [0., 0., -1.]);
    snapshot.traffic(1, [0., 0., LIMIT * 2.], [1., 0., 0.]);
    snapshot.rival(1, [f32::NAN; 3], [1., 0., 0.]);
    snapshot.omit();
    let snapshot = snapshot.finish();
    assert_eq!(snapshot.drivers.len(), 1);
    assert_eq!(snapshot.drivers[0].participant, 2);
    assert_eq!(snapshot.drivers[0].speed, 5.);
    assert_eq!(snapshot.drivers[0].pose.heading, Some(0.));
    assert!(snapshot.traffic.is_empty() && snapshot.rivals.is_empty());
    assert_eq!(snapshot.omitted, 5);
}

#[test]
fn lists_are_bounded_ordered_and_unique() {
    let mut snapshot = Snapshot::new(0);
    for participant in (0..=MAX_DRIVERS as u16).rev() {
        snapshot.driver(participant, [0.; 3], [0.; 3], [0., 0., -1.]);
    }
    for id in (0..=MAX_CARS as u64).rev() {
        snapshot.traffic(id, [id as f32, 0., 0.], [0., 0., -1.]);
    }
    for id in [9, 3, 9, 1] {
        snapshot.rival(id, [id as f32, 0., 0.], [0., 0., -1.]);
    }
    let snapshot = snapshot.finish();
    assert_eq!(snapshot.drivers.len(), MAX_DRIVERS);
    assert_eq!(snapshot.traffic.len(), MAX_CARS);
    assert!(
        snapshot
            .drivers
            .windows(2)
            .all(|w| w[0].participant < w[1].participant)
    );
    assert!(snapshot.traffic.windows(2).all(|w| w[0].id < w[1].id));
    assert_eq!(
        snapshot.rivals.iter().map(|c| c.id).collect::<Vec<_>>(),
        [1, 3, 9],
        "a repeated id is shown once"
    );
    // One driver and one car past the bounds, one repeated rival.
    assert_eq!(snapshot.omitted, 3);
}

#[test]
fn pacing_keeps_its_phase_and_never_bursts() {
    let mut pacing = Pacing::default();
    let taken: Vec<u64> = [0, 50, 249, 250, 300, 520, 700, 1300, 1310, 1500]
        .into_iter()
        .filter(|&now| pacing.take(now))
        .collect();
    assert_eq!(taken, [0, 250, 520, 1300, 1500]);
}

fn car_snapshot(world_ms: u64, x: f32) -> Snapshot {
    let mut snapshot = Snapshot::new(world_ms);
    snapshot.driver(1, [x, 0., 0.], [0.; 3], [0., 0., -1.]);
    snapshot
}

#[test]
fn a_slow_reader_gets_the_newest_snapshot_and_skips_the_rest() {
    let (board, mut reader) = Board::new();
    assert_eq!(reader.next_line(), Ok(None), "nothing published yet");
    let first = board.publisher(1, 100);
    for (ms, x) in [(0, 1.), (250, 2.), (500, 3.)] {
        first.publish(car_snapshot(ms, x));
    }
    let frame = line(&mut reader);
    assert_eq!(frame["frame"], 1);
    assert_eq!(frame["worlds"][0]["world_ms"], 500);
    assert_eq!(frame["worlds"][0]["drivers"][0]["x"], 3.);
    assert_eq!(reader.next_line(), Ok(None), "unchanged since");
    first.publish(car_snapshot(750, 4.));
    let frame = line(&mut reader);
    assert_eq!(frame["frame"], 2, "frames count lines, not snapshots");
    assert_eq!(frame["worlds"][0]["drivers"][0]["x"], 4.);
}

#[test]
fn sessions_are_ordered_by_connection_and_leave_when_they_end() {
    let (board, mut reader) = Board::new();
    let late = board.publisher(9, 200);
    let early = board.publisher(2, 100);
    late.publish(car_snapshot(10, 9.));
    early.publish(car_snapshot(20, 2.));
    let frame = line(&mut reader);
    let worlds = frame["worlds"].as_array().unwrap();
    assert_eq!(
        worlds
            .iter()
            .map(|w| (w["connection"].clone(), w["persona_id"].clone()))
            .collect::<Vec<_>>(),
        [(json!(2), json!("100")), (json!(9), json!("200"))]
    );
    drop(late);
    let frame = line(&mut reader);
    assert_eq!(frame["worlds"].as_array().unwrap().len(), 1);
    assert_eq!(frame["worlds"][0]["connection"], 2);
    // An unpublished session leaving changes nothing.
    drop(board.publisher(5, 300));
    assert_eq!(reader.next_line(), Ok(None));
    // The board closes after its last change is read.
    drop((board, early));
    assert_eq!(line(&mut reader)["worlds"], json!([]));
    assert_eq!(reader.next_line(), Err(Closed));
}

#[test]
fn the_board_holds_at_most_max_worlds_sessions() {
    let (board, mut reader) = Board::new();
    let publishers: Vec<_> = (0..=MAX_WORLDS as u32)
        .map(|connection| board.publisher(connection, 1))
        .collect();
    for publisher in &publishers {
        publisher.publish(Snapshot::new(0));
    }
    let frame = line(&mut reader);
    assert_eq!(frame["worlds"].as_array().unwrap().len(), MAX_WORLDS);
    // A session already on the board still updates when it is full.
    publishers[0].publish(Snapshot::new(7));
    assert_eq!(line(&mut reader)["worlds"][0]["world_ms"], 7);
}

#[test]
fn a_line_has_exactly_the_documented_fields_rounded() {
    let (board, mut reader) = Board::new();
    let publisher = board.publisher(3, 1_000_000_007);
    let mut snapshot = Snapshot::new(1234);
    snapshot.driver(
        1,
        [-1234.5678, 12.345, 6543.211],
        [0., 0., 12.3456],
        [1e-6, 0., -1.],
    );
    snapshot.traffic(41, [100.004, 0., -100.006], [-1e-5, 0., -1.]);
    snapshot.rival(7, [1., 2., 3.], [0., 1., 0.]);
    snapshot.traffic_ring = Some(Ring {
        inner: 300.,
        outer: 450.,
    });
    publisher.publish(snapshot);
    let text = reader.next_line().unwrap().unwrap();
    assert!(!text.contains('\n'), "one line");
    let frame: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(keys(&frame), ["frame", "map_feed", "worlds"]);
    assert_eq!(frame["map_feed"], FORMAT);
    let world = &frame["worlds"][0];
    assert_eq!(
        keys(world),
        [
            "connection",
            "drivers",
            "omitted",
            "persona_id",
            "rivals",
            "traffic",
            "traffic_ring",
            "world_ms"
        ]
    );
    assert_eq!(world["persona_id"], "1000000007");
    assert_eq!(
        world["drivers"],
        json!([{"participant": 1, "x": -1234.57, "y": 12.35, "z": 6543.21,
                "heading": 0.0, "speed": 12.35}])
    );
    assert_eq!(
        world["traffic"],
        json!([{"id": 41, "x": 100.0, "y": 0.0, "z": -100.01, "heading": 0.0}]),
        "a heading that rounds to 360 is 0"
    );
    assert_eq!(
        world["rivals"],
        json!([{"id": 7, "x": 1.0, "y": 2.0, "z": 3.0, "heading": null}])
    );
    assert_eq!(
        world["traffic_ring"],
        json!({"inner": 300.0, "outer": 450.0})
    );
    assert_eq!(world["omitted"], 0);
}

/// Standard output that blocks until released, like a reader that stopped
/// draining its pipe.
struct Stalled {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    lines: mpsc::Sender<String>,
}

impl Write for Stalled {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let _ = self.entered.send(());
        self.release
            .lock()
            .unwrap()
            .recv()
            .map_err(std::io::Error::other)?;
        self.lines
            .send(String::from_utf8(bytes.to_vec()).unwrap())
            .map_err(std::io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_stalled_writer_never_holds_up_publishing_and_resumes_with_the_newest() {
    let (board, reader) = Board::new();
    let (release, released) = mpsc::channel();
    let (sent, lines) = mpsc::channel();
    let (enter, entered) = mpsc::channel();
    let out = Stalled {
        entered: enter,
        release: Mutex::new(released),
        lines: sent,
    };
    let writer = std::thread::spawn(move || write(reader, out, Duration::from_millis(5)));
    let publisher = board.publisher(1, 1);
    publisher.publish(car_snapshot(0, 0.));
    let wait = Duration::from_secs(10);
    entered
        .recv_timeout(wait)
        .expect("the writer reached its first line");
    // The writer is now blocked on that line; publishing goes on.
    for step in 1..=200u16 {
        publisher.publish(car_snapshot(u64::from(step), f32::from(step)));
    }
    release.send(()).unwrap();
    let first: Value = serde_json::from_str(&lines.recv_timeout(wait).unwrap()).unwrap();
    assert_eq!(first["frame"], 1);
    assert_eq!(
        first["worlds"][0]["world_ms"], 0,
        "written before the stall"
    );
    release.send(()).unwrap();
    let second: Value = serde_json::from_str(&lines.recv_timeout(wait).unwrap()).unwrap();
    assert_eq!(second["frame"], 2);
    assert_eq!(
        second["worlds"][0]["world_ms"], 200,
        "the 199 snapshots in between were dropped"
    );
    // The board closing ends the writer after its last change.
    drop((board, publisher));
    release.send(()).unwrap();
    let last: Value = serde_json::from_str(&lines.recv_timeout(wait).unwrap()).unwrap();
    assert_eq!(last["worlds"], json!([]));
    writer.join().unwrap();
}

#[test]
fn a_failed_write_ends_the_writer() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (board, reader) = Board::new();
    let publisher = board.publisher(1, 1);
    publisher.publish(Snapshot::new(0));
    let writer = std::thread::spawn(move || write(reader, Broken, Duration::from_millis(1)));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !writer.is_finished() {
        assert!(std::time::Instant::now() < deadline, "writer kept running");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The board outlives its writer; sessions still publish.
    publisher.publish(Snapshot::new(1));
}
