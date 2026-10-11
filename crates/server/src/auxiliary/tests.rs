// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn get(authority: &str, user: i64, owner: i64, pictures: bool) -> Vec<u8> {
    let (context, category, kind, name) = if pictures {
        ("nfs-rivals-common", "Pictures", 1, owner.to_string())
    } else {
        ("nfs-2016-pc", "Liveries", 2, "LiveryHeader2_0X1234".into())
    };
    format!("GET /1.0/contexts/{context}/categories/{category}/records/{name}?ownerId={owner}&ownerType={kind}&subrecord= HTTP/1.1\r\nHost: {authority}\r\nX-USER-ID: {user}\r\nX-USER-TYPE: NUCLEUS_PERSONA\r\nX-TOKEN-TYPE: NUCLEUS_ACCESS_TOKEN\r\nAuthorization: constructed-test-token\r\n\r\n").into_bytes()
}
fn token(authority: &str) -> Vec<u8> {
    let body = "grant_type=authorization_code&code=test&redirect_uri=http%3A%2F%2Flocalhost&client_id=local&client_secret=test";
    format!("POST /connect/token HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",body.len()).into_bytes()
}
fn response(bytes: &[u8], records: &EmptyLocalRecords) -> Result<(Stop, Vec<u8>), Stop> {
    answer(
        inspect(bytes)?.ok_or(Stop::PeerClosed)?,
        "127.0.0.1:1234",
        records,
        &[0xab; 32],
    )
}
fn replace(bytes: &[u8], from: &str, to: &str) -> Vec<u8> {
    std::str::from_utf8(bytes)
        .unwrap()
        .replace(from, to)
        .into_bytes()
}

#[test]
fn missing_records_are_native_errors_for_both_owned_address_types() {
    let records = EmptyLocalRecords::new(7, 9).unwrap();
    for bytes in [
        get("127.0.0.1:1234", 7, 7, false),
        get("127.0.0.1:1234", 7, 9, true),
    ] {
        let expected=b"HTTP/1.1 404 Not Found\r\nX-BLAZE-ERRORCODE: 1179679\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        for _ in 0..3 {
            let (stop, wire) = response(&bytes, &records).unwrap();
            assert_eq!(stop, Stop::MissingRecord);
            assert_eq!(wire, expected);
        }
        for n in 0..bytes.len() {
            assert!(inspect(&bytes[..n]).unwrap().is_none(), "prefix {n}");
        }
    }
}

#[test]
fn wire_ids_do_not_select_another_catalog_and_authority_is_exact() {
    let alice = EmptyLocalRecords::new(7, 9).unwrap();
    let bob = EmptyLocalRecords::new(11, 13).unwrap();
    for (request, a, b) in [
        (get("127.0.0.1:1234", 7, 7, false), &alice, &bob),
        (get("127.0.0.1:1234", 11, 13, true), &bob, &alice),
    ] {
        assert_eq!(response(&request, a).unwrap().0, Stop::MissingRecord);
        assert_eq!(response(&request, b).unwrap().0, Stop::Forbidden);
    }
    assert_eq!(
        response(&get("127.0.0.1:1234", 7, 11, false), &alice)
            .unwrap()
            .0,
        Stop::Forbidden
    );
    assert_eq!(
        response(&get("127.0.0.1:9999", 7, 7, false), &alice),
        Err(Stop::UnsupportedRequest)
    );
    assert!(EmptyLocalRecords::new(0, 9).is_none());
    assert!(EmptyLocalRecords::new(9, 9).is_none());
}

#[test]
fn token_contract_is_preserved_and_each_session_has_injected_entropy() {
    let request = token("127.0.0.1:1234");

    let records = EmptyLocalRecords::new(7, 9).unwrap();
    for n in 0..request.len() {
        assert!(inspect(&request[..n]).unwrap().is_none());
    }
    let (stop, wire) = response(&request, &records).unwrap();
    assert_eq!(stop, Stop::Token);
    let (_, body) = std::str::from_utf8(&wire)
        .unwrap()
        .split_once("\r\n\r\n")
        .unwrap();
    assert_eq!(body.len(), 83);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap()["access_token"],
        "ab".repeat(32)
    );
    let other = answer(
        inspect(&request).unwrap().unwrap(),
        "127.0.0.1:1234",
        &records,
        &[0xcd; 32],
    )
    .unwrap();
    assert_ne!(wire, other.1);
    assert_eq!(
        answer(
            inspect(&request).unwrap().unwrap(),
            "127.0.0.1:1234",
            &records,
            &[0; 32]
        ),
        Err(Stop::InvalidHttp)
    );
    let invalid = replace(&request, "authorization_code", "authorization_codf");
    assert_eq!(inspect(&invalid).err(), Some(Stop::UnsupportedRequest));
}

#[test]
fn ambiguous_headers_lengths_and_pipelining_are_rejected() {
    let request = get("127.0.0.1:1234", 7, 7, false);
    for (from, to, stop) in [
        ("Host: ", "Host: other\r\nHost: ", Stop::InvalidHttp),
        (
            "X-USER-ID: 7",
            "X-USER-ID: 7\r\nx-user-id: 11",
            Stop::InvalidHttp,
        ),
        (
            "\r\n\r\n",
            "\r\nContent-Length: 0\r\ncontent-length: 0\r\n\r\n",
            Stop::InvalidHttp,
        ),
        (
            "\r\n\r\n",
            "\r\nContent-Length: 99999999999999999999999999\r\n\r\n",
            Stop::BodyLimit,
        ),
        (
            "\r\n\r\n",
            "\r\nContent-Length: +0\r\n\r\n",
            Stop::InvalidHttp,
        ),
        (
            "\r\n\r\n",
            "\r\nTransfer-Encoding: chunked\r\n\r\n",
            Stop::UnsupportedRequest,
        ),
        ("\r\nHost:", "\nHost:", Stop::InvalidHttp),
    ] {
        assert_eq!(
            inspect(&replace(&request, from, to)).err(),
            Some(stop),
            "{to}"
        );
    }
    let mut joined = request.clone();
    joined.extend(&request);
    assert_eq!(inspect(&joined).err(), Some(Stop::ExtraBytes));
    assert_eq!(
        inspect(&vec![b'A'; MAX_HEADER_BYTES]).err(),
        Some(Stop::HeaderLimit)
    );
    assert_eq!(
        inspect(&vec![b'A'; MAX_REQUEST_BYTES + 1]).err(),
        Some(Stop::BodyLimit)
    );
    let headers = (0..33).map(|i| format!("X-{i}: y\r\n")).collect::<String>();
    assert_eq!(
        inspect(format!("GET / HTTP/1.1\r\n{headers}\r\n").as_bytes()).err(),
        Some(Stop::HeaderCount)
    );
}

#[test]
fn unknown_operations_and_address_ambiguity_do_not_get_a_success() {
    let request = get("127.0.0.1:1234", 7, 7, false);
    for (from, to) in [
        ("GET ", "PUT "),
        ("Liveries", "Other"),
        ("ownerType=2", "ownerType=1"),
        ("ownerId=7", "ownerId=7&ownerId=9"),
        ("ownerId=7", "ownerId=-7"),
        ("ownerId=7", "ownerId=9223372036854775808"),
        ("subrecord=", "subrecord=other"),
        ("subrecord=", "unknown="),
        ("LiveryHeader2_0X1234", "../secret"),
        ("LiveryHeader2_0X1234", "encoded%2Fname"),
        ("NUCLEUS_PERSONA", "NUCLEUS_ACCOUNT"),
        ("Authorization: constructed-test-token\r\n", ""),
        ("X-USER-ID: 7\r\n", ""),
    ] {
        assert!(inspect(&replace(&request, from, to)).is_err(), "{to}");
    }
    assert!(inspect(&replace(&request, "LiveryHeader2_0X1234", &"A".repeat(129))).is_err());
    assert_eq!(
        response(
            &replace(
                &token("127.0.0.1:1234"),
                "grant_type=authorization_code",
                "grant_type=refresh_token"
            ),
            &EmptyLocalRecords::new(7, 9).unwrap()
        ),
        Err(Stop::PeerClosed)
    );
}

#[tokio::test]
async fn loopback_fragmentation_closure_and_deadline_are_bounded() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut client = TcpStream::connect(address).await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let task = tokio::spawn(async move {
        serve(
            stream,
            &EmptyLocalRecords::new(7, 9).unwrap(),
            &[1; 32],
            Instant::now() + Duration::from_secs(2),
        )
        .await
    });
    let request = get(&address.to_string(), 7, 9, true);
    for chunk in request.chunks(13) {
        client.write_all(chunk).await.unwrap();
    }
    let mut response = Vec::new();
    timeout_at(
        Instant::now() + Duration::from_secs(2),
        client.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    let observed = task.await.unwrap();
    assert_eq!(observed.stop, Stop::MissingRecord);
    assert_eq!(observed.request, request);
    assert_eq!(observed.response, response);
    assert_eq!(observed.response_bytes, response.len());

    let _idle = TcpStream::connect(address).await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let observed = serve(
        stream,
        &EmptyLocalRecords::new(7, 9).unwrap(),
        &[1; 32],
        Instant::now() + Duration::from_millis(25),
    )
    .await;
    assert_eq!(observed.stop, Stop::Deadline);
    assert_eq!(observed.response_bytes, 0);

    let mut closed = TcpStream::connect(address).await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    closed.write_all(b"GET ").await.unwrap();
    closed.shutdown().await.unwrap();
    let observed = serve(
        stream,
        &EmptyLocalRecords::new(7, 9).unwrap(),
        &[1; 32],
        Instant::now() + Duration::from_secs(1),
    )
    .await;
    assert_eq!(observed.stop, Stop::PeerClosed);
    assert_eq!(observed.request, b"GET ");
}

#[tokio::test]
async fn concurrent_endpoints_keep_owners_and_cancellation_closes_partial_requests() {
    let mut tasks = JoinSet::new();
    let mut clients = Vec::new();
    for (persona, account) in [(7, 9), (11, 13)] {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        tasks.spawn(async move {
            serve(
                stream,
                &EmptyLocalRecords::new(persona, account).unwrap(),
                &[1; 32],
                Instant::now() + Duration::from_secs(2),
            )
            .await
        });
        clients.push((client, address));
    }
    for (client, address) in &mut clients {
        client
            .write_all(&get(&address.to_string(), 7, 7, false))
            .await
            .unwrap();
    }
    let mut stops = Vec::new();
    while let Some(result) = tasks.join_next().await {
        stops.push(result.unwrap().stop);
    }
    assert!(stops.contains(&Stop::MissingRecord) && stops.contains(&Stop::Forbidden));

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut client = TcpStream::connect(address).await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let task = tokio::spawn(async move {
        serve(
            stream,
            &EmptyLocalRecords::new(7, 9).unwrap(),
            &[1; 32],
            Instant::now() + Duration::from_secs(60),
        )
        .await
    });
    client.write_all(b"GET ").await.unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    let mut tail = Vec::new();
    let closed = timeout_at(
        Instant::now() + Duration::from_secs(1),
        client.read_to_end(&mut tail),
    )
    .await
    .unwrap();
    assert!(closed.is_err() || tail.is_empty());
}
