// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed local world setup. Deployment policy, accepted player state, fresh
//! allocation and match measurements are separate inputs; no recorded body.
use crate::bootstrap::{body_limits, frame_limits};
use nfs_protocol::{Blob, gamemanager::*, users::*, util::ConfigEntries};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
};
mod allocation;
pub use allocation::{Generated, SEED_BYTES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Config,
    Context,
    Encode,
    Pending,
    Closed,
}

/// Owns one generated setup until the transport confirms the complete write.
/// Later requests cannot acquire a published world after a partial write.
pub struct Publication {
    prepared: Option<Vec<u8>>,
    pending: bool,
    published: bool,
    closed: bool,
}
impl Publication {
    pub fn new(
        config: &Config,
        current: &Current<'_>,
        allocation: &Allocation<'_>,
        metrics: MatchMetrics,
    ) -> Result<Self, Error> {
        Ok(Self {
            prepared: Some(config.setup(current, allocation, metrics)?),
            pending: false,
            published: false,
            closed: false,
        })
    }
    pub fn emit(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.pending {
            self.abort_write();
            return Err(Error::Pending);
        }
        if self.published {
            return Ok(None);
        }
        let output = self.prepared.take().ok_or(Error::Closed)?;
        self.pending = true;
        Ok(Some(output))
    }
    pub fn commit_after_write(&mut self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.pending {
            self.pending = false;
            self.published = true;
        }
        Ok(())
    }
    pub fn published(&self) -> bool {
        self.published && !self.closed
    }
    pub fn abort_write(&mut self) {
        self.prepared = None;
        self.pending = false;
        self.closed = true;
    }
}
#[derive(Clone)]
pub struct Config {
    protocol_version: String,
    protocol_hash: u64,
    ping_site: String,
    mode_attribute: String,
    telemetry_interval: i64,
    game_attributes: Vec<(String, String)>,
    player_attributes: Vec<(String, String)>,
    player_role: String,
    game_settings: u32,
    max_players: u16,
    min_players: u16,
    capacities: Vec<u16>,
    teams: Vec<u16>,
    roles: BTreeMap<String, u16>,
    multi_role_criteria: Vec<(String, String)>,
    report_name: String,
    max_fit: u32,
}
impl Config {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        use serde_json::Value;
        if bytes.len() > 16384 {
            return Err(Error::Config);
        }
        let v: Value = serde_json::from_slice(bytes).map_err(|_| Error::Config)?;
        let keys = [
            "version",
            "protocol_version",
            "protocol_hash",
            "ping_site",
            "mode_attribute",
            "telemetry_interval",
            "game_attributes",
            "player_attributes",
            "player_role",
            "game_settings",
            "max_players",
            "min_players",
            "slot_capacities",
            "teams",
            "roles",
            "multi_role_criteria",
            "report_name",
            "max_fit",
        ];
        let o = v.as_object().ok_or(Error::Config)?;
        if o.len() != keys.len() || !keys.iter().all(|k| o.contains_key(*k)) || v["version"] != 1 {
            return Err(Error::Config);
        }
        fn text(v: &Value) -> Result<String, Error> {
            let s = v.as_str().ok_or(Error::Config)?;
            if s.is_empty()
                || s.len() > 128
                || !s.bytes().all(|b| b == b' ' || b.is_ascii_graphic())
            {
                return Err(Error::Config);
            }
            Ok(s.to_owned())
        }
        fn map(v: &Value) -> Result<Vec<(String, String)>, Error> {
            let a = v.as_array().ok_or(Error::Config)?;
            if a.is_empty() || a.len() > 16 {
                return Err(Error::Config);
            }
            let mut entries: Vec<(String, String)> = Vec::new();
            for item in a {
                let pair = item
                    .as_array()
                    .filter(|p| p.len() == 2)
                    .ok_or(Error::Config)?;
                let key = text(&pair[0])?;
                if entries.iter().any(|(k, _)| k == &key) {
                    return Err(Error::Config);
                }
                entries.push((key, text(&pair[1])?));
            }
            Ok(entries)
        }
        fn list(v: &Value, max: usize) -> Result<Vec<u16>, Error> {
            let a = v.as_array().ok_or(Error::Config)?;
            if a.is_empty() || a.len() > max {
                return Err(Error::Config);
            }
            a.iter()
                .map(|v| u16::try_from(v.as_u64().ok_or(Error::Config)?).map_err(|_| Error::Config))
                .collect()
        }
        let u = |k: &str| v[k].as_u64().ok_or(Error::Config);
        let roles = v["roles"].as_object().ok_or(Error::Config)?;
        if roles.is_empty() || roles.len() > 8 {
            return Err(Error::Config);
        }
        let c = Self {
            protocol_version: text(&v["protocol_version"])?,
            protocol_hash: u("protocol_hash")?,
            ping_site: text(&v["ping_site"])?,
            mode_attribute: text(&v["mode_attribute"])?,
            telemetry_interval: v["telemetry_interval"].as_i64().ok_or(Error::Config)?,
            game_attributes: map(&v["game_attributes"])?,
            player_attributes: map(&v["player_attributes"])?,
            player_role: text(&v["player_role"])?,
            game_settings: u32::try_from(u("game_settings")?).map_err(|_| Error::Config)?,
            max_players: u16::try_from(u("max_players")?).map_err(|_| Error::Config)?,
            min_players: u16::try_from(u("min_players")?).map_err(|_| Error::Config)?,
            capacities: list(&v["slot_capacities"], 4)?,
            teams: list(&v["teams"], 16)?,
            roles: roles
                .iter()
                .map(|(k, v)| {
                    Ok((
                        text(&Value::String(k.clone()))?,
                        u16::try_from(v.as_u64().ok_or(Error::Config)?)
                            .map_err(|_| Error::Config)?,
                    ))
                })
                .collect::<Result<_, Error>>()?,
            multi_role_criteria: map(&v["multi_role_criteria"])?,
            report_name: text(&v["report_name"])?,
            max_fit: u32::try_from(u("max_fit")?).map_err(|_| Error::Config)?,
        };
        if c.telemetry_interval <= 0
            || c.telemetry_interval > 600_000_000
            || c.max_players == 0
            || c.max_players > 64
            || c.min_players == 0
            || c.min_players > c.max_players
            || c.capacities.len() != 4
            || c.capacities.iter().map(|x| u32::from(*x)).sum::<u32>() != u32::from(c.max_players)
            || c.roles.values().any(|v| *v == 0 || *v > c.max_players)
            || !c.roles.contains_key(&c.player_role)
            || !c
                .game_attributes
                .iter()
                .any(|(k, _)| k == &c.mode_attribute)
            || c.max_fit == 0
        {
            return Err(Error::Config);
        }
        Ok(c)
    }
    pub fn load(path: &std::path::Path) -> Result<Self, Error> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| Error::Config)?
            .take(16385)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Config)?;
        Self::from_slice(&bytes)
    }
    pub fn max_fit(&self) -> u32 {
        self.max_fit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> serde_json::Value {
        serde_json::json!({"version":1,"protocol_version":"local-v1","protocol_hash":7,"ping_site":"local","mode_attribute":"mode","telemetry_interval":1000000,"game_attributes":[["mode","practice"]],"player_attributes":[["mode","practice"]],"player_role":"driver","game_settings":1,"max_players":4,"min_players":1,"slot_capacities":[4,0,0,0],"teams":[0],"roles":{"driver":4},"multi_role_criteria":[["single","driver"]],"report_name":"local","max_fit":100})
    }
    fn config() -> Config {
        Config::from_slice(&serde_json::to_vec(&policy()).unwrap()).unwrap()
    }
    const GROUP_UUID: &[u8] = b"00000000-0000-4000-8000-000000000001";
    const PLAYER_UUID: &[u8] = b"00000000-0000-4000-8000-000000000002";
    const WORLD_UUID: &[u8] = b"00000000-0000-4000-8000-000000000003";
    fn player() -> Vec<u8> {
        ReplicatedGamePlayer {
            connection_group_id: Some(15),
            external_blob: Some(Blob(b"")),
            external_id: Some(0),
            game_id: Some(11),
            account_locale: Some(1),
            player_name: Some(b"player"),
            persona_namespace: Some(b"local"),
            player_id: Some(13),
            network_address: Some(network("127.0.0.1:9001".parse().unwrap(), 15)),
            player_session_id: Some(14),
            uuid: Some(PLAYER_UUID),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap()
    }
    fn current(player: &[u8]) -> Current<'_> {
        Current {
            group: 11,
            matchmaking: 12,
            persona: 13,
            user_session: 14,
            connection: 15,
            scenario: 16,
            player,
            group_uuid: GROUP_UUID,
        }
    }
    fn allocation() -> Allocation<'static> {
        Allocation {
            game: 21,
            reporting: 22,
            host_persona: 23,
            host_session: 24,
            host_connection: 25,
            shared_seed: 26,
            clock: 1000000,
            endpoint: "127.0.0.1:9002".parse().unwrap(),
            uuid: WORLD_UUID,
            name: b"local-world",
            external_name: b"local-world-session",
        }
    }
    fn metrics() -> MatchMetrics {
        MatchMetrics {
            fit: 80,
            elapsed_micros: 40,
        }
    }
    #[test]
    fn world_identity_roster_host_and_measurements_come_from_owned_inputs() {
        let p = player();
        let bytes = config()
            .setup(&current(&p), &allocation(), metrics())
            .unwrap();
        let d = nfs_fire2::decode(&bytes, frame_limits()).unwrap().unwrap();
        assert_eq!(d.consumed, bytes.len());
        assert_eq!(
            (
                d.frame.fields.routing_a,
                d.frame.fields.routing_b,
                d.frame.fields.category
            ),
            (4, 20, 2)
        );
        let s = NotifyGameSetup::decode(d.frame.body, body_limits()).unwrap();
        assert_eq!(s.unknown_field_count(), 0);
        let g = s.game_data.as_ref().unwrap();
        let roster = s.game_roster.as_ref().unwrap();
        assert_eq!(roster.0.len(), 1);
        let p = &roster.0[0];
        assert_eq!(
            (g.game_id, p.game_id, p.player_id, p.player_session_id),
            (Some(21), Some(21), Some(13), Some(14))
        );
        assert_eq!(p.uuid, Some(PLAYER_UUID));
        assert_eq!(g.uuid, Some(WORLD_UUID));
        assert_eq!(g.admin_player_list.as_ref().unwrap().0, vec![23]);
        let platform = g.platform_host_info.as_ref().unwrap();
        assert_eq!(
            (
                platform.player_id,
                platform.user_session_id,
                platform.connection_group_id
            ),
            (Some(13), Some(24), Some(15))
        );
        assert_eq!(
            g.dedicated_server_host_info
                .as_ref()
                .unwrap()
                .user_session_id,
            platform.user_session_id
        );
        let Some(GameSetupReason::Matchmaking(m)) = s.game_setup_reason else {
            panic!()
        };
        assert_eq!(
            (
                m.session_id,
                m.user_session_id,
                m.scenario_id,
                m.fit_score,
                m.max_possible_fit_score,
                m.time_to_match
            ),
            (Some(12), Some(14), Some(16), Some(80), Some(100), Some(40))
        );
    }
    #[test]
    fn setup_publication_requires_complete_write_and_retries_are_empty() {
        let p = player();
        let mut s = Publication::new(&config(), &current(&p), &allocation(), metrics()).unwrap();
        assert!(!s.published());
        assert!(s.emit().unwrap().is_some());
        assert!(!s.published());
        s.commit_after_write().unwrap();
        assert!(s.published());
        assert!(s.emit().unwrap().is_none());
    }
    #[test]
    fn failed_or_premature_write_cannot_publish() {
        for retry in [true, false] {
            let p = player();
            let mut s =
                Publication::new(&config(), &current(&p), &allocation(), metrics()).unwrap();
            s.emit().unwrap();
            if retry {
                assert_eq!(s.emit(), Err(Error::Pending));
            } else {
                s.abort_write();
            }
            assert!(!s.published());
            assert_eq!(s.commit_after_write(), Err(Error::Closed));
        }
    }
    #[test]
    fn allocation_owner_and_record_guards() {
        let p = player();
        let c = config();
        for kind in 0..7 {
            let mut a = allocation();
            let mut owner = current(&p);
            let mut m = metrics();
            match kind {
                0 => a.game = owner.group,
                1 => a.host_session = a.host_connection,
                2 => a.endpoint = "192.0.2.1:9002".parse().unwrap(),
                3 => a.uuid = PLAYER_UUID,
                4 => owner.persona = 99,
                5 => m.fit = 101,
                _ => m.elapsed_micros = -1,
            };
            assert_eq!(c.setup(&owner, &a, m), Err(Error::Context));
        }
        for end in 0..p.len() {
            assert!(
                c.setup(&current(&p[..end]), &allocation(), metrics())
                    .is_err()
            );
        }
    }
    #[test]
    fn configured_map_order_is_preserved_and_duplicate_keys_are_rejected() {
        let mut policy = policy();
        policy["game_attributes"] =
            serde_json::json!([["z", "last"], ["mode", "practice"], ["a", "first"]]);
        let config = Config::from_slice(&serde_json::to_vec(&policy).unwrap()).unwrap();
        let p = player();
        let wire = config
            .setup(&current(&p), &allocation(), metrics())
            .unwrap();
        let f = nfs_fire2::decode(&wire, frame_limits())
            .unwrap()
            .unwrap()
            .frame;
        let setup = NotifyGameSetup::decode(f.body, body_limits()).unwrap();
        assert_eq!(
            setup
                .game_data
                .unwrap()
                .game_attribs
                .unwrap()
                .0
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>(),
            vec![b"z".as_slice(), b"mode", b"a"]
        );
        policy["game_attributes"] = serde_json::json!([["mode", "practice"], ["mode", "other"]]);
        assert!(Config::from_slice(&serde_json::to_vec(&policy).unwrap()).is_err());
    }
    #[test]
    fn config_rejects_unbounded_or_inconsistent_policy() {
        for (k, v) in [
            ("max_players", serde_json::json!(65)),
            ("slot_capacities", serde_json::json!([3, 0, 0, 0])),
            ("roles", serde_json::json!({"driver":5})),
            ("player_role", serde_json::json!("missing")),
            ("mode_attribute", serde_json::json!("missing")),
            ("telemetry_interval", serde_json::json!(0)),
            ("extra", serde_json::json!(0)),
        ] {
            let mut p = policy();
            p[k] = v;
            assert!(
                Config::from_slice(&serde_json::to_vec(&p).unwrap()).is_err(),
                "{k}"
            );
        }
        assert!(Config::from_slice(&vec![b' '; 16385]).is_err());
    }
}
/// Current player record must originate in the owning initialized group.
pub struct Current<'a> {
    pub group: u64,
    pub matchmaking: u64,
    pub persona: i64,
    pub user_session: u64,
    pub connection: u64,
    pub scenario: u64,
    pub player: &'a [u8],
    pub group_uuid: &'a [u8],
}
/// The edge owns ID/entropy uniqueness and a bound loopback endpoint's lifetime.
pub struct Allocation<'a> {
    pub game: u64,
    pub reporting: u64,
    pub host_persona: i64,
    pub host_session: u64,
    pub host_connection: u64,
    pub shared_seed: u32,
    pub clock: i64,
    pub endpoint: SocketAddr,
    pub uuid: &'a [u8],
    pub name: &'a [u8],
    pub external_name: &'a [u8],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchMetrics {
    pub fit: u32,
    pub elapsed_micros: i64,
}
fn host(persona: i64, session: u64, connection: u64, slot: u8) -> HostInfo<'static> {
    HostInfo {
        player_id: Some(persona),
        user_session_id: Some(session),
        connection_group_id: Some(connection),
        connection_slot_id: Some(slot),
        slot_id: Some(slot),
        ..Default::default()
    }
}
fn network(endpoint: SocketAddr, connection: u64) -> NetworkAddress<'static> {
    let ip = u32::from_be_bytes(Ipv4Addr::LOCALHOST.octets());
    let address = || IpAddress {
        ip: Some(ip),
        port: Some(endpoint.port()),
        machine_id: Some(0),
        ..Default::default()
    };
    NetworkAddress::IpPair(IpPairAddress {
        external_address: Some(address()),
        internal_address: Some(address()),
        machine_id: Some(connection),
        ..Default::default()
    })
}
fn entries(map: &[(String, String)]) -> ConfigEntries<'_> {
    ConfigEntries(
        map.iter()
            .map(|(k, v)| (k.as_bytes(), v.as_bytes()))
            .collect(),
    )
}
fn uuid(bytes: &[u8]) -> bool {
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
impl Config {
    pub fn setup(
        &self,
        c: &Current<'_>,
        a: &Allocation<'_>,
        m: MatchMetrics,
    ) -> Result<Vec<u8>, Error> {
        a.validate(c)?;
        if m.fit > self.max_fit || m.elapsed_micros < 0 {
            return Err(Error::Context);
        }
        let mut p =
            ReplicatedGamePlayer::decode(c.player, body_limits()).map_err(|_| Error::Context)?;
        if p.unknown_field_count() != 0
            || p.encode(body_limits()).map_err(|_| Error::Context)? != c.player
            || p.game_id != Some(c.group)
            || p.player_id != Some(c.persona)
            || p.player_session_id != Some(c.user_session)
            || p.connection_group_id != Some(c.connection)
            || p.uuid.is_none_or(|u| !uuid(u) || u == a.uuid)
            || p.external_id.is_none()
            || p.external_blob.as_ref().is_none_or(|b| b.0.len() > 1024)
            || p.account_locale.is_none()
            || [p.player_name, p.persona_namespace]
                .iter()
                .any(|s| s.is_none_or(|v| v.is_empty() || v.len() > 64))
        {
            return Err(Error::Context);
        }
        let NetworkAddress::IpPair(pair) = p.network_address.as_ref().ok_or(Error::Context)? else {
            return Err(Error::Context);
        };
        if pair.machine_id.is_none()
            || [
                pair.external_address.as_ref(),
                pair.internal_address.as_ref(),
            ]
            .iter()
            .any(|a| a.is_none_or(|v| v.ip.is_none() || v.port.is_none() || v.machine_id.is_none()))
        {
            return Err(Error::Context);
        }
        let player = ReplicatedGamePlayer {
            custom_data: Some(Blob(b"")),
            connection_group_id: Some(c.connection),
            connection_slot_id: Some(1),
            dirty_sock_user_index: Some(0),
            external_blob: p.external_blob,
            external_id: p.external_id,
            game_id: Some(a.game),
            has_join_first_party_game_session_permission: Some(true),
            joined_via_matchmaking: Some(true),
            account_locale: p.account_locale,
            player_name: p.player_name,
            persona_namespace: p.persona_namespace,
            player_attribs: Some(entries(&self.player_attributes)),
            player_id: Some(c.persona),
            network_address: p.network_address.take(),
            player_settings: Some(1),
            reservation_creation_timestamp: Some(0),
            role_name: Some(self.player_role.as_bytes()),
            slot_id: Some(1),
            slot_type: Some(0),
            player_state: Some(2),
            team_index: Some(0),
            joined_game_timestamp: Some(a.clock),
            user_group_id: Some(ObjectId(0, 0, 0)),
            player_session_id: Some(c.user_session),
            uuid: p.uuid,
            ..Default::default()
        };
        let game = ReplicatedGameData {
            admin_player_list: Some(PlayerIds(vec![a.host_persona])),
            owns_first_party_presence: Some(false),
            game_attribs: Some(entries(&self.game_attributes)),
            slot_capacities: Some(CapacityList(self.capacities.clone())),
            external_session_correlation_id: Some(b""),
            create_time: Some(a.clock),
            dedicated_server_host_info: Some(host(
                a.host_persona,
                a.host_session,
                a.host_connection,
                0,
            )),
            dedicated_server_host_network_address_list: Some(NetworkAddressList(vec![network(
                a.endpoint,
                a.host_connection,
            )])),
            external_session_name: Some(a.external_name),
            game_type: Some(0),
            game_id: Some(a.game),
            game_mod_register: Some(0),
            game_name: Some(a.name),
            game_protocol_version_hash: Some(self.protocol_hash),
            game_settings: Some(self.game_settings),
            game_reporting_id: Some(a.reporting),
            game_state: Some(131),
            game_report_name: Some(self.report_name.as_bytes()),
            game_status_url: Some(b""),
            topology_host_network_address_list: Some(NetworkAddressList(vec![network(
                a.endpoint,
                a.host_connection,
            )])),
            max_player_capacity: Some(self.max_players),
            min_player_capacity: Some(self.min_players),
            np_session_id: Some(b""),
            network_qos_data: Some(NetworkQosData {
                bandwidth_error_code: Some(0),
                downstream_bits_per_second: Some(0),
                nat_error_code: Some(0),
                nat_type: Some(0),
                upstream_bits_per_second: Some(0),
                ..Default::default()
            }),
            server_not_resetable: Some(true),
            network_topology: Some(1),
            persisted_game_id: Some(b""),
            persisted_game_id_secret: Some(Blob(b"")),
            platform_host_info: Some(host(c.persona, a.host_session, c.connection, 1)),
            presence_mode: Some(0),
            ping_site_alias: Some(self.ping_site.as_bytes()),
            queue_capacity: Some(0),
            role_information: Some(RoleInformation {
                role_criteria_map: Some(RoleCriteriaMap(
                    self.roles
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.as_bytes(),
                                RoleCriteria {
                                    role_capacity: Some(*v),
                                    ..Default::default()
                                },
                            )
                        })
                        .collect(),
                )),
                multi_role_criteria: Some(entries(&self.multi_role_criteria)),
                ..Default::default()
            }),
            scid: Some(b""),
            shared_seed: Some(a.shared_seed),
            external_session_template_name: Some(b""),
            topology_host_info: Some(host(a.host_persona, a.host_session, a.host_connection, 0)),
            team_ids: Some(CapacityList(self.teams.clone())),
            uuid: Some(a.uuid),
            voip_network: Some(1),
            game_protocol_version_string: Some(self.protocol_version.as_bytes()),
            xnet_nonce: Some(Blob(b"")),
            xnet_session: Some(Blob(b"")),
            ..Default::default()
        };
        let body = NotifyGameSetup {
            game_data: Some(game),
            is_lockable_for_preferred_joins: Some(false),
            game_mode_attribute_name: Some(self.mode_attribute.as_bytes()),
            game_roster: Some(PlayerRoster(vec![player])),
            qos_settings: Some(QosSettings {
                duration_ms: Some(0),
                interval_ms: Some(0),
                packet_size: Some(0),
                ..Default::default()
            }),
            perform_qos_validation: Some(false),
            game_setup_reason: Some(GameSetupReason::Matchmaking(MatchmakingSetupContext {
                fit_score: Some(m.fit),
                game_entry_type: Some(0),
                max_possible_fit_score: Some(self.max_fit),
                scenario_id: Some(c.scenario),
                session_id: Some(c.matchmaking),
                matchmaking_result: Some(2),
                time_to_match: Some(m.elapsed_micros),
                user_session_id: Some(c.user_session),
                ..Default::default()
            })),
            qos_telemetry_interval: Some(self.telemetry_interval),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        nfs_fire2::encode(
            nfs_fire2::Frame {
                fields: nfs_fire2::Fields {
                    routing_a: 4,
                    routing_b: 20,
                    category: 2,
                    ..Default::default()
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .map_err(|_| Error::Encode)
    }
}

impl Allocation<'_> {
    pub fn validate(&self, c: &Current<'_>) -> Result<(), Error> {
        let a = self;
        let ids = [
            a.game,
            a.reporting,
            a.host_persona as u64,
            a.host_session,
            a.host_connection,
        ];
        let current = [
            c.group,
            c.matchmaking,
            c.persona as u64,
            c.user_session,
            c.connection,
        ];
        if current.iter().any(|v| *v == 0 || *v > i64::MAX as u64)
            || c.group == c.matchmaking
            || ids
                .iter()
                .any(|v| *v == 0 || *v > i64::MAX as u64 || current.contains(v))
            || ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id))
            || a.shared_seed == 0
            || a.clock <= 0
            || a.endpoint.ip() != std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)
            || a.endpoint.port() == 0
            || !uuid(a.uuid)
            || !uuid(c.group_uuid)
            || a.uuid == c.group_uuid
            || [a.name, a.external_name]
                .iter()
                .any(|s| s.is_empty() || s.len() > 64 || !s.iter().all(u8::is_ascii_graphic))
        {
            return Err(Error::Context);
        }

        let player =
            ReplicatedGamePlayer::decode(c.player, body_limits()).map_err(|_| Error::Context)?;
        if player.uuid.is_none_or(|u| !uuid(u) || u == a.uuid) {
            return Err(Error::Context);
        }
        Ok(())
    }
}
