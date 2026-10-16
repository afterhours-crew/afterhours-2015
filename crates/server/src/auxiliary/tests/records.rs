// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_storage::{InventoryRepository, SqliteRepository, records::SqliteRecords};
use std::{path::PathBuf, sync::atomic::AtomicU64};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nfs-wrap-http-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (Temp, LocalRecords, SqliteRecords, AccountId) {
    let temp = Temp::new();
    let account = AccountId::from_owned_config([1; 16]).unwrap();
    let repo = SqliteRepository::open_owned_directory(&temp.0).unwrap();
    repo.open(account).unwrap();
    let store = SqliteRecords(repo);
    let records = LocalRecords::new(7, 9)
        .unwrap()
        .with_storage(Arc::new(store.clone()), account);
    (temp, records, store, account)
}
fn put(authority: &str, user: i64, owner: i64, body: &[u8]) -> Vec<u8> {
    let mut bytes=format!("PUT /1.0/contexts/nfs-2016-pc/categories/Liveries/records/LiveryHeader2_0X1234?ownerId={owner}&ownerType=2&subrecordupdate=false HTTP/1.1\r\nHost: {authority}\r\nContent-Length: {}\r\nContent-Type: image/jpeg\r\nX-USER-ID: {user}\r\nX-USER-TYPE: NUCLEUS_PERSONA\r\nX-TOKEN-TYPE: NUCLEUS_ACCESS_TOKEN\r\nAuthorization: constructed-token\r\n\r\n",body.len()).into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
fn at(bytes: &[u8], records: &LocalRecords, time: u64) -> Result<(Stop, Vec<u8>), Stop> {
    response_at(bytes, "127.0.0.1:1234", records, &[3; 32], UnixMicros(time))?
        .ok_or(Stop::PeerClosed)
}
fn body(wire: &[u8]) -> &[u8] {
    let end = wire.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    &wire[end..]
}

#[test]
fn upload_commits_exact_payload_before_ack_and_get_survives_reopen() {
    let (temp, records, store, account) = setup();
    let payload = b"\0\x01opaque\xffrecord\0";
    let request = put("127.0.0.1:1234", 7, 7, payload);
    for n in 0..request.len() {
        assert!(
            response_at(
                &request[..n],
                "127.0.0.1:1234",
                &records,
                &[3; 32],
                UnixMicros(5)
            )
            .unwrap()
            .is_none()
        );
    }
    assert!(
        store
            .get(account, "LiveryHeader2_0X1234")
            .unwrap()
            .is_none()
    );
    let first = at(&request, &records, 5).unwrap();
    assert_eq!(first.0, Stop::Stored);
    assert!(first.1.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(body(&first.1).is_empty());
    assert_eq!(
        store
            .get(account, "LiveryHeader2_0X1234")
            .unwrap()
            .unwrap()
            .bytes,
        payload
    );
    assert_eq!(at(&request, &records, 20).unwrap(), first);
    let reopened = SqliteRecords(SqliteRepository::open_owned_directory(&temp.0).unwrap());
    let records = LocalRecords::new(7, 9)
        .unwrap()
        .with_storage(Arc::new(reopened), account);
    let download = get("127.0.0.1:1234", 7, 7, false);
    let read = at(&download, &records, 21).unwrap();
    assert_eq!(read.0, Stop::Record);
    assert_eq!(body(&read.1), payload);
    let head = replace(&download, "GET ", "HEAD ");
    let metadata = at(&head, &records, 21).unwrap();
    assert!(body(&metadata.1).is_empty());
    let delete = replace(&replace(&download, "GET ", "DELETE "), "&subrecord=", "");
    assert_eq!(at(&delete, &records, 22).unwrap().0, Stop::Deleted);
    assert_eq!(at(&delete, &records, 23).unwrap().0, Stop::Deleted);
    assert_eq!(at(&download, &records, 24).unwrap().0, Stop::MissingRecord);
}

#[test]
fn mutations_require_owner_supported_address_complete_frame_and_storage() {
    let (_temp, records, store, account) = setup();
    for (user, owner) in [(7, 11), (11, 7), (11, 11)] {
        assert_eq!(
            at(&put("127.0.0.1:1234", user, owner, b"bad"), &records, 1)
                .unwrap()
                .0,
            Stop::Forbidden
        );
    }
    let request = put("127.0.0.1:1234", 7, 7, b"valid");
    for (from, to) in [
        ("subrecordupdate=false", "subrecordupdate=true"),
        ("subrecordupdate=false", "subrecordupdate=false&subrecord="),
        ("subrecordupdate=false", "subrecord="),
        ("image/jpeg", "text/html"),
        ("Liveries", "Pictures"),
        ("127.0.0.1:1234", "127.0.0.1:9999"),
        ("PUT ", "POST "),
    ] {
        assert!(
            at(&replace(&request, from, to), &records, 1).is_err(),
            "{to}"
        );
    }
    let mut pipelined = request.clone();
    pipelined.extend_from_slice(&request);
    assert_eq!(at(&pipelined, &records, 1), Err(Stop::ExtraBytes));
    assert_eq!(
        at(&request, &LocalRecords::new(7, 9).unwrap(), 1),
        Err(Stop::UnsupportedRequest)
    );
    assert!(
        store
            .get(account, "LiveryHeader2_0X1234")
            .unwrap()
            .is_none()
    );
    let old = at(&request, &records, 1).unwrap();
    assert_eq!(old.0, Stop::Stored);
    assert_eq!(
        at(
            &put("127.0.0.1:1234", 7, 7, &vec![0; MAX_BODY_BYTES + 1]),
            &records,
            2
        ),
        Err(Stop::BodyLimit)
    );
    assert_eq!(
        store
            .get(account, "LiveryHeader2_0X1234")
            .unwrap()
            .unwrap()
            .bytes,
        b"valid"
    );
}

struct Failing;
impl Repository for Failing {
    fn get(&self, _: AccountId, _: &str) -> Result<Option<Record>, nfs_storage::Error> {
        Err(nfs_storage::Error::Storage)
    }
    fn put(
        &self,
        _: AccountId,
        _: &str,
        _: &str,
        _: &[u8],
        _: UnixMicros,
    ) -> Result<Record, nfs_storage::Error> {
        Err(nfs_storage::Error::Storage)
    }
    fn delete(&self, _: AccountId, _: &str) -> Result<(), nfs_storage::Error> {
        Err(nfs_storage::Error::Storage)
    }
}
#[test]
fn storage_failure_never_produces_http_success() {
    let records = LocalRecords::new(7, 9).unwrap().with_storage(
        Arc::new(Failing),
        AccountId::from_owned_config([1; 16]).unwrap(),
    );
    for request in [
        put("127.0.0.1:1234", 7, 7, b"test"),
        get("127.0.0.1:1234", 7, 7, false),
    ] {
        assert_eq!(at(&request, &records, 1), Err(Stop::Storage));
    }
}

#[tokio::test]
async fn fragmented_upload_and_disconnect_preserve_transaction_boundary() {
    let (_temp, records, store, account) = setup();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let payload = vec![0xff; 5000];
    for complete in [false, true] {
        let mut client = TcpStream::connect(address).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let records = records.clone();
        let task = tokio::spawn(async move {
            serve(
                stream,
                &records,
                &[1; 32],
                Instant::now() + Duration::from_secs(3),
            )
            .await
        });
        let request = put(&address.to_string(), 7, 7, &payload);
        let sent = if complete {
            &request[..]
        } else {
            &request[..request.len() - 1]
        };
        for chunk in sent.chunks(71) {
            client.write_all(chunk).await.unwrap();
        }
        client.shutdown().await.unwrap();
        let mut reply = Vec::new();
        timeout_at(
            Instant::now() + Duration::from_secs(3),
            client.read_to_end(&mut reply),
        )
        .await
        .unwrap()
        .unwrap();
        let observed = task.await.unwrap();
        assert_eq!(
            observed.stop,
            if complete {
                Stop::Stored
            } else {
                Stop::PeerClosed
            }
        );
        if complete {
            assert!(reply.starts_with(b"HTTP/1.1 200 OK\r\n"));
            assert_eq!(
                store
                    .get(account, "LiveryHeader2_0X1234")
                    .unwrap()
                    .unwrap()
                    .bytes,
                payload
            );
        } else {
            assert!(reply.is_empty());
            assert!(
                store
                    .get(account, "LiveryHeader2_0X1234")
                    .unwrap()
                    .is_none()
            );
        }
    }
}
