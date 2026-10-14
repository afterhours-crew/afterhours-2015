// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_server::record::{self, Direction, Options};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Run(PathBuf);
impl Run {
    fn new() -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nfs-record-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join("artifacts")).unwrap();
        Self(path)
    }
    fn output(&self) -> PathBuf {
        self.0.join("artifacts/run")
    }
    fn events(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.output().join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
}
impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn default_diagnostics_never_persist_payloads_or_create_binary_streams() {
    let run = Run::new();
    let (recorder, worker) = record::start(&run.0, &run.output()).unwrap();
    recorder
        .chunk(
            "auxiliary",
            1,
            Direction::In,
            b"private-authentication-input",
            None,
        )
        .await
        .unwrap();
    drop(recorder);
    worker.finish().unwrap();
    let files: Vec<_> = std::fs::read_dir(run.output())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(files, ["events.jsonl"]);
    let events = run.events();
    assert_eq!(events[0]["len"], 28);
    assert!(events[0].get("offset").is_none());
    assert!(!events[0].to_string().contains("private-authentication"));
}

#[tokio::test]
async fn raw_capture_is_explicit_and_retains_exact_bytes_until_event_limit() {
    let run = Run::new();
    let (recorder, worker) = record::start_with_options(
        &run.0,
        &run.output(),
        Options {
            max_events: 1,
            ..Options::raw()
        },
    )
    .unwrap();
    for bytes in [b"first".as_slice(), b"second", b"third"] {
        recorder
            .chunk("auxiliary", 1, Direction::In, bytes, None)
            .await
            .unwrap();
    }
    drop(recorder);
    worker.finish().unwrap();
    assert_eq!(
        std::fs::read(run.output().join("auxiliary-1-in.bin")).unwrap(),
        b"first"
    );
    assert_eq!(run.events().len(), 2);
    assert_eq!(run.events()[1]["recording_stopped"], "limit");
}

#[tokio::test]
async fn byte_and_stream_limits_stop_recording_without_failing_traffic() {
    for options in [
        Options {
            max_bytes: 300,
            ..Options::raw()
        },
        Options {
            max_streams: 1,
            ..Options::raw()
        },
    ] {
        let run = Run::new();
        let (recorder, worker) =
            record::start_with_options(&run.0, &run.output(), options).unwrap();
        recorder
            .chunk("auxiliary", 1, Direction::In, b"one", None)
            .await
            .unwrap();
        recorder
            .chunk("auxiliary", 2, Direction::In, &[7; 300], None)
            .await
            .unwrap();
        recorder
            .chunk("auxiliary", 3, Direction::In, b"still accepted", None)
            .await
            .unwrap();
        drop(recorder);
        worker.finish().unwrap();
        assert!(!run.output().join("auxiliary-2-in.bin").exists());
        assert_eq!(run.events().last().unwrap()["recording_stopped"], "limit");
        let bytes: u64 = std::fs::read_dir(run.output())
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        assert!(bytes <= options.max_bytes);
    }
}
