// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only live positions for the launcher's map (`--map-feed`).
//!
//! Each world session observes state it already owns, at a tick boundary and
//! at most every [`INTERVAL_MS`]: its players' cars, its ambient traffic and
//! its roaming rivals. The observation is a bounded [`Snapshot`]; building,
//! pacing and encoding it are sans-IO. Nothing here changes game state.
//!
//! Sessions publish to one [`Board`], a latest-value slot per world session
//! (a watch channel). Publishing never waits for a reader, and no lock is held
//! around game traffic: the slot is replaced in one short update. [`write`]
//! prints the board as one JSON line at most every [`INTERVAL_MS`], on its own
//! thread. A reader that falls behind blocks only that thread; sessions keep
//! replacing their snapshots and the next line carries the newest state, so a
//! slow reader drops frames instead of queueing them. `frame` counts the lines
//! written, so a reader that keeps only the newest line sees what it skipped.
//!
//! Axes and units are those of the replicated state: positions are absolute
//! world coordinates in metres (the units of the population radii and of the
//! map box), y is up, and on the game's world map (`map-images`) +x points
//! right and +z points down. A heading is the car's forward axis, the third
//! basis vector of its rotation (traffic is created with it along the road),
//! in degrees clockwise from up the map. Speed is the length of the reported
//! velocity in metres per second.
use std::{collections::BTreeMap, io::Write, sync::Arc, time::Duration};
use tokio::sync::watch;

/// Version of the line format.
pub const FORMAT: u32 = 1;
/// Shortest time between two snapshots of a session, and between two lines.
pub const INTERVAL_MS: u64 = 250;
/// World sessions on the board: one per control connection.
pub const MAX_WORLDS: usize = crate::net::MAX_CONNECTIONS;
/// Players' cars per session.
pub const MAX_DRIVERS: usize = 16;
/// Traffic cars, and rivals, per session: the world's AI budget.
pub const MAX_CARS: usize = nfs_world::replication::players::ai::MAX_AI;
/// A coordinate farther from the origin is not a position (the traffic
/// focus bound); the map itself spans 8,192 m either side.
const LIMIT: f32 = 100_000.;

/// Where a car is and which way it points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: [f32; 3],
    /// Degrees clockwise from up the map, `0 <= heading < 360`; `None` when
    /// the forward axis is not usable or points straight up or down.
    pub heading: Option<f32>,
}

impl Pose {
    /// `None` when the position is not finite or not within the world.
    pub fn new(position: [f32; 3], forward: [f32; 3]) -> Option<Self> {
        position
            .iter()
            .all(|v| v.is_finite() && v.abs() <= LIMIT)
            .then(|| Self {
                position,
                heading: heading(forward),
            })
    }
}

/// Map heading of a forward axis: up the map (-z) is 0, right (+x) is 90.
pub fn heading(forward: [f32; 3]) -> Option<f32> {
    let [x, _, z] = forward;
    if !x.is_finite() || !z.is_finite() || x.hypot(z) < 1e-3 {
        return None;
    }
    let degrees = x.atan2(-z).to_degrees().rem_euclid(360.);
    // A tiny negative angle rounds to 360 in f32.
    Some(if degrees >= 360. { 0. } else { degrees })
}

/// A player's car.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Driver {
    /// The player's participant in this world session.
    pub participant: u16,
    pub pose: Pose,
    /// Metres per second.
    pub speed: f32,
}

/// A traffic car or a roaming rival.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Car {
    /// Unique within its list for the session, never reused: the host's
    /// spawn key of a traffic car, or the slot key of a rival.
    pub id: u64,
    pub pose: Pose,
}

/// The fixed radii of the native traffic policy, horizontal metres around
/// each driver: cars are created between `inner` and a little more than
/// `outer` ahead of where the driver will be in three seconds (a quarter of
/// that to the sides) and kept while within `outer` of the driver.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub inner: f32,
    pub outer: f32,
}

/// What one world session shows on the map at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// The session's world clock, milliseconds since its world host started.
    pub world_ms: u64,
    /// By participant.
    pub drivers: Vec<Driver>,
    /// By id.
    pub traffic: Vec<Car>,
    /// By id.
    pub rivals: Vec<Car>,
    /// `None` without traffic, or while traffic is off.
    pub traffic_ring: Option<Ring>,
    /// Cars left out: no usable position, state that could not be read, a
    /// repeated id, or beyond a bound.
    pub omitted: u32,
}

impl Snapshot {
    pub fn new(world_ms: u64) -> Self {
        Self {
            world_ms,
            ..Self::default()
        }
    }

    pub fn driver(
        &mut self,
        participant: u16,
        position: [f32; 3],
        velocity: [f32; 3],
        forward: [f32; 3],
    ) {
        let speed = velocity.iter().map(|v| v * v).sum::<f32>().sqrt();
        match Pose::new(position, forward) {
            Some(pose) if self.drivers.len() < MAX_DRIVERS && speed.is_finite() => {
                self.drivers.push(Driver {
                    participant,
                    pose,
                    speed,
                })
            }
            _ => self.omit(),
        }
    }

    pub fn traffic(&mut self, id: u64, position: [f32; 3], forward: [f32; 3]) {
        if !push(&mut self.traffic, id, position, forward) {
            self.omit();
        }
    }

    pub fn rival(&mut self, id: u64, position: [f32; 3], forward: [f32; 3]) {
        if !push(&mut self.rivals, id, position, forward) {
            self.omit();
        }
    }

    /// Count a car whose state could not be read.
    pub fn omit(&mut self) {
        self.omitted = self.omitted.saturating_add(1);
    }

    /// Order every list by id and keep the first of a repeated id, so a
    /// reader can key markers by id.
    pub fn finish(mut self) -> Self {
        let before = self.drivers.len() + self.traffic.len() + self.rivals.len();
        self.drivers.sort_by_key(|d| d.participant);
        self.drivers.dedup_by_key(|d| d.participant);
        for cars in [&mut self.traffic, &mut self.rivals] {
            cars.sort_by_key(|c| c.id);
            cars.dedup_by_key(|c| c.id);
        }
        let after = self.drivers.len() + self.traffic.len() + self.rivals.len();
        self.omitted = self
            .omitted
            .saturating_add(u32::try_from(before - after).unwrap_or(u32::MAX));
        self
    }
}

fn push(cars: &mut Vec<Car>, id: u64, position: [f32; 3], forward: [f32; 3]) -> bool {
    match Pose::new(position, forward) {
        Some(pose) if cars.len() < MAX_CARS => {
            cars.push(Car { id, pose });
            true
        }
        _ => false,
    }
}

/// Pace snapshots at most every [`INTERVAL_MS`], keeping the original phase
/// when a tick runs late so late ticks neither drift nor catch up in a burst.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pacing {
    last: Option<u64>,
}

impl Pacing {
    /// Whether a snapshot is due at `now_ms`; marks it taken when it is.
    pub fn take(&mut self, now_ms: u64) -> bool {
        match self.last {
            Some(last) if now_ms < last.saturating_add(INTERVAL_MS) => false,
            Some(last) => {
                self.last = Some(last + (now_ms - last) / INTERVAL_MS * INTERVAL_MS);
                true
            }
            None => {
                self.last = Some(now_ms);
                true
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    persona: u64,
    snapshot: Arc<Snapshot>,
}

type Worlds = BTreeMap<u32, Entry>;

/// The newest snapshot of every world session. Cloned into each session.
#[derive(Clone, Debug)]
pub struct Board {
    sender: watch::Sender<Worlds>,
}

impl Board {
    /// A board and the reader that prints it.
    pub fn new() -> (Self, Reader) {
        let (sender, receiver) = watch::channel(Worlds::new());
        (Self { sender }, Reader { receiver, frame: 0 })
    }

    /// The publisher of one world session; its snapshot leaves the board
    /// when the publisher is dropped, as the session ends.
    pub fn publisher(&self, connection: u32, persona: u64) -> Publisher {
        Publisher {
            board: self.clone(),
            connection,
            persona,
            pacing: Pacing::default(),
        }
    }
}

/// One world session's slot on the [`Board`].
#[derive(Debug)]
pub struct Publisher {
    board: Board,
    connection: u32,
    persona: u64,
    pacing: Pacing,
}

impl Publisher {
    /// Whether a snapshot is due; marks it taken when it is.
    pub fn due(&mut self, now_ms: u64) -> bool {
        self.pacing.take(now_ms)
    }

    /// Replace this session's snapshot. A session beyond [`MAX_WORLDS`] is
    /// left off the board.
    pub fn publish(&self, snapshot: Snapshot) {
        let entry = Entry {
            persona: self.persona,
            snapshot: Arc::new(snapshot.finish()),
        };
        self.board.sender.send_if_modified(|worlds| {
            if worlds.len() >= MAX_WORLDS && !worlds.contains_key(&self.connection) {
                return false;
            }
            worlds.insert(self.connection, entry);
            true
        });
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.board
            .sender
            .send_if_modified(|worlds| worlds.remove(&self.connection).is_some());
    }
}

/// Reads the [`Board`] and encodes it as lines.
#[derive(Debug)]
pub struct Reader {
    receiver: watch::Receiver<Worlds>,
    frame: u64,
}

impl Reader {
    /// The next line, without its line ending, when the board changed since
    /// the last one: every session's newest snapshot, by connection. Changes
    /// in between are not kept. `Err` once every [`Board`] is gone and its
    /// last change has been read.
    pub fn next_line(&mut self) -> Result<Option<String>, Closed> {
        let closed = self.receiver.has_changed().is_err();
        let worlds = {
            let current = self.receiver.borrow_and_update();
            if !current.has_changed() {
                return if closed { Err(Closed) } else { Ok(None) };
            }
            current.clone()
        };
        self.frame += 1;
        Ok(Some(encode(self.frame, &worlds)))
    }
}

/// Every [`Board`] was dropped: the server is stopping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Closed;

/// Print the board to `out` until the server stops or a write fails: at most
/// one line every `interval`, and only when it changed. Blocks; run it on
/// its own thread.
pub fn write(mut reader: Reader, mut out: impl Write, interval: Duration) {
    loop {
        std::thread::sleep(interval);
        match reader.next_line() {
            Ok(Some(mut line)) => {
                line.push('\n');
                if out
                    .write_all(line.as_bytes())
                    .and_then(|()| out.flush())
                    .is_err()
                {
                    return;
                }
            }
            Ok(None) => {}
            Err(Closed) => return,
        }
    }
}

/// One line: `{"map_feed":1,"frame":N,"worlds":[...]}`. Positions are
/// rounded to centimetres, headings to tenths of a degree and speeds to
/// centimetres per second.
fn encode(frame: u64, worlds: &Worlds) -> String {
    use serde_json::{Value, json};
    let round = |value: f32, scale: f64| (f64::from(value) * scale).round() / scale;
    let heading = |pose: &Pose| match pose.heading.map(|h| round(h, 10.)) {
        Some(h) if h >= 360. => json!(0.),
        Some(h) => json!(h),
        None => Value::Null,
    };
    let place = |pose: &Pose| pose.position.map(|v| round(v, 100.));
    let cars = |cars: &[Car]| {
        cars.iter()
            .map(|car| {
                let [x, y, z] = place(&car.pose);
                json!({"id": car.id, "x": x, "y": y, "z": z, "heading": heading(&car.pose)})
            })
            .collect::<Vec<_>>()
    };
    let worlds: Vec<Value> = worlds
        .iter()
        .map(|(connection, entry)| {
            let snapshot = &entry.snapshot;
            let drivers: Vec<Value> = snapshot
                .drivers
                .iter()
                .map(|driver| {
                    let [x, y, z] = place(&driver.pose);
                    json!({
                        "participant": driver.participant,
                        "x": x, "y": y, "z": z,
                        "heading": heading(&driver.pose),
                        "speed": round(driver.speed, 100.),
                    })
                })
                .collect();
            json!({
                "connection": connection,
                "persona_id": entry.persona.to_string(),
                "world_ms": snapshot.world_ms,
                "drivers": drivers,
                "traffic": cars(&snapshot.traffic),
                "rivals": cars(&snapshot.rivals),
                "traffic_ring": snapshot.traffic_ring.map(|ring| json!({
                    "inner": f64::from(ring.inner),
                    "outer": f64::from(ring.outer),
                })),
                "omitted": snapshot.omitted,
            })
        })
        .collect();
    json!({"map_feed": FORMAT, "frame": frame, "worlds": worlds}).to_string()
}

#[cfg(test)]
mod tests;
