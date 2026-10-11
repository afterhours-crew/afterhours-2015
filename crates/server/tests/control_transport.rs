// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_server::{
    Failure,
    control::{Outcome, PhaseName},
    net::{End, FrameHandler, run_control},
    record,
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

fn frame(component: u16, command: u16, correlation: u32, body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: component,
                routing_b: command,
                correlation,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        nfs_server::limits::control_frame_limits(),
    )
    .unwrap()
}
#[derive(Default)]
struct Scripted {
    seen: Vec<Vec<u8>>,
    committed: usize,
    failed_writes: usize,
    unsupported: usize,
}

impl FrameHandler for Scripted {
    fn on_frame(
        &mut self,
        wire: &[u8],
        _unix_seconds: u32,
        _unix_micros: i64,
    ) -> Result<Outcome, Failure> {
        self.seen.push(wire.to_vec());
        let decoded = nfs_fire2::decode(wire, nfs_server::limits::control_frame_limits())
            .unwrap()
            .unwrap();
        if decoded.frame.fields.routing_a == 1 {
            let n = decoded.frame.fields.correlation;
            Ok(Outcome::Reply(vec![
                frame(1, 1, n, &n.to_be_bytes()),
                frame(1, 2, n, b"second"),
            ]))
        } else {
            self.unsupported += 1;
            Ok(Outcome::Unsupported)
        }
    }
    fn committed(&mut self) -> Result<(), Failure> {
        self.committed += 1;
        Ok(())
    }
    fn write_failed(&mut self) {
        self.failed_writes += 1;
    }
    fn phase(&self) -> PhaseName {
        PhaseName::Startup
    }
}

struct Root(PathBuf);
impl Root {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "nfs-server-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("artifacts")).unwrap();
        Self(path)
    }
    fn output(&self) -> PathBuf {
        self.0.join("artifacts").join("run")
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (client, server)
}

async fn read_exact_len(stream: &mut TcpStream, len: usize) -> Vec<u8> {
    let mut out = vec![0; len];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut out))
        .await
        .unwrap()
        .unwrap();
    out
}

fn events(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

struct SettingsHandler {
    session: nfs_server::user_settings::Session,
    fail_storage: Option<PathBuf>,
    committed: usize,
    aborted: bool,
}
impl FrameHandler for SettingsHandler {
    fn needs_settings_view(&self, _: &[u8]) -> bool {
        true
    }
    fn settings_view(
        &mut self,
        current: nfs_server::user_settings::Settings,
    ) -> Result<(), Failure> {
        self.session
            .refresh(current)
            .map_err(nfs_server::user_settings::failure)
    }
    fn pending_settings_change(&self) -> Option<nfs_server::user_settings::Change> {
        self.session.pending()
    }
    fn on_frame(&mut self, wire: &[u8], _: u32, _: i64) -> Result<Outcome, Failure> {
        let reply = self
            .session
            .reply(wire)
            .map_err(nfs_server::user_settings::failure)?
            .ok_or(Failure::Reply)?;
        if let Some(path) = self.fail_storage.take() {
            std::fs::rename(&path, path.with_extension("backup")).unwrap();
            std::fs::create_dir(path).unwrap();
        }
        Ok(Outcome::Reply(vec![reply]))
    }
    fn committed(&mut self) -> Result<(), Failure> {
        assert_eq!(self.session.pending(), None);
        self.committed += 1;
        Ok(())
    }
    fn write_failed(&mut self) {
        self.session.abort();
        self.aborted = true;
    }
    fn phase(&self) -> PhaseName {
        PhaseName::Startup
    }
}
fn settings_save(value: &[u8]) -> Vec<u8> {
    let body = nfs_protocol::util::UserSettingsSaveRequest {
        key: Some(b"option"),
        data: Some(value),
        user_id: Some(0),
        ..Default::default()
    }
    .encode(nfs_server::user_settings::body_limits())
    .unwrap();
    frame(9, 11, 99, &body)
}

#[tokio::test]
async fn settings_ack_is_durable_and_reads_refresh_across_connections() {
    use nfs_server::user_settings::{Change, Session, Settings, Store};
    let root = Root::new("settings-durable");
    let account = nfs_storage::AccountId::from_owned_config([9; 16]).unwrap();
    let state = root.0.join("state");
    let store = std::sync::Arc::new(
        Store::open(Settings::default(), Some((state.clone(), account))).unwrap(),
    );
    let (recorder, thread) = record::start(&root.0, &root.output()).unwrap();
    let (mut client, mut server) = pair().await;
    let server_store = store.clone();
    let task = tokio::spawn(async move {
        let mut handler = SettingsHandler {
            session: Session::new(123, Settings::default()).unwrap(),
            fail_storage: None,
            committed: 0,
            aborted: false,
        };
        let end = run_control(
            &mut server,
            &mut handler,
            1,
            &recorder,
            Duration::from_secs(5),
            None,
            Some(nfs_server::net::AccountServices {
                settings: Some(server_store),
                stats: None,
                challenges: None,
            }),
        )
        .await;
        (end, handler)
    });
    client.write_all(&settings_save(b"saved")).await.unwrap();
    let mut ack = frame(9, 11, 99, &[]);
    let decoded = nfs_fire2::decode(&ack, nfs_server::frame_limits())
        .unwrap()
        .unwrap();
    ack = nfs_fire2::encode(
        Frame {
            fields: Fields {
                category: 1,
                ..decoded.frame.fields
            },
            metadata: &[],
            body: &[],
        },
        nfs_server::frame_limits(),
    )
    .unwrap();
    assert_eq!(read_exact_len(&mut client, ack.len()).await, ack);
    let independent = Store::open(Settings::default(), Some((state, account))).unwrap();
    assert_eq!(
        independent.current().unwrap().strings,
        vec![(b"option".to_vec(), b"saved".to_vec())]
    );
    independent
        .apply(&Change::String(
            b"option".to_vec(),
            b"other connection".to_vec(),
        ))
        .unwrap();
    client.write_all(&frame(9, 12, 100, &[])).await.unwrap();
    let mut oracle = Session::new(123, independent.current().unwrap()).unwrap();
    let expected = oracle.reply(&frame(9, 12, 100, &[])).unwrap().unwrap();
    assert_eq!(read_exact_len(&mut client, expected.len()).await, expected);
    drop(client);
    let (end, handler) = task.await.unwrap();
    assert_eq!(end, End::PeerClosed);
    assert_eq!(handler.committed, 2);
    assert!(!handler.aborted);
    thread.finish().unwrap();
}

#[tokio::test]
async fn settings_persistence_failure_sends_no_success_reply() {
    use nfs_server::user_settings::{Session, Settings, Store};
    let root = Root::new("settings-fail");
    let account = nfs_storage::AccountId::from_owned_config([8; 16]).unwrap();
    let state = root.0.join("state");
    let store = std::sync::Arc::new(
        Store::open(Settings::default(), Some((state.clone(), account))).unwrap(),
    );
    let path = nfs_storage::settings::Repository::open_owned_directory(&state)
        .unwrap()
        .path(account);
    let (recorder, thread) = record::start(&root.0, &root.output()).unwrap();
    let (mut client, mut server) = pair().await;
    let task = tokio::spawn(async move {
        let mut handler = SettingsHandler {
            session: Session::new(123, Settings::default()).unwrap(),
            fail_storage: Some(path),
            committed: 0,
            aborted: false,
        };
        let end = run_control(
            &mut server,
            &mut handler,
            1,
            &recorder,
            Duration::from_secs(5),
            None,
            Some(nfs_server::net::AccountServices {
                settings: Some(store),
                stats: None,
                challenges: None,
            }),
        )
        .await;
        (end, handler)
    });
    client
        .write_all(&settings_save(b"must not ack"))
        .await
        .unwrap();
    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert!(received.is_empty());
    let (end, handler) = task.await.unwrap();
    assert_eq!(end, End::Internal);
    assert_eq!(handler.committed, 0);
    assert!(handler.aborted);
    thread.finish().unwrap();
    assert!(events(&root.output()).iter().all(|e| e["dir"] != "out"));
}

#[tokio::test]
async fn concatenated_requests_get_ordered_replies_and_exact_recording() {
    let root = Root::new("concat");
    let (recorder, thread) = record::start(&root.0, &root.output()).unwrap();
    let (mut client, mut server) = pair().await;
    let requests = [frame(1, 5, 1, b"a"), frame(1, 5, 2, b"bb")];
    let task = tokio::spawn(async move {
        let mut handler = Scripted::default();
        let end = run_control(
            &mut server,
            &mut handler,
            7,
            &recorder,
            Duration::from_secs(5),
            None,
            None,
        )
        .await;
        (end, handler)
    });
    client.write_all(&requests.concat()).await.unwrap();
    let expected = [
        frame(1, 1, 1, &1u32.to_be_bytes()),
        frame(1, 2, 1, b"second"),
        frame(1, 1, 2, &2u32.to_be_bytes()),
        frame(1, 2, 2, b"second"),
    ]
    .concat();
    assert_eq!(read_exact_len(&mut client, expected.len()).await, expected);
    drop(client);
    let (end, handler) = task.await.unwrap();
    assert_eq!(end, End::PeerClosed);
    assert_eq!(handler.seen, requests.to_vec());
    assert_eq!(handler.committed, 2);
    thread.finish().unwrap();
    let dir = root.output();
    assert_eq!(
        std::fs::read(dir.join("blaze-7-in.bin")).unwrap(),
        requests.concat()
    );
    assert_eq!(
        std::fs::read(dir.join("blaze-7-out.bin")).unwrap(),
        expected
    );
    let ev = events(&dir);
    assert_eq!(ev.len(), 6);
    assert_eq!(ev[0]["dir"], "in");
    assert_eq!(ev[0]["component"], 1);
    assert_eq!(ev[0]["correlation"], 1);
    assert_eq!(ev[3]["dir"], "in");
    assert_eq!(ev[3]["offset"], requests[0].len());
    assert!(ev.iter().all(|e| e.get("note").is_none()));
}

#[tokio::test]
async fn byte_by_byte_frame_is_handled_once_complete() {
    let root = Root::new("bytewise");
    let (recorder, thread) = record::start(&root.0, &root.output()).unwrap();
    let (mut client, mut server) = pair().await;
    let request = frame(1, 9, 3, b"slow body");
    let task = tokio::spawn(async move {
        let mut handler = Scripted::default();
        run_control(
            &mut server,
            &mut handler,
            1,
            &recorder,
            Duration::from_secs(5),
            None,
            None,
        )
        .await;
        handler
    });
    client.set_nodelay(true).unwrap();
    for byte in &request {
        client.write_all(&[*byte]).await.unwrap();
        client.flush().await.unwrap();
    }
    let expected = [
        frame(1, 1, 3, &3u32.to_be_bytes()),
        frame(1, 2, 3, b"second"),
    ]
    .concat();
    assert_eq!(read_exact_len(&mut client, expected.len()).await, expected);
    drop(client);
    let handler = task.await.unwrap();
    assert_eq!(handler.seen, vec![request]);
    thread.finish().unwrap();
}

#[tokio::test]
async fn unsupported_request_gets_no_reply_and_later_requests_are_still_served() {
    let root = Root::new("unsupported");
    let (recorder, thread) = record::start(&root.0, &root.output()).unwrap();
    let (mut client, mut server) = pair().await;
    let requests = [
        frame(1, 5, 1, b""),
        frame(9, 9, 2, b"x"),
        frame(1, 5, 3, b""),
    ];
    let task = tokio::spawn(async move {
        let mut handler = Scripted::default();
        let end = run_control(
            &mut server,
            &mut handler,
            2,
            &recorder,
            Duration::from_secs(5),
            None,
            None,
        )
        .await;
        (end, handler)
    });
    client.write_all(&requests.concat()).await.unwrap();
    let expected = [
        frame(1, 1, 1, &1u32.to_be_bytes()),
        frame(1, 2, 1, b"second"),
        frame(1, 1, 3, &3u32.to_be_bytes()),
        frame(1, 2, 3, b"second"),
    ]
    .concat();
    assert_eq!(read_exact_len(&mut client, expected.len()).await, expected);
    client.shutdown().await.unwrap();
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await.unwrap();
    assert!(
        rest.is_empty(),
        "nothing is sent for the unsupported request"
    );
    let (end, handler) = task.await.unwrap();
    assert_eq!(end, End::PeerClosed);
    assert_eq!(handler.seen.len(), 3);
    assert_eq!(handler.unsupported, 1);
    assert_eq!(handler.committed, 2);
    thread.finish().unwrap();
    let ev = events(&root.output());
    let dirs: Vec<_> = ev.iter().map(|e| e["dir"].as_str().unwrap()).collect();
    assert_eq!(dirs, ["in", "out", "out", "in", "in", "out", "out"]);
    assert!(ev[0].get("note").is_none());
    assert_eq!(ev[3]["note"], "unsupported");
    assert_eq!(ev[3]["component"], 9);
    assert!(ev[4].get("note").is_none());
}

#[tokio::test]
async fn oversized_header_closes_the_connection_without_reply() {
    let root = Root::new("oversized");
    let (recorder, thread) = record::start(&root.0, &root.output()).unwrap();
    let (mut client, mut server) = pair().await;
    let mut header = frame(1, 5, 1, b"");
    header[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    let task = tokio::spawn(async move {
        let mut handler = Scripted::default();
        let end = run_control(
            &mut server,
            &mut handler,
            3,
            &recorder,
            Duration::from_secs(5),
            None,
            None,
        )
        .await;
        (end, handler)
    });
    client.write_all(&header).await.unwrap();
    let (end, handler) = task.await.unwrap();
    assert_eq!(end, End::InvalidFrame);
    assert!(handler.seen.is_empty());
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());
    thread.finish().unwrap();
    assert_eq!(events(&root.output())[0]["note"], "invalid");
}

#[tokio::test]
async fn idle_connection_ends_and_keeps_partial_input() {
    let root = Root::new("idle");
    let (recorder, thread) = record::start(&root.0, &root.output()).unwrap();
    let (mut client, mut server) = pair().await;
    let partial = &frame(1, 5, 1, b"abc")[..10];
    let task = tokio::spawn(async move {
        let mut handler = Scripted::default();
        run_control(
            &mut server,
            &mut handler,
            4,
            &recorder,
            Duration::from_millis(200),
            None,
            None,
        )
        .await
    });
    client.write_all(partial).await.unwrap();
    assert_eq!(task.await.unwrap(), End::Idle);
    thread.finish().unwrap();
    assert_eq!(
        std::fs::read(root.output().join("blaze-4-in.bin")).unwrap(),
        partial
    );
    assert_eq!(events(&root.output())[0]["note"], "partial");
}

#[test]
fn recorder_refuses_output_outside_artifacts_or_existing_directories() {
    let root = Root::new("paths");
    assert!(record::start(&root.0, &root.0.join("outside")).is_err());
    std::fs::create_dir(root.output()).unwrap();
    assert!(record::start(&root.0, &root.output()).is_err());
}
