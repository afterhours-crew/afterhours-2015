// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_server_support::discovery::*;
use std::io::{Cursor, Read};

fn body() -> String {
    let mut out = String::from("<serverinstancerequest>");
    for (name, value) in [
        ("blazesdkversion", "test-sdk"),
        ("blazesdkbuilddate", "test-date"),
        ("clientname", "test-client"),
        ("clienttype", "CLIENT_TYPE_GAMEPLAY_USER"),
        ("clientplatform", "pc"),
        ("clientskuid", "test-sku"),
        ("clientversion", "test-version"),
        ("dirtysdkversion", "test-dirty"),
        ("environment", "local"),
        ("clientlocale", "0"),
        ("name", "local-service"),
        ("platform", "Windows"),
        ("connectionprofile", "local"),
        ("istrial", "0"),
    ] {
        out.push_str(&format!("<{name}>{value}</{name}>"));
    }
    out.push_str("</serverinstancerequest>");
    out
}
fn request(body: &str) -> Vec<u8> {
    format!("POST {ROUTE} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n\r\n{body}",body.len()).into_bytes()
}
struct ShortReads(Cursor<Vec<u8>>);
impl Read for ShortReads {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = out.len().min(3);
        self.0.read(&mut out[..n])
    }
}
#[test]
fn bounded_short_reads_and_one_message_per_connection() {
    let wire = request(&body());
    assert!(eligible(&receive(&mut ShortReads(Cursor::new(
        wire.clone()
    )))));
    for end in 0..wire.len() {
        let capture = receive(&mut Cursor::new(&wire[..end]));
        assert!(!eligible(&capture));
        assert_eq!(capture.result, Err(Failure::Eof));
    }
    let mut concatenated = wire.clone();
    concatenated.extend(&wire);
    let mut reader = Cursor::new(concatenated);
    let capture = receive(&mut reader);
    // This one-request endpoint rejects pipelined bytes already received with
    // the request instead of silently accepting an ambiguous connection.
    assert!(!eligible(&capture));
    assert_eq!(reader.position(), (wire.len() * 2) as u64);
}
#[test]
fn strict_headers_reject_ambiguous_framing_and_resource_exhaustion() {
    for (headers, expected) in [
        (
            "Host: a\r\nContent-Length: 1\r\nContent-Length: 1",
            Failure::ContentLength,
        ),
        ("Host: a\r\nContent-Length: 65537", Failure::BodyLimit),
        ("Host: a\r\nContent-Length: -1", Failure::ContentLength),
        (
            "Host: a\r\nContent-Length: 0\r\nTransfer-Encoding: chunked",
            Failure::UnsupportedFraming,
        ),
        ("Host: a\r\nHost: b\r\nContent-Length: 0", Failure::Host),
        ("Content-Length: 0", Failure::Host),
    ] {
        let wire = format!("POST {ROUTE} HTTP/1.1\r\n{headers}\r\n\r\n");
        assert_eq!(parse_head(wire.as_bytes()), Err(expected));
    }
    assert_eq!(
        parse_head(&vec![b'x'; MAX_HEADER]),
        Err(Failure::HeaderLimit)
    );
    assert_eq!(
        parse_head(b"POST / HTTP/1.1\nHost: a\r\nContent-Length: 0\r\n\r\n"),
        Err(Failure::InvalidHttp)
    );
}
#[test]
fn eligibility_checks_platform_unknown_fields_and_complete_body() {
    let source = body();
    for changed in [
        source.replace("Windows", "Other"),
        source.replace("<istrial>0", "<istrial>1"),
        source.replace(
            "</serverinstancerequest>",
            "<unknown>1</unknown></serverinstancerequest>",
        ),
        source.replace("<name>local-service</name>", ""),
    ] {
        assert!(!eligible(&receive(&mut Cursor::new(request(&changed)))));
    }
    let mut wire = request(&source);
    wire[0] = b'G';
    assert!(!eligible(&receive(&mut Cursor::new(wire))));
}
#[test]
fn generated_response_uses_only_owned_loopback_and_bounded_ca() {
    for address in [
        "0.0.0.0:1",
        "192.0.2.1:1",
        "127.0.0.2:1",
        "[::1]:1",
        "127.0.0.1:0",
    ] {
        assert_eq!(
            local_response(address.parse().unwrap()),
            Err(Failure::NonLoopback)
        );
    }
    let addr = "127.0.0.1:12345".parse().unwrap();
    assert_eq!(
        local_response_with_ca(addr, Some(&[])),
        Err(Failure::BodyLimit)
    );
    assert_eq!(
        local_response_with_ca(addr, Some(&vec![0; 4097])),
        Err(Failure::BodyLimit)
    );
    let wire = local_response_with_ca(addr, Some(b"synthetic-ca")).unwrap();
    let mut headers = [httparse::EMPTY_HEADER; 8];
    let mut response = httparse::Response::new(&mut headers);
    let httparse::Status::Complete(end) = response.parse(&wire).unwrap() else {
        panic!("partial")
    };
    assert_eq!(response.code, Some(200));
    let length = response
        .headers
        .iter()
        .find(|h| h.name == "Content-Length")
        .unwrap()
        .value;
    assert_eq!(length, (wire.len() - end).to_string().as_bytes());
    assert!(std::str::from_utf8(&wire[end..]).unwrap().contains("12345"));
    assert_eq!(local_response(addr).unwrap(), local_response(addr).unwrap());
}
#[test]
fn elapsed_socket_deadline_returns_without_waiting_for_peer() {
    use std::{
        net::{TcpListener, TcpStream},
        time::{Duration, Instant},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let _client =
        TcpStream::connect_timeout(&listener.local_addr().unwrap(), Duration::from_secs(2))
            .unwrap();
    let (mut server, _) = listener.accept().unwrap();
    assert_eq!(
        receive_socket(&mut server, Instant::now()).result,
        Err(Failure::Deadline)
    );
}
