// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::{Blob, gamemanager::*, users::*, util::ConfigEntries};
use nfs_services::group::{self, Config, Generated, Identity, Session, State};

fn config() -> Config {
    Config {
        protocol_version: b"test-version".to_vec(),
        protocol_hash: 123,
        ping_site: b"local".to_vec(),
        mode_attribute: b"mode".to_vec(),
        telemetry_interval: 10_000_000,
    }
}
fn user(persona: i64) -> UserIdentification<'static> {
    UserIdentification {
        account_id: Some(persona + 100),
        account_locale: Some(1),
        external_blob: Some(Blob(b"")),
        external_id: Some(persona as u64 + 100),
        blaze_id: Some(persona),
        name: Some(b"Test Driver"),
        persona_namespace: Some(b"local"),
        origin_persona_id: Some(0),
        pid_id: Some(0),
        ..Default::default()
    }
}
fn qos() -> NetworkQosData<'static> {
    NetworkQosData {
        downstream_bits_per_second: Some(1000),
        upstream_bits_per_second: Some(2000),
        nat_type: Some(0),
        bandwidth_error_code: Some(0),
        nat_error_code: Some(0),
        ..Default::default()
    }
}
fn generated(n: u8) -> Generated {
    Generated::from_seed(&std::array::from_fn::<_, 48, _>(|i| i as u8 + n), 123_456).unwrap()
}
fn connection(persona: i64) -> ObjectId {
    ObjectId(30722, 2, persona + 200)
}
fn session(persona: i64, n: u8) -> Session {
    Session::new(
        config(),
        Identity::new(&user(persona), connection(persona), &qos()).unwrap(),
        generated(n),
    )
    .unwrap()
}
fn network() -> NetworkAddress<'static> {
    let address = || IpAddress {
        ip: Some(0x7f000001),
        port: Some(12345),
        machine_id: Some(0),
        ..Default::default()
    };
    NetworkAddress::IpPair(IpPairAddress {
        external_address: Some(address()),
        internal_address: Some(address()),
        machine_id: Some(1),
        ..Default::default()
    })
}
fn create(persona: i64, private: bool) -> CreateGameRequest<'static> {
    CreateGameRequest {
        common_game_data: Some(CommonGameRequestData {
            game_type: Some(1),
            game_protocol_version_string: Some(b"test-version"),
            originating_scenario_id: Some(0),
            player_network_address: Some(network()),
            x_lspnetwork_address: Some(NetworkAddress::Unset),
            scenario_info: Some(ScenarioInfo {
                scenario_name: Some(b""),
                scenario_version: Some(0),
                scenario_variant: Some(0),
                sub_session_name: Some(b""),
                ..Default::default()
            }),
            ..Default::default()
        }),
        game_ping_site_alias: Some(b""),
        game_creation_data: Some(GameCreationData {
            game_attribs: Some(ConfigEntries(vec![
                (b"gameSessionId", b"0"),
                (b"Matchmaking", b"Idle"),
                (b"mode", b"Group"),
                (b"Type", if private { b"Private" } else { b"Public" }),
            ])),
            game_mod_register: Some(0),
            game_name: Some(b""),
            game_settings: Some(if private { 1_082_384 } else { 1_082_397 }),
            network_topology: Some(255),
            max_player_capacity: Some(8),
            min_player_capacity: Some(1),
            presence_mode: Some(1),
            queue_capacity: Some(0),
            role_information: Some(RoleInformation::default()),
            external_session_template_name: Some(b""),
            voip_network: Some(2),
            ..Default::default()
        }),
        game_report_name: Some(b""),
        game_status_url: Some(b""),
        server_not_resetable: Some(false),
        slot_capacities: Some(CapacityList(vec![8, 0, 0, 0])),
        persisted_game_id: Some(b""),
        persisted_game_id_secret: Some(Blob(b"")),
        team_ids: Some(CapacityList(vec![65534])),
        player_join_data: Some(PlayerJoinData {
            group_id: Some(connection(persona)),
            default_role: Some(b""),
            game_entry_type: Some(0),
            player_data_list: Some(PlayerJoinList(vec![PerPlayerJoinData {
                is_optional_player: Some(false),
                role: Some(b""),
                user: Some(user(persona)),
                ..Default::default()
            }])),
            slot_type: Some(0),
            team_id: Some(65534),
            team_index: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn wire(command: u16, correlation: u32, body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 4,
                routing_b: command,
                correlation,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        group::frame_limits(),
    )
    .unwrap()
}
fn create_wire(persona: i64, private: bool) -> Vec<u8> {
    wire(
        CREATE_GAME,
        1,
        &create(persona, private)
            .encode(group::body_limits())
            .unwrap(),
    )
}
fn mesh(persona: i64, game: u64) -> Vec<u8> {
    wire(
        UPDATE_MESH_CONNECTION,
        2,
        &UpdateMeshConnectionRequest {
            player_net_connection_flags: Some(0),
            game_id: Some(game),
            player_net_connection_status: Some(2),
            qos_info: Some(MeshConnectionQosInfo {
                packet_loss: Some(PacketLossBits(0)),
                latency_ms: Some(0),
                ..Default::default()
            }),
            source_group_id: Some(connection(persona)),
            target_group_id: Some(connection(persona)),
            ..Default::default()
        }
        .encode(group::body_limits())
        .unwrap(),
    )
}
fn finalize(game: u64) -> Vec<u8> {
    wire(
        FINALIZE_GAME_CREATION,
        3,
        &UpdateGameSessionRequest {
            game_id: Some(game),
            np_session_id: Some(b""),
            xnet_nonce: Some(Blob(b"")),
            xnet_session: Some(Blob(b"")),
            ..Default::default()
        }
        .encode(group::body_limits())
        .unwrap(),
    )
}
fn initialize(game: u64) -> Vec<u8> {
    wire(
        ADVANCE_GAME_STATE,
        4,
        &AdvanceGameStateRequest {
            game_id: Some(game),
            new_game_state: Some(16),
            ..Default::default()
        }
        .encode(group::body_limits())
        .unwrap(),
    )
}
fn body(w: &[u8]) -> &[u8] {
    nfs_fire2::decode(w, group::frame_limits())
        .unwrap()
        .unwrap()
        .frame
        .body
}

#[test]
fn public_and_private_setup_come_from_current_membership_and_request() {
    for private in [false, true] {
        let mut s = session(10, 1);
        let query = create_wire(10, private);
        let replies = s.response(&query).unwrap().unwrap();
        assert_eq!(replies.len(), 2);
        let setup = NotifyGameSetup::decode(body(&replies[1]), group::body_limits()).unwrap();
        let g = setup.game_data.unwrap();
        assert_eq!(g.game_id, Some(generated(1).game_id));
        assert_eq!(g.game_reporting_id, Some(generated(1).reporting_id));
        assert_eq!(g.admin_player_list.unwrap().0, [10]);
        assert_eq!(
            g.game_settings,
            Some(if private { 1_082_384 } else { 1_082_397 })
        );
        assert_eq!(
            g.game_attribs.unwrap().0.last().unwrap().1,
            if private {
                b"Private".as_slice()
            } else {
                b"Public"
            }
        );
        assert_eq!(
            g.network_qos_data.unwrap().upstream_bits_per_second,
            Some(2000)
        );
        assert_eq!(g.platform_host_info.unwrap().connection_group_id, Some(210));
        let player = &setup.game_roster.unwrap().0[0];
        assert_eq!(player.player_id, Some(10));
        assert_eq!(player.external_id, Some(110));
        assert_eq!(player.user_group_id, Some(connection(10)));
        assert_eq!(player.joined_game_timestamp, Some(123456));
        assert!(s.matchmaking_context().is_none());
        s.commit_after_write();
        assert_eq!(
            s.response(&query).unwrap().unwrap(),
            vec![replies[0].clone()]
        );
        for request in [
            mesh(10, generated(1).game_id),
            finalize(generated(1).game_id),
        ] {
            assert_eq!(s.response(&request).unwrap().unwrap().len(), 1);
            s.commit_after_write();
            assert!(s.matchmaking_context().is_none());
        }
        let last = s
            .response(&initialize(generated(1).game_id))
            .unwrap()
            .unwrap();
        assert_eq!(last.len(), 2);
        assert!(s.matchmaking_context().is_none());
        s.commit_after_write();
        let c = s.matchmaking_context().unwrap();
        assert_eq!(c.group(), generated(1).game_id);
        assert_eq!(c.persona(), 10);
        assert_eq!(c.setup(), &replies[1]);
        assert_eq!(
            s.response(&initialize(generated(1).game_id))
                .unwrap()
                .unwrap(),
            vec![last[0].clone()]
        );
    }
}

#[test]
fn foreign_owner_or_mesh_and_wrong_order_never_authorize_matchmaking() {
    assert!(session(10, 1).response(&create_wire(20, false)).is_err());
    assert!(
        session(10, 1)
            .response(&mesh(10, generated(1).game_id))
            .unwrap()
            .is_none()
    );
    let mut s = session(10, 1);
    s.response(&create_wire(10, false)).unwrap();
    s.commit_after_write();
    assert!(s.response(&mesh(20, generated(1).game_id)).is_err());
    assert_eq!(s.state(), State::ObserveOnly);
    assert!(s.matchmaking_context().is_none());
}

#[test]
fn unrelated_sessions_allocate_separately_and_write_failure_revokes_handoff() {
    let mut a = session(10, 1);
    let mut b = session(20, 2);
    let ar = a.response(&create_wire(10, false)).unwrap().unwrap();
    let br = b.response(&create_wire(20, true)).unwrap().unwrap();
    assert_ne!(ar, br);
    assert_ne!(a.game_id(), b.game_id());
    assert!(a.response(&mesh(10, generated(1).game_id)).is_err());
    a.abort_write();
    assert_eq!(a.game_id(), None);
    assert!(a.matchmaking_context().is_none());
    assert!(a.response(&create_wire(10, false)).unwrap().is_none());
    b.commit_after_write();
    assert!(
        b.response(&mesh(20, generated(2).game_id))
            .unwrap()
            .is_some()
    );
}

#[test]
fn malformed_partial_concatenated_and_unknown_fields_are_rejected() {
    let good = create_wire(10, false);
    for wire in [
        &good[..good.len() - 1],
        &[good.clone(), good.clone()].concat(),
        &vec![0; 40000],
    ] {
        assert!(!matches!(session(10, 1).response(wire), Ok(Some(_))));
    }
    let mut payload = create(10, false).encode(group::body_limits()).unwrap();
    let mut field = nfs_heat2::Encoder::new(group::body_limits());
    field.integer([0xeb, 0xae, 0xba], 1).unwrap();
    payload.extend(field.finish().unwrap());
    assert!(
        session(10, 1)
            .response(&wire(CREATE_GAME, 1, &payload))
            .is_err()
    );
    let mut bad = create(10, true);
    bad.game_creation_data.as_mut().unwrap().game_settings = Some(1_082_397);
    assert!(
        session(10, 1)
            .response(&wire(
                CREATE_GAME,
                1,
                &bad.encode(group::body_limits()).unwrap()
            ))
            .is_err()
    );
}

#[test]
fn altered_correlations_and_request_budget_fail_closed() {
    let request = create_wire(10, false);
    let mut s = session(10, 1);
    s.response(&request).unwrap();
    s.commit_after_write();
    let mut changed = nfs_fire2::decode(&request, group::frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    changed.fields.routing_b = FINALIZE_GAME_CREATION;
    assert!(
        s.response(&nfs_fire2::encode(changed, group::frame_limits()).unwrap())
            .is_err()
    );
    let mut s = session(10, 1);
    s.response(&request).unwrap();
    s.commit_after_write();
    for _ in 1..group::MAX_REQUESTS {
        assert!(s.response(&request).unwrap().is_some());
    }
    assert!(s.response(&request).unwrap().is_none());
    assert_eq!(s.state(), State::ObserveOnly);
}

#[test]
fn configuration_and_generated_state_are_bounded() {
    let mut policy = config();
    policy.protocol_version = vec![b'a'; 65];
    assert!(
        Session::new(
            policy,
            Identity::new(&user(10), connection(10), &qos()).unwrap(),
            generated(1)
        )
        .is_err()
    );
    assert!(Generated::from_seed(&[0; 48], 1).is_err());
    assert!(Generated::from_seed(&[1; 47], 1).is_err());
    assert!(Generated::from_seed(&[1; 48], 0).is_err());
    let mut ids = generated(1);
    ids.game_id = u64::MAX;
    assert!(
        Session::new(
            config(),
            Identity::new(&user(10), connection(10), &qos()).unwrap(),
            ids
        )
        .is_err()
    );
    assert!(Config::from_json(&serde_json::json!({})).is_err());
}

#[path = "support/matchmaking_admission.rs"]
mod matchmaking_admission;
