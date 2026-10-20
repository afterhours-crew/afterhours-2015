// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::events::Body;
use serde_json::{Value, json};

/// A server line as `nfs-server --map-feed` writes one.
pub(crate) fn sample(frame: u64) -> Value {
    json!({
        "map_feed": 1,
        "frame": frame,
        "worlds": [{
            "connection": 1,
            "persona_id": "1000000007",
            "world_ms": 5000,
            "drivers": [
                {"participant": 1, "x": -1234.57, "y": 12.35, "z": 6543.21,
                 "heading": 90.5, "speed": 12.35}
            ],
            "traffic": [
                {"id": 3, "x": 100.0, "y": 0.0, "z": -100.01, "heading": 0.0},
                {"id": 8, "x": 1.0, "y": 2.0, "z": 3.0, "heading": 359.9}
            ],
            "rivals": [
                {"id": 2, "x": 5.0, "y": 0.0, "z": 5.0, "heading": null}
            ],
            "traffic_ring": {"inner": 300.0, "outer": 450.0},
            "omitted": 0
        }]
    })
}

pub(crate) fn sample_line(frame: u64) -> String {
    sample(frame).to_string()
}

fn rejected(change: impl FnOnce(&mut Value)) -> String {
    let mut line = sample(1);
    change(&mut line);
    parse(&line.to_string()).expect_err("rejected")
}

#[test]
fn a_server_line_reads_as_typed_worlds() {
    let frame = parse(&sample_line(7)).unwrap();
    assert_eq!((frame.map_feed, frame.frame), (1, 7));
    let [world] = &frame.worlds[..] else {
        panic!("one world")
    };
    assert_eq!(
        (world.connection, world.persona_id.as_str(), world.world_ms),
        (1, "1000000007", 5000)
    );
    assert_eq!(
        world.drivers,
        [MapDriver {
            participant: 1,
            x: -1234.57,
            y: 12.35,
            z: 6543.21,
            heading: Some(90.5),
            speed: 12.35,
        }]
    );
    assert_eq!(
        world.traffic.iter().map(|c| c.id).collect::<Vec<_>>(),
        [3, 8]
    );
    assert_eq!(world.rivals[0].heading, None);
    assert_eq!(
        world.traffic_ring,
        Some(TrafficRing {
            inner: 300.,
            outer: 450.
        })
    );
    // Fields a later server adds are ignored.
    let mut newer = sample(8);
    newer["extra"] = json!(true);
    newer["worlds"][0]["drivers"][0]["extra"] = json!(1);
    assert!(parse(&newer.to_string()).is_ok());
    // A server with no world session running.
    let mut empty = sample(9);
    empty["worlds"] = json!([]);
    assert!(parse(&empty.to_string()).unwrap().worlds.is_empty());
}

#[test]
fn a_line_outside_the_bounds_is_refused() {
    let ok = |w: &mut Value, key: &str, value: Value| w["worlds"][0][key] = value;
    for (why, error) in [
        ("format", rejected(|l| l["map_feed"] = json!(2))),
        ("frame zero", rejected(|l| l["frame"] = json!(0))),
        ("negative frame", rejected(|l| l["frame"] = json!(-1))),
        (
            "too many worlds",
            rejected(|l| {
                let world = l["worlds"][0].clone();
                l["worlds"] = json!(
                    (0..=MAX_WORLDS)
                        .map(|c| {
                            let mut w = world.clone();
                            w["connection"] = json!(c);
                            w
                        })
                        .collect::<Vec<_>>()
                );
            }),
        ),
        (
            "repeated connection",
            rejected(|l| {
                let world = l["worlds"][0].clone();
                l["worlds"] = json!([world.clone(), world]);
            }),
        ),
        ("persona", rejected(|l| ok(l, "persona_id", json!("12a")))),
        (
            "empty persona",
            rejected(|l| ok(l, "persona_id", json!(""))),
        ),
        (
            "long persona",
            rejected(|l| ok(l, "persona_id", json!("1".repeat(21)))),
        ),
        (
            "too many drivers",
            rejected(|l| {
                ok(
                    l,
                    "drivers",
                    json!(
                        (0..=MAX_DRIVERS)
                            .map(|p| json!({"participant": p, "x": 0, "y": 0, "z": 0,
                                            "heading": null, "speed": 0}))
                            .collect::<Vec<_>>()
                    ),
                )
            }),
        ),
        (
            "too many cars",
            rejected(|l| {
                ok(
                    l,
                    "rivals",
                    json!(
                        (0..=MAX_CARS)
                            .map(|id| json!({"id": id, "x": 0, "y": 0, "z": 0, "heading": null}))
                            .collect::<Vec<_>>()
                    ),
                )
            }),
        ),
        (
            "unordered ids",
            rejected(|l| {
                let cars = l["worlds"][0]["traffic"].as_array_mut().unwrap();
                cars.reverse();
            }),
        ),
        (
            "repeated participant",
            rejected(|l| {
                let driver = l["worlds"][0]["drivers"][0].clone();
                ok(l, "drivers", json!([driver.clone(), driver]));
            }),
        ),
        (
            "far away",
            rejected(|l| l["worlds"][0]["traffic"][0]["x"] = json!(MAX_COORDINATE * 2.)),
        ),
        (
            "heading 360",
            rejected(|l| l["worlds"][0]["drivers"][0]["heading"] = json!(360.0)),
        ),
        (
            "negative heading",
            rejected(|l| l["worlds"][0]["rivals"][0]["heading"] = json!(-0.1)),
        ),
        (
            "negative speed",
            rejected(|l| l["worlds"][0]["drivers"][0]["speed"] = json!(-1.0)),
        ),
        (
            "unsafe id",
            rejected(|l| l["worlds"][0]["rivals"][0]["id"] = json!(MAX_ID + 1)),
        ),
        (
            "ring",
            rejected(|l| ok(l, "traffic_ring", json!({"inner": 500.0, "outer": 450.0}))),
        ),
        (
            "missing field",
            rejected(|l| {
                l["worlds"][0].as_object_mut().unwrap().remove("omitted");
            }),
        ),
        ("not a frame", parse("{\"ready\":true}").unwrap_err()),
        ("not JSON", parse("fake server starting").unwrap_err()),
    ] {
        assert!(error.starts_with("map "), "{why}: {error}");
    }
    let mut huge = sample(1);
    huge["padding"] = json!("x".repeat(MAX_LINE_BYTES));
    assert_eq!(
        parse(&huge.to_string()).unwrap_err(),
        "map frame exceeds its bound"
    );
}

#[test]
fn the_relay_only_moves_forward_and_counts_what_it_skipped() {
    let mut relay = Relay::default();
    let frames = |body: Option<Body>| match body {
        Some(Body::MapFeed { frame, skipped, .. }) => Some((frame, skipped)),
        _ => None,
    };
    // Frames 1 and 2 were superseded before the first poll.
    assert_eq!(frames(relay.accept(&sample_line(3))), Some((3, 2)));
    assert_eq!(frames(relay.accept(&sample_line(4))), Some((4, 0)));
    // The same line again, or an older one, is never sent.
    assert_eq!(frames(relay.accept(&sample_line(4))), None);
    assert_eq!(frames(relay.accept(&sample_line(2))), None);
    // An unreadable line is dropped; the next frame counts it as skipped.
    assert_eq!(frames(relay.accept("{\"map_feed\":1,\"frame\":5")), None);
    assert_eq!(frames(relay.accept(&sample_line(6))), Some((6, 1)));
    let Some(Body::MapFeed { worlds, .. }) = relay.accept(&sample_line(9)) else {
        panic!("a map-feed event")
    };
    assert_eq!(worlds, parse(&sample_line(9)).unwrap().worlds);
}

#[test]
fn the_latch_keeps_the_newest_complete_line() {
    let mut latch = Latch::default();
    assert_eq!(latch.take(), None);
    latch.push(b"{\"frame\":1}\n{\"frame\":2}\r\n{\"fra");
    assert_eq!(latch.take().as_deref(), Some("{\"frame\":2}"));
    assert_eq!(latch.take(), None, "taken once");
    // A line split across reads is joined.
    latch.push(b"me\":3}");
    assert_eq!(latch.take(), None, "not complete yet");
    latch.push(b"\n");
    assert_eq!(latch.take().as_deref(), Some("{\"frame\":3}"));
}

#[test]
fn the_latch_discards_a_line_over_its_bound_up_to_its_end() {
    let mut latch = Latch::default();
    latch.push(b"old\n");
    // Exactly at the bound is kept.
    latch.push(&vec![b'a'; MAX_LINE_BYTES]);
    latch.push(b"\n");
    assert_eq!(latch.take().map(|l| l.len()), Some(MAX_LINE_BYTES));
    // One byte more, over two reads, is dropped; the line before it stays.
    latch.push(b"kept\n");
    latch.push(&vec![b'b'; MAX_LINE_BYTES]);
    latch.push(b"b");
    latch.push(&vec![b'b'; 4096]);
    latch.push(b"\nnext");
    assert_eq!(latch.take().as_deref(), Some("kept"));
    // The stream recovers at the next line.
    latch.push(b" line\n");
    assert_eq!(latch.take().as_deref(), Some("next line"));
    // An over-long line ending in the same read as it overflows.
    let mut long = vec![b'c'; MAX_LINE_BYTES + 1];
    long.extend_from_slice(b"\nafter\n");
    latch.push(&long);
    assert_eq!(latch.take().as_deref(), Some("after"));
}
