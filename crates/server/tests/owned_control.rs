// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_protocol::{Blob, authentication::LoginRequest, util::*};
use nfs_server::{
    Failure,
    auxiliary::EmptyLocalRecords,
    control::{ControlSession, Outcome, PhaseName},
    deployment::SessionProfiles,
    limits::control_body_limits,
    startup_profile,
};
use nfs_services::{authentication as auth, bootstrap};

fn profiles(id: u8) -> SessionProfiles {
    let bootstrap = bootstrap::Config::from_json(
        &serde_json::from_slice(include_bytes!("support/bootstrap.json")).unwrap(),
    )
    .unwrap();
    let identity = auth::Identity::new(
        nfs_storage::AccountId::from_owned_config([id; 16]).unwrap(),
        i64::from(id) * 100,
        i64::from(id) * 100 + 1,
        format!("Local {id}"),
    )
    .unwrap();
    let persona = identity.persona_id();
    let records = EmptyLocalRecords::new(persona, identity.account_id()).unwrap();
    SessionProfiles {
        bootstrap: bootstrap::Profile::new(
            &bootstrap,
            "127.0.0.1:3000".parse().unwrap(),
            "127.0.0.1:3001".parse().unwrap(),
        )
        .unwrap(),
        auth: auth::Profile::new(
            auth::Config {
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
            },
            identity,
            auth::Tokens::from_seed(&[id; auth::SEED_BYTES]).unwrap(),
            "127.0.0.1:3000".parse().unwrap(),
        )
        .unwrap(),
        startup: startup_profile::Profile::owned(persona).unwrap(),
        mac_template: None,
        persona,
        records,
    }
}
fn frame(component: u16, command: u16, body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        nfs_fire2::Frame {
            fields: nfs_fire2::Fields {
                routing_a: component,
                routing_b: command,
                correlation: u32::from(component) * 100 + u32::from(command),
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        nfs_server::frame_limits(),
    )
    .unwrap()
}
fn preauth() -> Vec<u8> {
    frame(
        9,
        7,
        &PreAuthRequest {
            client_data: Some(ClientData {
                client_type: Some(0),
                service_name: Some(b"local"),
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
        .encode(control_body_limits())
        .unwrap(),
    )
}
fn identity() -> Vec<u8> {
    frame(
        9,
        1,
        &FetchClientConfigRequest {
            config_section: Some(b"IdentityParams"),
            ..Default::default()
        }
        .encode(control_body_limits())
        .unwrap(),
    )
}
fn login() -> Vec<u8> {
    frame(
        1,
        10,
        &LoginRequest {
            auth_code: Some(b"LOCAL_TEST_ONLY"),
            external_blob: Some(Blob(&[])),
            external_id: Some(0),
            ..Default::default()
        }
        .encode(control_body_limits())
        .unwrap(),
    )
}
fn postauth() -> Vec<u8> {
    frame(
        9,
        8,
        &PostAuthRequest {
            dirty_sock_user_index: Some(0),
            unique_device_id: Some(b""),
            ..Default::default()
        }
        .encode(control_body_limits())
        .unwrap(),
    )
}
fn reply(session: &mut ControlSession<'_>, request: &[u8]) -> Vec<Vec<u8>> {
    let outcome = session
        .on_frame(request, 100, 100_000_000)
        .unwrap_or_else(|error| {
            panic!(
                "{error:?} in {:?} for {:?}",
                session.phase(),
                nfs_server::record::Route::of(request)
            )
        });
    let Outcome::Reply(frames) = outcome else {
        panic!(
            "expected modeled reply in {:?} for {:?}",
            session.phase(),
            nfs_server::record::Route::of(request)
        )
    };
    frames
}
fn bootstrap(session: &mut ControlSession<'_>) {
    assert_eq!(reply(session, &preauth()).len(), 1);
    session.committed().unwrap();
    assert_eq!(reply(session, &frame(9, 2, &[])).len(), 1);
    session.committed().unwrap();
    assert_eq!(reply(session, &identity()).len(), 1);
    session.committed().unwrap();
}

#[test]
fn owned_authentication_commits_before_startup_and_isolates_connections() {
    let a = profiles(1);
    let b = profiles(2);
    let mut a = ControlSession::new(&a);
    let mut b = ControlSession::new(&b);
    assert_eq!(
        a.on_frame(&login(), 100, 100_000_000),
        Ok(Outcome::Unsupported)
    );
    bootstrap(&mut a);
    bootstrap(&mut b);
    let login_a = reply(&mut a, &login());
    let login_b = reply(&mut b, &login());
    assert_eq!(login_a.len(), 4);
    assert_ne!(login_a, login_b);
    assert_eq!(a.phase(), PhaseName::PostAuth);
    assert_eq!(
        a.on_frame(&postauth(), 100, 100_000_000),
        Err(Failure::ProfileConfig)
    );
    a.committed().unwrap();
    assert_eq!(reply(&mut a, &login()), login_a);
    a.committed().unwrap();
    assert_eq!(reply(&mut a, &postauth()).len(), 1);
    assert_eq!(a.phase(), PhaseName::PostAuth);
    a.committed().unwrap();
    assert_eq!(a.phase(), PhaseName::Startup);
    assert_ne!(
        a.identities().unwrap().persona,
        b.identities().unwrap().persona
    );
    assert_eq!(
        a.on_frame(&frame(60000, 60000, &[]), 100, 100_000_000),
        Ok(Outcome::Unsupported)
    );
    b.write_failed();
    assert_eq!(b.phase(), PhaseName::Closed);
    assert_eq!(
        b.on_frame(&postauth(), 100, 100_000_000),
        Ok(Outcome::Unsupported)
    );
    assert_eq!(a.phase(), PhaseName::Startup);
}

#[test]
fn failed_bootstrap_write_never_opens_identity_or_authentication() {
    let p = profiles(3);
    let mut session = ControlSession::new(&p);
    reply(&mut session, &preauth());
    session.write_failed();
    assert_eq!(session.phase(), PhaseName::Closed);
    assert_eq!(
        session.on_frame(&identity(), 100, 100_000_000),
        Ok(Outcome::Unsupported)
    );
    assert_eq!(
        session.on_frame(&login(), 100, 100_000_000),
        Ok(Outcome::Unsupported)
    );
}
