// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::Instant,
};
use tokio::sync::mpsc;

pub const CHANNEL_CAPACITY: usize = 256;
const MAX_CHUNK_BYTES: usize = 1 << 20;

/// Per-run limits. Raw payloads are an explicit, private diagnostic option.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub raw: bool,
    pub max_bytes: u64,
    pub max_events: u64,
    pub max_streams: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            raw: false,
            max_bytes: 64 << 20,
            max_events: 100_000,
            max_streams: 256,
        }
    }
}
impl Options {
    pub fn raw() -> Self {
        Self {
            raw: true,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    In,
    Out,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Route {
    pub component: u16,
    pub command: u16,
    pub category: u8,
    pub correlation: u32,
}

impl Route {
    pub fn of(wire: &[u8]) -> Option<Self> {
        let decoded = nfs_fire2::decode(wire, crate::frame_limits()).ok()??;
        let f = decoded.frame.fields;
        Some(Self {
            component: f.routing_a,
            command: f.routing_b,
            category: f.category,
            correlation: f.correlation,
        })
    }
}

enum Message {
    Chunk {
        service: &'static str,
        connection: u32,
        direction: Direction,
        elapsed_ms: u128,
        route: Option<Route>,
        note: Option<&'static str>,
        length: usize,
        bytes: Vec<u8>,
    },
}

#[derive(Clone)]
pub struct Recorder {
    sender: mpsc::Sender<Message>,
    origin: Instant,
    raw: bool,
    #[cfg(feature = "network-test")]
    test_directory: Option<PathBuf>,
}

pub struct RecorderThread {
    handle: thread::JoinHandle<Result<(), Failure>>,
}

impl RecorderThread {
    pub fn finish(self) -> Result<(), Failure> {
        self.handle.join().map_err(|_| Failure::Output)?
    }
}
pub fn start(root: &Path, output: &Path) -> Result<(Recorder, RecorderThread), Failure> {
    start_with_options(root, output, Options::default())
}

pub fn start_with_options(
    root: &Path,
    output: &Path,
    options: Options,
) -> Result<(Recorder, RecorderThread), Failure> {
    if options.max_bytes < 256 || options.max_events == 0 || options.max_streams == 0 {
        return Err(Failure::Output);
    }
    let artifacts = fs::canonicalize(root.join("artifacts")).map_err(|_| Failure::Output)?;
    let parent = output.parent().ok_or(Failure::Output)?;
    let parent = fs::canonicalize(if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    })
    .map_err(|_| Failure::Output)?;
    if !parent.starts_with(&artifacts) {
        return Err(Failure::Output);
    }
    let directory = parent.join(output.file_name().ok_or(Failure::Output)?);
    fs::create_dir(&directory).map_err(|_| Failure::Output)?;
    let mut events =
        File::create_new(directory.join("events.jsonl")).map_err(|_| Failure::Output)?;
    let (sender, mut receiver) = mpsc::channel::<Message>(CHANNEL_CAPACITY);
    #[cfg(feature = "network-test")]
    let test_directory = options.raw.then(|| directory.clone());
    let handle = thread::Builder::new()
        .name("nfs-server-recorder".into())
        .spawn(move || {
            let mut streams: BTreeMap<PathBuf, (File, u64)> = BTreeMap::new();
            let (mut total_bytes, mut total_events, mut exhausted) = (0u64, 0u64, false);
            while let Some(message) = receiver.blocking_recv() {
                if exhausted {
                    continue;
                }
                let Message::Chunk {
                    service,
                    connection,
                    direction,
                    elapsed_ms,
                    route,
                    note,
                    length,
                    bytes,
                } = message;
                let suffix = match direction {
                    Direction::In => "in",
                    Direction::Out => "out",
                };
                let path = directory.join(format!("{service}-{connection}-{suffix}.bin"));
                let mut line = json!({"t_ms":elapsed_ms,"service":service,"conn":connection,
                    "dir":suffix,"len":length});
                if options.raw {
                    line["offset"] = json!(streams.get(&path).map_or(0, |(_, offset)| *offset));
                }
                if let Some(r) = route {
                    line["component"] = json!(r.component);
                    line["command"] = json!(r.command);
                    line["category"] = json!(r.category);
                    line["correlation"] = json!(r.correlation);
                }
                if let Some(note) = note {
                    line["note"] = json!(note);
                }
                let encoded = format!("{line}\n");
                let needed = encoded.len() as u64 + bytes.len() as u64;
                // Reserve space for one final marker. Exhaustion stops recording,
                // never gameplay, and does not leave a partial protocol chunk.
                if total_events >= options.max_events
                    || total_bytes.saturating_add(needed).saturating_add(128) > options.max_bytes
                    || (options.raw
                        && (length != bytes.len()
                            || (!streams.contains_key(&path)
                                && streams.len() >= options.max_streams)))
                {
                    writeln!(events, "{{\"recording_stopped\":\"limit\"}}")
                        .map_err(|_| Failure::Output)?;
                    exhausted = true;
                    continue;
                }
                if options.raw {
                    if !streams.contains_key(&path) {
                        let file = File::create_new(&path).map_err(|_| Failure::Output)?;
                        streams.insert(path.clone(), (file, 0));
                    }
                    let (file, offset) = streams.get_mut(&path).ok_or(Failure::Output)?;
                    file.write_all(&bytes).map_err(|_| Failure::Output)?;
                    *offset += bytes.len() as u64;
                }
                events
                    .write_all(encoded.as_bytes())
                    .map_err(|_| Failure::Output)?;
                total_bytes += needed;
                total_events += 1;
            }
            for (_, (file, _)) in streams {
                file.sync_all().map_err(|_| Failure::Output)?;
            }
            events.sync_all().map_err(|_| Failure::Output)
        })
        .map_err(|_| Failure::Io)?;
    Ok((
        Recorder {
            sender,
            origin: Instant::now(),
            raw: options.raw,
            #[cfg(feature = "network-test")]
            test_directory,
        },
        RecorderThread { handle },
    ))
}

/// A recorder whose bounded queue is full and never drained: every chunk waits.
#[cfg(test)]
pub(crate) fn full_for_tests() -> (Recorder, Box<dyn std::any::Any>) {
    let (sender, receiver) = mpsc::channel(1);
    sender
        .try_send(Message::Chunk {
            service: "world",
            connection: 0,
            direction: Direction::In,
            elapsed_ms: 0,
            route: None,
            note: None,
            length: 0,
            bytes: Vec::new(),
        })
        .unwrap_or_else(|_| panic!("empty test queue"));
    let recorder = Recorder {
        sender,
        origin: Instant::now(),
        raw: false,
        #[cfg(feature = "network-test")]
        test_directory: None,
    };
    (recorder, Box::new(receiver))
}

impl Recorder {
    #[cfg(feature = "network-test")]
    pub(crate) fn loss_trigger(&self, connection: u32) -> Option<PathBuf> {
        self.test_directory
            .as_ref()
            .map(|directory| directory.join(format!("world-{connection}-loss.txt")))
    }

    pub async fn chunk(
        &self,
        service: &'static str,
        connection: u32,
        direction: Direction,
        bytes: &[u8],
        note: Option<&'static str>,
    ) -> Result<(), Failure> {
        let route = if service == "blaze" {
            Route::of(bytes)
        } else {
            None
        };
        self.sender
            .send(Message::Chunk {
                service,
                connection,
                direction,
                elapsed_ms: self.origin.elapsed().as_millis(),
                route,
                note,
                length: bytes.len(),
                bytes: if self.raw && bytes.len() <= MAX_CHUNK_BYTES {
                    bytes.to_vec()
                } else {
                    Vec::new()
                },
            })
            .await
            .map_err(|_| Failure::Output)
    }
}
