// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_services::matchmaking::{AttributeRule, Config as Policy};
fn policy() -> Policy {
    Policy {
        protocol_version: b"test-version".to_vec(),
        game_rules: vec![AttributeRule {
            name: b"mode-rule".to_vec(),
            threshold: b"exact".to_vec(),
            values: vec![b"solo".to_vec()],
        }],
        ued_rule: b"skill-rule".to_vec(),
        ued_threshold: b"default".to_vec(),
        player_attributes: vec![
            (b"mode".to_vec(), b"solo".to_vec()),
            (b"variant".to_vec(), b"test".to_vec()),
        ],
        default_role: b"driver".to_vec(),
        max_players: 4,
        player_count_threshold: b"count".to_vec(),
        utilization_threshold: b"fill".to_vec(),
        ping_site_threshold: b"ping".to_vec(),
        desired_percent_full: 50,
        creation_settings: 7,
        duration_micros: 10_000,
        starting_decay_ages: vec![0, 500],
    }
}
fn context(persona: i64, seed: u8) -> group::Context {
    let mut s = session(persona, seed);
    let game = generated(seed).game_id;
    for q in [
        create_wire(persona, false),
        mesh(persona, game),
        finalize(game),
        initialize(game),
    ] {
        s.response(&q).unwrap().unwrap();
        s.commit_after_write();
    }
    s.matchmaking_context().unwrap().clone()
}
fn query<'a>(p: &'a Policy, g: &'a group::Context, age: i64) -> StartMatchmakingRequest<'a> {
    use nfs_protocol::autolog::StringList;
    let c = CommonGameRequestData::decode(g.network(), group::body_limits()).unwrap();
    StartMatchmakingRequest {
        common_game_data: Some(CommonGameRequestData {
            game_type: Some(0),
            game_protocol_version_string: Some(&p.protocol_version),
            originating_scenario_id: Some(0),
            player_network_address: c.player_network_address,
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
        criteria_data: Some(MatchmakingCriteriaData {
            avoid_games_rule_criteria: Some(AvoidGamesRuleCriteria::default()),
            avoid_players_rule_criteria: Some(AvoidPlayersRuleCriteria::default()),
            free_player_slots_rule_criteria: Some(FreePlayerSlotsRuleCriteria {
                max_free_player_slots: Some(u16::MAX),
                min_free_player_slots: Some(0),
                ..Default::default()
            }),
            game_attribute_rule_criteria_map: Some(GameAttributeRuleCriteriaMap(
                p.game_rules
                    .iter()
                    .map(|r| {
                        (
                            r.name.as_slice(),
                            GameAttributeRuleCriteria {
                                min_fit_threshold_name: Some(&r.threshold),
                                desired_values: Some(StringList(
                                    r.values.iter().map(Vec::as_slice).collect(),
                                )),
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
            )),
            geo_location_rule_criteria: Some(GeoLocationRuleCriteria {
                min_fit_threshold_name: Some(b""),
                ..Default::default()
            }),
            game_name_rule_criteria: Some(GameNameRuleCriteria {
                search_string: Some(b""),
                ..Default::default()
            }),
            mod_rule_criteria: Some(ModRuleCriteria {
                is_enabled: Some(false),
                desired_mod_register: Some(0),
                ..Default::default()
            }),
            host_balancing_rule_prefs: Some(HostBalancingRulePrefs {
                min_fit_threshold_name: Some(b""),
                ..Default::default()
            }),
            player_count_rule_criteria: Some(PlayerCountRuleCriteria {
                is_single_group_match: Some(0),
                max_player_count: Some(p.max_players),
                desired_player_count: Some(p.max_players),
                min_player_count: Some(1),
                range_offset_list_name: Some(&p.player_count_threshold),
                ..Default::default()
            }),
            player_slot_utilization_rule_criteria: Some(PlayerSlotUtilizationRuleCriteria {
                desired_percent_full: Some(p.desired_percent_full),
                max_percent_full: Some(100),
                min_percent_full: Some(0),
                range_offset_list_name: Some(&p.utilization_threshold),
                ..Default::default()
            }),
            preferred_games_rule_criteria: Some(PreferredGamesRuleCriteria {
                require_preferred_game: Some(false),
                ..Default::default()
            }),
            preferred_players_rule_criteria: Some(PreferredPlayersRuleCriteria {
                preferred_list_id: Some(ObjectId(25, 1, g.persona())),
                require_preferred_player: Some(false),
                ..Default::default()
            }),
            ping_site_rule_prefs: Some(PingSiteRulePrefs {
                min_fit_threshold_name: Some(&p.ping_site_threshold),
                ..Default::default()
            }),
            ranked_game_rule_prefs: Some(RankedGameRulePrefs {
                min_fit_threshold_name: Some(b""),
                desired_ranked_game_value: Some(2),
                ..Default::default()
            }),
            reputation_rule_prefs: Some(ReputationRulePrefs {
                reputation_requirement: Some(1),
                ..Default::default()
            }),
            roster_size_rule_prefs: Some(RosterSizeRulePrefs {
                max_player_count: Some(u16::MAX),
                min_player_count: Some(0),
                ..Default::default()
            }),
            team_balance_rule_prefs: Some(TeamBalanceRulePrefs {
                max_team_size_difference_allowed: Some(0),
                range_offset_list_name: Some(b""),
                ..Default::default()
            }),
            team_count_rule_prefs: Some(TeamCountRulePrefs {
                team_count: Some(0),
                ..Default::default()
            }),
            team_composition_rule_prefs: Some(TeamCompositionRulePrefs {
                rule_name: Some(b""),
                min_fit_threshold_name: Some(b""),
                ..Default::default()
            }),
            team_min_size_rule_prefs: Some(TeamMinSizeRulePrefs {
                team_min_size: Some(0),
                range_offset_list_name: Some(b""),
                ..Default::default()
            }),
            total_player_slots_rule_criteria: Some(TotalPlayerSlotsRuleCriteria {
                desired_total_player_slots: Some(1),
                max_total_player_slots: Some(1),
                min_total_player_slots: Some(1),
                range_offset_list_name: Some(b""),
                ..Default::default()
            }),
            team_ued_position_parity_rule_prefs: Some(TeamUEDPositionParityRulePrefs {
                rule_name: Some(b""),
                range_offset_list_name: Some(b""),
                ..Default::default()
            }),
            team_ued_balance_rule_prefs: Some(TeamUEDBalanceRulePrefs {
                rule_name: Some(b""),
                range_offset_list_name: Some(b""),
                ..Default::default()
            }),
            ued_rule_criteria_map: Some(UEDRuleCriteriaMap(vec![(
                &p.ued_rule,
                UEDRuleCriteria {
                    client_ued_search_value: Some(i64::MIN),
                    override_ued_value: Some(i64::MIN),
                    threshold_name: Some(&p.ued_threshold),
                    ..Default::default()
                },
            )])),
            host_viability_rule_prefs: Some(HostViabilityRulePrefs {
                min_fit_threshold_name: Some(b""),
                ..Default::default()
            }),
            virtual_game_rule_prefs: Some(VirtualGameRulePrefs {
                min_fit_threshold_name: Some(b""),
                desired_virtual_game_value: Some(8),
                ..Default::default()
            }),
            ..Default::default()
        }),
        game_creation_data: Some(GameCreationData {
            game_mod_register: Some(0),
            game_name: Some(b""),
            game_settings: Some(p.creation_settings),
            network_topology: Some(0),
            max_player_capacity: Some(0),
            min_player_capacity: Some(1),
            presence_mode: Some(1),
            queue_capacity: Some(0),
            role_information: Some(RoleInformation::default()),
            external_session_template_name: Some(b""),
            voip_network: Some(0),
            ..Default::default()
        }),
        player_join_data: Some(PlayerJoinData {
            group_id: Some(ObjectId(4, 2, g.group() as i64)),
            default_role: Some(&p.default_role),
            game_entry_type: Some(0),
            player_data_list: Some(PlayerJoinList(vec![PerPlayerJoinData {
                is_optional_player: Some(false),
                player_attributes: Some(ConfigEntries(
                    p.player_attributes
                        .iter()
                        .map(|(k, v)| (k.as_slice(), v.as_slice()))
                        .collect(),
                )),
                role: Some(b""),
                user: Some(UserIdentification::decode(g.user(), group::body_limits()).unwrap()),
                ..Default::default()
            }])),
            slot_type: Some(0),
            team_id: Some(65534),
            team_index: Some(65535),
            ..Default::default()
        }),
        session_data: Some(MatchmakingSessionData {
            session_duration: Some(p.duration_micros),
            debug_freeze_decay: Some(false),
            session_mode: Some(1),
            external_mm_session_template_name: Some(b""),
            pseudo_request: Some(false),
            starting_decay_age: Some(age),
            start_delay: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn encode(q: &StartMatchmakingRequest<'_>) -> Vec<u8> {
    q.encode(group::body_limits()).unwrap()
}
#[test]
fn admission_uses_initialized_current_group_and_named_policy() {
    let p = policy();
    let a = context(10, 1);
    let b = context(20, 2);
    for g in [&a, &b] {
        for age in [0, 500] {
            let mut q = query(&p, g, age);
            // Attribute map order is not part of local admission semantics.
            q.player_join_data
                .as_mut()
                .unwrap()
                .player_data_list
                .as_mut()
                .unwrap()
                .0[0]
                .player_attributes
                .as_mut()
                .unwrap()
                .0
                .reverse();
            let admitted = p.admit(g, &encode(&q)).unwrap();
            assert_eq!(
                (
                    admitted.group,
                    admitted.persona,
                    admitted.user_session,
                    admitted.connection
                ),
                (g.group(), g.persona(), g.player_session(), g.connection())
            );
        }
    }
    let body = encode(&query(&p, &a, 0));
    assert!(p.admit(&b, &body).is_err());
    let another = context(10, 3);
    assert!(p.admit(&another, &body).is_err());
}
#[test]
fn admission_rejects_changed_identity_network_criteria_and_presence() {
    type Mutation = for<'a> fn(&mut StartMatchmakingRequest<'a>);
    let mutations: &[Mutation] = &[
        |q| q.common_game_data.as_mut().unwrap().originating_scenario_id = Some(1),
        |q| {
            q.common_game_data
                .as_mut()
                .unwrap()
                .game_protocol_version_string = Some(b"wrong")
        },
        |q| {
            q.common_game_data.as_mut().unwrap().player_network_address =
                Some(NetworkAddress::Unset)
        },
        |q| q.player_join_data.as_mut().unwrap().group_id = Some(ObjectId(4, 2, 99)),
        |q| {
            q.player_join_data
                .as_mut()
                .unwrap()
                .player_data_list
                .as_mut()
                .unwrap()
                .0[0]
                .user
                .as_mut()
                .unwrap()
                .account_id = Some(999)
        },
        |q| {
            q.player_join_data
                .as_mut()
                .unwrap()
                .player_data_list
                .as_mut()
                .unwrap()
                .0[0]
                .is_optional_player = Some(true)
        },
        |q| {
            q.player_join_data
                .as_mut()
                .unwrap()
                .player_data_list
                .as_mut()
                .unwrap()
                .0
                .clear()
        },
        |q| q.game_creation_data.as_mut().unwrap().network_topology = Some(1),
        |q| q.game_creation_data.as_mut().unwrap().game_attribs = Some(ConfigEntries(vec![])),
        |q| {
            q.criteria_data
                .as_mut()
                .unwrap()
                .avoid_games_rule_criteria
                .as_mut()
                .unwrap()
                .game_id_list = Some(GameIds(vec![123]))
        },
        |q| {
            q.criteria_data
                .as_mut()
                .unwrap()
                .player_count_rule_criteria
                .as_mut()
                .unwrap()
                .max_player_count = Some(99)
        },
        |q| {
            q.criteria_data
                .as_mut()
                .unwrap()
                .preferred_players_rule_criteria
                .as_mut()
                .unwrap()
                .preferred_list_id = Some(ObjectId(25, 1, 99))
        },
        |q| {
            q.criteria_data
                .as_mut()
                .unwrap()
                .ued_rule_criteria_map
                .as_mut()
                .unwrap()
                .0[0]
                .1
                .override_ued_value = Some(1)
        },
        |q| {
            q.criteria_data
                .as_mut()
                .unwrap()
                .game_attribute_rule_criteria_map
                .as_mut()
                .unwrap()
                .0[0]
                .1
                .min_fit_threshold_name = Some(b"unknown")
        },
        |q| q.criteria_data.as_mut().unwrap().virtual_game_rule_prefs = None,
        |q| {
            q.criteria_data.as_mut().unwrap().variable_custom_rule_prefs =
                Some(EmptyVariableCustomRulePrefs)
        },
        |q| q.session_data.as_mut().unwrap().starting_decay_age = Some(501),
        |q| q.session_data.as_mut().unwrap().debug_freeze_decay = Some(true),
        |q| q.session_data.as_mut().unwrap().start_delay = None,
    ];
    let p = policy();
    let g = context(10, 1);
    for (i, change) in mutations.iter().enumerate() {
        let mut q = query(&p, &g, 0);
        change(&mut q);
        assert!(p.admit(&g, &encode(&q)).is_err(), "mutation {i}");
    }
}
#[test]
fn admission_rejects_truncation_unknown_fields_and_oversize() {
    let p = policy();
    let g = context(10, 1);
    let body = encode(&query(&p, &g, 0));
    for n in 0..body.len() {
        assert!(p.admit(&g, &body[..n]).is_err(), "prefix {n}");
    }
    let mut unknown = body.clone();
    unknown.extend_from_slice(&[0xfa, 0xfb, 0xfc, 0, 1]);
    assert!(p.admit(&g, &unknown).is_err());
    assert!(p.admit(&g, &vec![0; 16385]).is_err());
    let mut q = query(&p, &g, 0);
    q.player_join_data
        .as_mut()
        .unwrap()
        .player_data_list
        .as_mut()
        .unwrap()
        .0[0]
        .player_attributes
        .as_mut()
        .unwrap()
        .0
        .push((b"mode", b"solo"));
    assert!(q.encode(group::body_limits()).is_err());
}
#[test]
fn admission_configuration_is_bounded_and_has_no_implicit_fields() {
    let value = serde_json::json!({"version":1,"protocol_version":"test-version","game_rules":[{"name":"mode-rule","threshold":"exact","values":["solo"]}],"ued_rule":"skill-rule","ued_threshold":"default","player_attributes":[["mode","solo"]],"default_role":"driver","max_players":4,"player_count_threshold":"count","utilization_threshold":"fill","ping_site_threshold":"ping","desired_percent_full":50,"creation_settings":7,"duration_micros":10000,"starting_decay_ages":[0,500]});
    assert!(Policy::from_json(&serde_json::to_vec(&value).unwrap()).is_ok());
    for (key, bad) in [
        ("max_players", serde_json::json!(65536)),
        ("starting_decay_ages", serde_json::json!([0, 0])),
        ("duration_micros", serde_json::json!(-1)),
        ("game_rules", serde_json::json!([])),
        (
            "player_attributes",
            serde_json::json!([["a", "b"], ["a", "c"]]),
        ),
        ("default_role", serde_json::json!("bad role")),
        ("desired_percent_full", serde_json::json!(101)),
    ] {
        let mut v = value.clone();
        v[key] = bad;
        assert!(
            Policy::from_json(&serde_json::to_vec(&v).unwrap()).is_err(),
            "{key}"
        );
    }
    let mut v = value.clone();
    v.as_object_mut().unwrap().remove("version");
    assert!(Policy::from_json(&serde_json::to_vec(&v).unwrap()).is_err());
    let mut v = value;
    v["unexpected"] = serde_json::json!(0);
    assert!(Policy::from_json(&serde_json::to_vec(&v).unwrap()).is_err());
    assert!(Policy::from_json(&vec![b' '; 16385]).is_err());
}
