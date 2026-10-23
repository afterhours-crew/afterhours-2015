// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Live positions for the map: the `map-feed` event.
//!
//! With `--events`, the supervisor starts the server with `--map-feed`. The
//! server then writes, after its readiness line, one JSON line on standard
//! output at most four times a second while a world session runs: its
//! players' cars, traffic and roaming rivals, read from the state it owns.
//! The server's standard output pump keeps only the newest complete line
//! ([`Latch`]) and writes none of them to `server.stdout.jsonl`. The
//! supervisor's poll, every 250 ms, checks that line ([`parse`]) and emits it
//! as a `map-feed` event when it is newer than the last one ([`Relay`]).
//! A reader that falls behind therefore skips frames rather than queueing
//! them, and `skipped` says how many.
//!
//! World positions are in metres; on the game's world map (`map-images`)
//! +x points right and +z points down, so a position maps to u = (x -
//! min_x) / (max_x - min_x) and v = (z - min_z) / (max_z - min_z) of the
//! reported world box. Headings are degrees clockwise from up the map.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Version of the server's line format this launcher reads.
pub const SERVER_FORMAT: u32 = 1;
/// Longest server line kept; longer lines are discarded.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// World sessions per frame: the server's connection limit.
pub const MAX_WORLDS: usize = 32;
/// Players' cars per world session.
pub const MAX_DRIVERS: usize = 16;
/// Traffic cars, and rivals, per world session: the world's AI budget.
pub const MAX_CARS: usize = 128;
/// A coordinate farther from the origin is not a world position.
pub const MAX_COORDINATE: f64 = 100_000.;
/// Largest id: JavaScript keeps every digit up to here.
pub const MAX_ID: u64 = (1 << 53) - 1;

/// One world session: the server's view of one game client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MapWorld {
    /// The server's control connection of this game client; one world
    /// session each. Unique and ascending within a frame.
    pub connection: u32,
    /// The player's persona: `persona_id` in `profile` for the same driver.
    pub persona_id: String,
    /// Milliseconds since this world session started, when it was observed.
    pub world_ms: u64,
    /// The players' cars driving in the world, by `participant`.
    pub drivers: Vec<MapDriver>,
    /// Ambient traffic cars, by `id`.
    pub traffic: Vec<MapCar>,
    /// Roaming rivals, by `id`: the car while the game drives it, otherwise
    /// the server's simulation of the racer on the road network.
    pub rivals: Vec<MapCar>,
    /// The radii the native traffic policy uses around each driver; `null`
    /// without traffic or while it is off.
    pub traffic_ring: Option<TrafficRing>,
    /// Cars the server left out of this frame: no usable position, state
    /// that could not be read, or beyond a bound.
    pub omitted: u32,
}

/// A player's car.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MapDriver {
    /// The player's participant in its world session. Unique within it.
    pub participant: u16,
    /// World x in metres, to the centimetre: +x points right on the map.
    pub x: f64,
    /// Height in metres, to the centimetre.
    pub y: f64,
    /// World z in metres, to the centimetre: +z points down the map.
    pub z: f64,
    /// Degrees clockwise from up the map, `0 <= heading < 360`, to a tenth;
    /// `null` when unknown.
    pub heading: Option<f64>,
    /// Metres per second.
    pub speed: f64,
}

/// A traffic car or a roaming rival.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MapCar {
    /// Unique within its list for the world session, never reused there.
    pub id: u64,
    /// World x in metres, to the centimetre: +x points right on the map.
    pub x: f64,
    /// Height in metres, to the centimetre.
    pub y: f64,
    /// World z in metres, to the centimetre: +z points down the map.
    pub z: f64,
    /// Degrees clockwise from up the map, `0 <= heading < 360`, to a tenth;
    /// `null` when unknown.
    pub heading: Option<f64>,
}

/// Horizontal metres around each driver. Cars are created between `inner`
/// and a little more than `outer` ahead of where the driver will be in about
/// three seconds (a quarter of that to the sides), and kept while within
/// `outer` of the driver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrafficRing {
    /// Metres; cars are created beyond this.
    pub inner: f64,
    /// Metres; cars are kept within this.
    pub outer: f64,
}

/// One frame of the server's map feed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Frame {
    /// The server's line format.
    pub map_feed: u32,
    /// One more for every line the server wrote.
    pub frame: u64,
    /// By connection.
    pub worlds: Vec<MapWorld>,
}

/// Read and check one line of the server's map feed.
pub fn parse(line: &str) -> Result<Frame, String> {
    if line.len() > MAX_LINE_BYTES {
        return Err("map frame exceeds its bound".into());
    }
    let frame: Frame =
        serde_json::from_str(line).map_err(|e| format!("map frame is not readable: {e}"))?;
    if frame.map_feed != SERVER_FORMAT {
        return Err(format!(
            "map frame format {} is not supported",
            frame.map_feed
        ));
    }
    if frame.frame == 0 {
        return Err("map frame numbers start at 1".into());
    }
    if frame.worlds.len() > MAX_WORLDS {
        return Err("map frame has too many worlds".into());
    }
    ascending(
        frame.worlds.iter().map(|w| u64::from(w.connection)),
        "connection",
    )?;
    for world in &frame.worlds {
        check_world(world)?;
    }
    Ok(frame)
}

fn check_world(world: &MapWorld) -> Result<(), String> {
    if world.persona_id.is_empty()
        || world.persona_id.len() > 20
        || !world.persona_id.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("map world persona is not a decimal ID".into());
    }
    if world.drivers.len() > MAX_DRIVERS
        || world.traffic.len() > MAX_CARS
        || world.rivals.len() > MAX_CARS
    {
        return Err("map world has too many cars".into());
    }
    ascending(
        world.drivers.iter().map(|d| u64::from(d.participant)),
        "participant",
    )?;
    for driver in &world.drivers {
        place(driver.x, driver.y, driver.z, driver.heading)?;
        if !driver.speed.is_finite() || driver.speed < 0. {
            return Err("map driver speed is not a speed".into());
        }
    }
    for cars in [&world.traffic, &world.rivals] {
        ascending(cars.iter().map(|c| c.id), "id")?;
        for car in cars {
            if car.id > MAX_ID {
                return Err("map car id exceeds its bound".into());
            }
            place(car.x, car.y, car.z, car.heading)?;
        }
    }
    if let Some(ring) = &world.traffic_ring
        && !(ring.inner.is_finite()
            && ring.outer.is_finite()
            && 0. < ring.inner
            && ring.inner <= ring.outer
            && ring.outer <= MAX_COORDINATE)
    {
        return Err("map traffic ring is not a ring".into());
    }
    Ok(())
}

fn place(x: f64, y: f64, z: f64, heading: Option<f64>) -> Result<(), String> {
    if [x, y, z]
        .iter()
        .any(|v| !v.is_finite() || v.abs() > MAX_COORDINATE)
    {
        return Err("map position is outside the world".into());
    }
    if heading.is_some_and(|h| !(0. ..360.).contains(&h)) {
        return Err("map heading is not between 0 and 360".into());
    }
    Ok(())
}

fn ascending(values: impl Iterator<Item = u64>, name: &str) -> Result<(), String> {
    let mut previous = None;
    for value in values {
        if previous.is_some_and(|p| p >= value) {
            return Err(format!("map {name}s are not unique and ascending"));
        }
        previous = Some(value);
    }
    Ok(())
}

/// Decides which server frames become events: only readable frames newer
/// than the last one sent, so the stream never goes back in time.
#[derive(Debug, Default)]
pub struct Relay {
    last: Option<u64>,
}

impl Relay {
    /// The event for the newest server line, if it should be sent.
    pub fn accept(&mut self, line: &str) -> Option<crate::events::Body> {
        let frame = parse(line).ok()?;
        if self.last.is_some_and(|last| frame.frame <= last) {
            return None;
        }
        // Frames the server wrote since the last event, which this stream
        // did not deliver: superseded before a poll, or unreadable.
        let skipped = frame.frame - self.last.unwrap_or(0) - 1;
        self.last = Some(frame.frame);
        Some(crate::events::Body::MapFeed {
            frame: frame.frame,
            skipped,
            worlds: frame.worlds,
        })
    }
}

/// The newest complete line of a byte stream, kept within
/// [`MAX_LINE_BYTES`]. A longer line is discarded up to its end.
#[derive(Debug, Default)]
pub struct Latch {
    partial: Vec<u8>,
    overflow: bool,
    newest: Option<Vec<u8>>,
}

impl Latch {
    /// Take more of the stream.
    pub fn push(&mut self, mut bytes: &[u8]) {
        while let Some(at) = bytes.iter().position(|&b| b == b'\n') {
            if !self.overflow && self.partial.len() + at <= MAX_LINE_BYTES {
                self.partial.extend_from_slice(&bytes[..at]);
                self.newest = Some(std::mem::take(&mut self.partial));
            }
            self.partial.clear();
            self.overflow = false;
            bytes = &bytes[at + 1..];
        }
        if self.overflow {
            return;
        }
        if self.partial.len() + bytes.len() > MAX_LINE_BYTES {
            self.partial = Vec::new();
            self.overflow = true;
        } else {
            self.partial.extend_from_slice(bytes);
        }
    }

    /// The newest complete line since the last call, without its line ending.
    pub fn take(&mut self) -> Option<String> {
        self.newest.take().map(|line| {
            String::from_utf8_lossy(&line)
                .trim_end_matches('\r')
                .to_owned()
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
