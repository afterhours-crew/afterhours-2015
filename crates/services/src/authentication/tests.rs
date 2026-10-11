// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
fn config() -> Config {
    Config {
        first_console: false,
        first_login: false,
        locale: 64,
        namespace: b"local".to_vec(),
        session_type: 1,
        object_parts: [3, 7],
        filter: b"-all".to_vec(),
        no_toggle_ok: b"false".to_vec(),
        use_server_time: b"true".to_vec(),
        service_name: b"test".to_vec(),
        ticker_key_length: 57,
    }
}
fn identity(id: u8) -> Identity {
    Identity::new(
        nfs_storage::AccountId::from_owned_config([id; 16]).unwrap(),
        i64::from(id) * 100,
        i64::from(id) * 100 + 1,
        format!("Local {id}"),
    )
    .unwrap()
}
fn profile(id: u8) -> Profile {
    Profile::new(
        config(),
        identity(id),
        Tokens::from_seed(&[id; SEED_BYTES]).unwrap(),
        "127.0.0.1:32100".parse().unwrap(),
    )
    .unwrap()
}
fn request(route: [u16; 2], body: &[u8], correlation: u32) -> Vec<u8> {
    encode(
        Fields {
            routing_a: route[0],
            routing_b: route[1],
            correlation,
            ..Default::default()
        },
        &[],
        body,
    )
    .unwrap()
}
fn login() -> Vec<u8> {
    request(
        [1, 10],
        &LoginRequest {
            auth_code: Some(b"LOCAL_TEST_ONLY"),
            external_blob: Some(Blob(&[])),
            external_id: Some(0),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap(),
        4,
    )
}
fn postauth() -> Vec<u8> {
    request(
        [9, 8],
        &PostAuthRequest {
            dirty_sock_user_index: Some(0),
            unique_device_id: Some(b""),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap(),
        5,
    )
}
fn decoded(wire: &[u8]) -> Frame<'_> {
    nfs_fire2::decode(wire, frame_limits())
        .unwrap()
        .unwrap()
        .frame
}
#[test]
fn ordered_commit_retry_and_failed_write() {
    let p = profile(1);
    let mut exchange = Exchange::new(&p);
    assert_eq!(exchange.response(&postauth(), 100).unwrap(), None);
    let batch = exchange.response(&login(), 100).unwrap().unwrap();
    assert_eq!(batch.len(), 4);
    assert_eq!(
        batch
            .iter()
            .map(|b| {
                let f = decoded(b);
                (f.fields.routing_a, f.fields.routing_b, f.fields.category)
            })
            .collect::<Vec<_>>(),
        vec![(30722, 8, 2), (1, 10, 1), (30722, 2, 2), (30722, 5, 2)]
    );
    assert_eq!(exchange.response(&postauth(), 101), Err(Error::Pending));
    assert!(!exchange.login_complete());
    exchange.committed().unwrap();
    assert!(exchange.login_complete());
    assert_eq!(exchange.response(&login(), 999).unwrap().unwrap(), batch);
    exchange.committed().unwrap();
    let reply = exchange.response(&postauth(), 999).unwrap().unwrap();
    assert_eq!(exchange.stage(), Stage::PostAuth);
    exchange.committed().unwrap();
    assert_eq!(exchange.stage(), Stage::Ready);
    assert_eq!(
        exchange.response(&postauth(), 1000).unwrap().unwrap(),
        reply
    );
    exchange.write_failed();
    assert_eq!(exchange.response(&login(), 1), Err(Error::Closed));
    assert_eq!(exchange.committed(), Err(Error::Closed));
    let mut partial = Exchange::new(&p);
    partial.response(&login(), 100).unwrap();
    partial.write_failed();
    assert!(!partial.login_complete());
}
#[test]
fn identities_tokens_notifications_and_local_endpoints() {
    let first = profile(1);
    let second = profile(2);
    let batch = first.login_batch(&login(), 123).unwrap();
    let n = UserSessionLoginInfo::decode(decoded(&batch[0]).body, body_limits()).unwrap();
    assert_eq!(
        (
            n.persona_id,
            n.user_id,
            n.display_name,
            n.last_authenticated
        ),
        (Some(100), Some(101), Some(&b"Local 1"[..]), Some(123))
    );
    assert_eq!(decoded(&batch[0]).fields.reserved, [1, 0]);
    assert_eq!(decoded(&batch[0]).fields.correlation, 0);
    let f = decoded(&batch[1]);
    let meta = Fire2Metadata::decode(f.metadata, body_limits()).unwrap();
    let info = LoginResponse::decode(f.body, body_limits())
        .unwrap()
        .user_login_info
        .unwrap();
    assert_eq!(meta.session_key, n.session_key);
    assert_eq!(info.session_key, n.session_key);
    assert_eq!(info.last_login_date_time, Some(123));
    let added = NotifyUserAddedInitial::decode(decoded(&batch[2]).body, body_limits()).unwrap();
    assert_eq!(added.user_info.unwrap().account_id, Some(101));
    let status = UserStatus::decode(decoded(&batch[3]).body, body_limits()).unwrap();
    assert_eq!((status.blaze_id, status.status_flags), (Some(100), Some(3)));
    let post = first.postauth_reply(&postauth()).unwrap();
    let post = PostAuthResponse::decode(decoded(&post).body, body_limits()).unwrap();
    assert_eq!(post.ticker_server.as_ref().unwrap().key.unwrap().len(), 57);
    assert_eq!(
        post.telemetry_server.as_ref().unwrap().address,
        Some(&b"127.0.0.1"[..])
    );
    assert_eq!(post.user_options.unwrap().user_id, Some(100));
    let other = second.login_batch(&login(), 123).unwrap();
    assert_ne!(batch, other);
    let reconnect = Profile::new(
        config(),
        identity(1),
        Tokens::from_seed(&[3; SEED_BYTES]).unwrap(),
        "127.0.0.1:32100".parse().unwrap(),
    )
    .unwrap();
    let again = reconnect.login_batch(&login(), 124).unwrap();
    let again = UserSessionLoginInfo::decode(decoded(&again[0]).body, body_limits()).unwrap();
    assert_eq!(n.persona_id, again.persona_id);
    assert_ne!(n.session_key, again.session_key);
    assert_ne!(
        n.connection_group_object_id,
        again.connection_group_object_id
    );
    assert!(
        batch
            .iter()
            .all(|b| !b.windows(15).any(|v| v == b"LOCAL_TEST_ONLY"))
    );
}
#[test]
fn malformed_requests_do_not_advance_or_leak() {
    let p = profile(1);
    let wire = login();
    for length in 0..wire.len() {
        assert!(!eligible_login(&wire[..length]));
        let mut s = Exchange::new(&p);
        assert!(s.response(&wire[..length], 1).is_ok_and(|v| v.is_none()));
        assert_eq!(s.stage(), Stage::Login);
    }
    assert!(!eligible_login(&[wire.clone(), wire.clone()].concat()));
    let f = decoded(&wire);
    let bad = request([1, 10], &[f.body, &[1, 2, 3, 0]].concat(), 4);
    assert!(!eligible_login(&bad));
    let meta = encode(f.fields, b"unexpected", f.body).unwrap();
    assert!(!eligible_login(&meta));
    let mut s = Exchange::new(&p);
    s.response(&wire, 1).unwrap();
    s.committed().unwrap();
    let mut other = decoded(&wire).fields;
    other.correlation += 1;
    assert_eq!(
        s.response(&encode(other, &[], f.body).unwrap(), 1).unwrap(),
        None
    );
    let bad = request(
        [9, 8],
        &PostAuthRequest {
            dirty_sock_user_index: Some(1),
            unique_device_id: Some(b""),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap(),
        5,
    );
    assert_eq!(s.response(&bad, 1), Err(Error::Ineligible));
    assert_eq!(s.stage(), Stage::PostAuth);
}
#[test]
fn bounded_policy_and_entropy() {
    for count in [0, 1, SEED_BYTES - 1, SEED_BYTES + 1] {
        assert!(Tokens::from_seed(&vec![1; count]).is_err());
    }
    for (start, end) in [(0, 32), (36, 42), (42, 86), (86, 115)] {
        let mut bytes = [1; SEED_BYTES];
        bytes[start..end].fill(0);
        assert!(Tokens::from_seed(&bytes).is_err());
    }
    let mut c = config();
    c.namespace = vec![b'x'; 65];
    assert_eq!(c.validate(), Err(Error::Config));
    assert!(Identity::new(identity(1).storage_account(), 0, 2, "a".into()).is_err());
    assert!(Identity::new(identity(1).storage_account(), 1, 2, "\n".into()).is_err());
    assert!(
        Profile::new(
            config(),
            identity(1),
            Tokens::from_seed(&[1; SEED_BYTES]).unwrap(),
            "192.0.2.1:80".parse().unwrap()
        )
        .is_err()
    );
    assert!(Config::from_json(&serde_json::json!({"version":1})).is_err());
}

#[test]
fn deployment_and_local_identity_documents_are_strict() {
    let mut v = serde_json::json!({"version":1,"build_sha256":crate::SUPPORTED_BUILD_SHA256,"first_console":false,"first_login":false,"locale":64,"namespace":"local","session_type":1,"object_parts":[3,7],"filter":"-all","no_toggle_ok":"false","use_server_time":"true","service_name":"test","ticker_key_length":58});
    assert_eq!(Config::from_json(&v).unwrap().ticker_key_length, 58);
    v["ticker_key_length"] = 59.into();
    assert!(Config::from_json(&v).is_err());
    v["ticker_key_length"] = 58.into();
    v["unknown"] = true.into();
    assert!(Config::from_json(&v).is_err());
    let mut v = serde_json::json!({"version":1,"storage_account":"01010101010101010101010101010101","persona":100,"account":101,"name":"Local 1"});
    let id = Identity::from_json(&v).unwrap();
    assert_eq!(id.storage_account(), identity(1).storage_account());
    assert_eq!(id.persona_id(), 100);
    v["storage_account"] = "00000000000000000000000000000000".into();
    assert!(Identity::from_json(&v).is_err());
    v["storage_account"] = "01010101010101010101010101010101".into();
    v["account"] = 100.into();
    assert!(Identity::from_json(&v).is_err());
    v["account"] = 101.into();
    v["version"] = 2.into();
    assert!(Identity::from_json(&v).is_err());
}
