// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use serde_json::json;
fn profile(port: u16) -> Profile {
    Profile::new(
        &Config::from_json(&content::constructed()).unwrap(),
        ([127, 0, 0, 1], port).into(),
        ([127, 0, 0, 1], port + 1).into(),
    )
    .unwrap()
}
fn wire(command: u16, body: &[u8], correlation: u32) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 9,
                routing_b: command,
                correlation,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .unwrap()
}
fn preauth(service: &[u8]) -> Vec<u8> {
    wire(
        7,
        &PreAuthRequest {
            client_data: Some(ClientData {
                client_type: Some(0),
                service_name: Some(service),
                ..Default::default()
            }),
            client_info: Some(ClientInfo {
                platform: Some(4),
                ..Default::default()
            }),
            fetch_client_config: Some(FetchClientConfigRequest {
                config_section: Some(b"local"),
                ..Default::default()
            }),
            local_address: Some(0),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap(),
        45,
    )
}
fn identity() -> Vec<u8> {
    wire(
        1,
        &FetchClientConfigRequest {
            config_section: Some(b"IdentityParams"),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap(),
        47,
    )
}
fn body(wire: &[u8]) -> &[u8] {
    nfs_fire2::decode(wire, frame_limits())
        .unwrap()
        .unwrap()
        .frame
        .body
}

#[test]
fn owned_configuration_binds_every_endpoint_and_keeps_bandwidth_unset() {
    let p = profile(2345);
    let mut session = Session::new(&p);
    let reply = session.response(&preauth(b"local"), 5).unwrap().unwrap();
    let f = nfs_fire2::decode(&reply, frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    assert_eq!(
        (
            f.fields.routing_a,
            f.fields.routing_b,
            f.fields.category,
            f.fields.correlation
        ),
        (9, 7, 1, 45)
    );
    let response = PreAuthResponse::decode(f.body, body_limits()).unwrap();
    assert_eq!(response.machine_id, Some(1));
    assert_eq!(response.server_version, Some(&b"local\nserver"[..]));
    let config = response.config.unwrap().config.unwrap().0;
    assert_eq!(config.len(), 77);
    for (key, value) in config {
        let key = std::str::from_utf8(key).unwrap();
        let expected: &[u8] = match key {
            "bytevaultHostname" => b"127.0.0.1",
            "bytevaultPort" | "telemetryPinServerPort" => b"2345",
            "bytevaultSecure" => b"0",
            "xblTokenUrn" => b"",
            _ if content::URL_KEYS.contains(&key) => b"http://127.0.0.1:2345",
            _ => b"12",
        };
        assert_eq!(value, expected);
    }
    let qos = response.qos_settings.unwrap();
    let bandwidth = qos.bandwidth_ping_site_info.unwrap();
    assert_eq!(
        (bandwidth.address, bandwidth.port, bandwidth.site_name),
        (Some(&b""[..]), Some(0), Some(&b""[..]))
    );
    let sites = qos.ping_site_info_by_alias_map.unwrap().0;
    assert_eq!(sites[0].1.address, Some(&b"127.0.0.1"[..]));
    assert_eq!(sites[0].1.port, Some(2346));
}

#[test]
fn write_commit_order_retry_time_and_failure_are_per_connection() {
    let a = profile(3000);
    let b = profile(4000);
    let mut a = Session::new(&a);
    let mut b = Session::new(&b);
    assert_eq!(a.response(&identity(), 1), Ok(None));
    let query = preauth(b"local");
    let initial = a.response(&query, 1).unwrap().unwrap();
    assert_eq!(a.stage(), Stage::PreAuth);
    assert_eq!(a.response(&query, 1), Err(Error::Pending));
    a.committed().unwrap();
    assert_eq!(a.stage(), Stage::Ping);
    assert_eq!(a.response(&query, 9).unwrap().unwrap(), initial);
    a.committed().unwrap();
    let other = b.response(&query, 1).unwrap().unwrap();
    assert_ne!(other, initial);
    b.write_failed();
    assert_eq!(b.response(&query, 1), Err(Error::Closed));
    assert_eq!(b.committed(), Err(Error::Closed));
    let query = wire(2, &[], 46);
    let ping = a.response(&query, 17).unwrap().unwrap();
    a.committed().unwrap();
    assert_eq!(
        PingResponse::decode(body(&ping), body_limits())
            .unwrap()
            .server_time,
        Some(17)
    );
    assert_eq!(a.response(&query, 99).unwrap().unwrap(), ping);
    a.committed().unwrap();
    let response = a.response(&identity(), 99).unwrap().unwrap();
    assert!(!a.identity_complete());
    let values = FetchConfigResponse::decode(body(&response), body_limits())
        .unwrap()
        .config
        .unwrap()
        .0;
    assert_eq!(
        values,
        vec![
            (&b"client_id"[..], &b"local"[..]),
            (b"display", b"local"),
            (b"redirect_uri", b"http://127.0.0.1:3000/identity/callback")
        ]
    );
    a.committed().unwrap();
    assert!(a.identity_complete());
    a.write_failed();
    assert!(!a.identity_complete());
}

#[test]
fn malformed_partial_concatenated_wrong_service_and_unknown_routes_do_not_advance() {
    let p = profile(3000);
    let mut session = Session::new(&p);
    let query = preauth(b"local");
    for size in 0..query.len() {
        assert_eq!(session.response(&query[..size], 1), Err(Error::Ineligible));
        assert_eq!(session.stage(), Stage::PreAuth);
    }
    for invalid in [
        preauth(b"foreign"),
        [query.clone(), query.clone()].concat(),
        vec![0; 65537],
    ] {
        assert_eq!(session.response(&invalid, 1), Err(Error::Ineligible));
    }
    assert_eq!(session.response(&wire(99, &[], 1), 1), Ok(None));
    assert_eq!(session.stage(), Stage::PreAuth);
    session.response(&query, 1).unwrap();
    session.committed().unwrap();
    assert_eq!(session.response(&wire(2, &[0], 1), 1), Ok(None));
    assert_eq!(session.stage(), Stage::Ping);
}

#[test]
fn configuration_bounds_duplicate_keys_remote_endpoints_and_unknown_fields_fail() {
    let valid = content::constructed();
    for change in [0, 1, 2, 3, 4, 5, 6] {
        let mut v = valid.clone();
        match change {
            0 => v["version"] = json!(2),
            1 => v["metadata"]["service_name"] = json!(""),
            2 => v["settings"][1] = v["settings"][0].clone(),
            3 => v["qos"]["probes"] = json!(65536),
            4 => v["qos"]["sites"][0]["host"] = json!("remote"),
            5 => v["components"] = json!([9, 9]),
            _ => v["metadata"]["server_version"] = json!("x".repeat(257)),
        };
        assert!(Config::from_json(&v).is_err());
    }
    let config = Config::from_json(&valid).unwrap();
    assert!(
        Profile::new(
            &config,
            ([192, 0, 2, 1], 80).into(),
            ([127, 0, 0, 1], 80).into()
        )
        .is_err()
    );
    assert!(
        Profile::new(
            &config,
            ([127, 0, 0, 1], 80).into(),
            ([127, 0, 0, 1], 0).into()
        )
        .is_err()
    );
}
