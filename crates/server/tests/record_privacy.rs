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

#[tokio::test]
async fn world_clock_is_durable_before_packets_and_independent_of_logs() {
    let run = Run::new();
    let (recorder, worker) =
        record::start_with_options(&run.0, &run.output(), Options::raw()).unwrap();
    let started = std::time::Instant::now();
    recorder.world_clock(1, started, 123456789).await.unwrap();
    assert!(!run.output().join("recording-complete.json").exists());
    let receipt_path = run.output().join("world-1-clock.json");
    let original = std::fs::read(&receipt_path).unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(receipt["format"], "afterhours-world-clock-v1");
    assert_eq!(receipt["recording_id"], "run");
    assert_eq!(receipt["pid"], std::process::id());
    assert_eq!(receipt["connection"], 1);
    assert_eq!(receipt["unix_origin_micros"], 123456789);
    assert!(receipt["recorder_offset_ns"].is_u64());
    // A repeated call cannot overwrite the original origin.
    assert!(
        recorder
            .world_clock(1, std::time::Instant::now(), 999)
            .await
            .is_err()
    );
    std::fs::write(run.output().join("server.stderr.log"), b"log rotated").unwrap();
    assert_eq!(std::fs::read(&receipt_path).unwrap(), original);
    recorder
        .chunk("world", 1, Direction::In, b"first-packet", None)
        .await
        .unwrap();
    drop(recorder);
    worker.finish().unwrap();
    assert_eq!(
        std::fs::read(run.output().join("world-1-in.bin")).unwrap(),
        b"first-packet"
    );
    let complete: serde_json::Value = serde_json::from_slice(
        &std::fs::read(run.output().join("recording-complete.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(complete["world_connections"], serde_json::json!([1]));
    assert_eq!(complete["exhausted"], false);
}

#[tokio::test]
async fn world_clock_rejects_invalid_fields_and_honors_metadata_bounds() {
    let run = Run::new();
    let (recorder, worker) = record::start_with_options(
        &run.0,
        &run.output(),
        Options {
            max_streams: 1,
            ..Options::raw()
        },
    )
    .unwrap();
    assert!(
        recorder
            .world_clock(0, std::time::Instant::now(), 1)
            .await
            .is_err()
    );
    assert!(
        recorder
            .world_clock(1, std::time::Instant::now(), 0)
            .await
            .is_err()
    );
    recorder
        .world_clock(1, std::time::Instant::now(), 1)
        .await
        .unwrap();
    assert!(
        recorder
            .world_clock(2, std::time::Instant::now(), 1)
            .await
            .is_err()
    );
    assert!(!run.output().join("world-2-clock.json").exists());
    drop(recorder);
    worker.finish().unwrap();
}

#[tokio::test]
async fn world_clock_write_failure_is_returned_without_a_success_receipt() {
    let run = Run::new();
    let (recorder, worker) = record::start(&run.0, &run.output()).unwrap();
    std::fs::create_dir(run.output().join("world-1-clock.json")).unwrap();
    assert!(
        recorder
            .world_clock(1, std::time::Instant::now(), 1)
            .await
            .is_err()
    );
    drop(recorder);
    assert!(worker.finish().is_err());
}

#[tokio::test]
async fn clock_and_completion_receipts_share_the_recording_byte_budget() {
    let run = Run::new();
    let options = Options {
        max_bytes: 700,
        ..Options::raw()
    };
    let (recorder, worker) = record::start_with_options(&run.0, &run.output(), options).unwrap();
    recorder
        .world_clock(1, std::time::Instant::now(), 1)
        .await
        .unwrap();
    recorder
        .chunk("world", 1, Direction::In, &[7; 700], None)
        .await
        .unwrap();
    drop(recorder);
    worker.finish().unwrap();
    let bytes: u64 = std::fs::read_dir(run.output())
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(bytes <= options.max_bytes);
    assert!(!run.output().join("world-1-in.bin").exists());
    let complete: serde_json::Value = serde_json::from_slice(
        &std::fs::read(run.output().join("recording-complete.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(complete["exhausted"], true);
}
