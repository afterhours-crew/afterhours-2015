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
        bytes: Vec<u8>,
    },
}

#[derive(Clone)]
pub struct Recorder {
    sender: mpsc::Sender<Message>,
    origin: Instant,
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
    let handle = thread::Builder::new()
        .name("nfs-server-recorder".into())
        .spawn(move || {
            let mut streams: BTreeMap<PathBuf, (File, u64)> = BTreeMap::new();
            while let Some(message) = receiver.blocking_recv() {
                let Message::Chunk {
                    service,
                    connection,
                    direction,
                    elapsed_ms,
                    route,
                    note,
                    bytes,
                } = message;
                let suffix = match direction {
                    Direction::In => "in",
                    Direction::Out => "out",
                };
                let path = directory.join(format!("{service}-{connection}-{suffix}.bin"));
                if !streams.contains_key(&path) {
                    let file = File::create_new(&path).map_err(|_| Failure::Output)?;
                    streams.insert(path.clone(), (file, 0));
                }
                let (file, offset) = streams.get_mut(&path).ok_or(Failure::Output)?;
                file.write_all(&bytes).map_err(|_| Failure::Output)?;
                let mut line = json!({"t_ms":elapsed_ms,"service":service,"conn":connection,
                    "dir":suffix,"offset":*offset,"len":bytes.len()});
                if let Some(r) = route {
                    line["component"] = json!(r.component);
                    line["command"] = json!(r.command);
                    line["category"] = json!(r.category);
                    line["correlation"] = json!(r.correlation);
                }
                if let Some(note) = note {
                    line["note"] = json!(note);
                }
                *offset += bytes.len() as u64;
                writeln!(events, "{line}").map_err(|_| Failure::Output)?;
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
        },
        RecorderThread { handle },
    ))
}

impl Recorder {
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
                bytes: bytes.to_vec(),
            })
            .await
            .map_err(|_| Failure::Output)
    }
}
