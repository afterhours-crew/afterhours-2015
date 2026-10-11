// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_server::world_handshake::Binding;
use nfs_server::{
    net::{WorldServe, world_host},
    record,
};
use nfs_world::{
    crypto::session_key,
    transport::{self, Codec, Header, Part},
};
use std::{path::PathBuf, time::Duration};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, watch},
    time::timeout,
};

const WORLD: &str = "11111111-2222-3333-4444-555555555555";
const LOCAL: &str = "66666666-7777-8888-9999-aaaaaaaaaaaa";
const CLIENT: u32 = 0x0102_0304;
const HOST: u32 = 0x0a0b_0c0d;
const CLIENT_INDEX: u16 = 9;
const TOKEN: u32 = 0x5151_0001;

fn template() -> [u8; 64] {
    std::array::from_fn(|i| (i * 5) as u8)
}

fn codec() -> Codec {
    Codec::new(session_key(WORLD, LOCAL).unwrap(), template()).unwrap()
}

struct Root(PathBuf);
impl Root {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "nfs-server-test-world-{name}-{}-{}",
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
struct Client {
    codec: Codec,
    tx: u16,
}

impl Client {
    fn send(&mut self, header: Header, bytes: Vec<u8>) -> Vec<u8> {
        let wire = self
            .codec
            .encode(header, &[Part { channel: 0, bytes }])
            .unwrap();
        let header_len = match header {
            Header::Initial { .. } => 11,
            Header::Ordinary { .. } => 4,
        };
        self.tx = transport::advance(self.tx, wire.len() - header_len).unwrap();
        wire
    }
    fn words(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }
    fn request(&mut self) -> Vec<u8> {
        let header = Header::Initial {
            sender: CLIENT,
            index: CLIENT_INDEX,
            peer_known: false,
            cursor: self.tx,
        };
        self.send(header, Self::words(&[1, TOKEN, CLIENT]))
    }
    fn ordinary(&self, host_index: u16) -> Header {
        Header::Ordinary {
            index: host_index,
            cursor: self.tx,
        }
    }
    fn confirm(&mut self, host_index: u16) -> Vec<u8> {
        let header = self.ordinary(host_index);
        self.send(header, Self::words(&[2, TOKEN]))
    }
    fn close(&mut self, host_index: u16) -> Vec<u8> {
        let header = self.ordinary(host_index);
        self.send(header, Self::words(&[3, TOKEN]))
    }
    fn sync(&mut self, host_index: u16, now: u32) -> Vec<u8> {
        let mut inner = Self::words(&[256, 256, now, now + 3]);
        inner.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 16, 0x40]);
        let header = self.ordinary(host_index);
        self.send(header, inner)
    }
}

async fn receive(socket: &UdpSocket) -> Vec<u8> {
    let mut buffer = vec![0; 2048];
    let n = timeout(Duration::from_secs(5), socket.recv(&mut buffer))
        .await
        .expect("a datagram within 5 s")
        .unwrap();
    buffer.truncate(n);
    buffer
}

#[tokio::test]
async fn world_host_handshakes_syncs_publishes_the_proof_and_closes() {
    let root = Root::new("sync");
    let (recorder, thread) = record::start(&root.0, &root.output()).unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let (start, start_rx) = mpsc::channel(1);
    let (proof_tx, mut proof) = watch::channel(None);
    let task = tokio::spawn(world_host(
        socket,
        start_rx,
        proof_tx,
        WorldServe {
            mac_template: Some(template()),
            content: None,
            persona: 0x1000_0007,
            inventory: None,
            progression: None,
            vehicles: None,
            sequences: None,
            garage_logic: None,
        },
        3,
        recorder.clone(),
    ));
    let binding = Binding::from_current(
        WORLD.as_bytes(),
        LOCAL.as_bytes(),
        u64::from(CLIENT),
        u64::from(HOST),
    )
    .unwrap();
    start.send(binding).await.unwrap();

    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(address).await.unwrap();
    let mut game = Client {
        codec: codec(),
        tx: 0,
    };

    client.send(&game.request()).await.unwrap();
    let reply = receive(&client).await;
    assert_eq!(reply.len(), 37, "handshake reply");
    let decoded = game.codec.decode(&reply).unwrap();
    let Header::Initial {
        sender,
        index: host_index,
        peer_known,
        ..
    } = decoded.header
    else {
        panic!("initial header expected")
    };
    assert_eq!((sender, peer_known), (HOST, true));
    assert_eq!(decoded.parts[0].bytes, Client::words(&[5, TOKEN, HOST]));

    client.send(&game.confirm(host_index)).await.unwrap();
    assert_eq!(receive(&client).await.len(), 43, "initial host sync");
    assert!(
        proof.borrow().is_none(),
        "no proof before the client's sync"
    );

    client.send(&game.sync(host_index, 5)).await.unwrap();
    assert_eq!(receive(&client).await.len(), 26, "empty acknowledgement");
    timeout(Duration::from_secs(5), proof.changed())
        .await
        .expect("proof within 5 s")
        .unwrap();
    assert_eq!(
        *proof.borrow(),
        Some(None),
        "synced; a bare binding carries no readiness"
    );

    client.send(&game.close(host_index)).await.unwrap();
    timeout(Duration::from_secs(5), task)
        .await
        .expect("the host closes after the client's close")
        .unwrap();
    drop(recorder);
    thread.finish().unwrap();
    let events = std::fs::read_to_string(root.output().join("events.jsonl")).unwrap();
    let world: Vec<serde_json::Value> = events
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|e| e["service"] == "world")
        .collect();
    assert_eq!(world.len(), 7, "four datagrams in, three out");
    assert!(world.iter().all(|e| e["conn"] == 3));
}

#[tokio::test]
async fn world_host_without_identities_or_template_does_not_serve() {
    let root = Root::new("unserved");
    let (recorder, thread) = record::start(&root.0, &root.output()).unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (start, start_rx) = mpsc::channel(1);
    let (proof_tx, proof) = watch::channel(None);
    let task = tokio::spawn(world_host(
        socket,
        start_rx,
        proof_tx,
        WorldServe {
            mac_template: None,
            content: None,
            persona: 0x1000_0007,
            inventory: None,
            progression: None,
            vehicles: None,
            sequences: None,
            garage_logic: None,
        },
        4,
        recorder.clone(),
    ));
    let binding = Binding::from_current(
        WORLD.as_bytes(),
        LOCAL.as_bytes(),
        u64::from(CLIENT),
        u64::from(HOST),
    )
    .unwrap();
    start.send(binding).await.unwrap();
    timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(proof.borrow().is_none());
    drop(recorder);
    thread.finish().unwrap();
}
