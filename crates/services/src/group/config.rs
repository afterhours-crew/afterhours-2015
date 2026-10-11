// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::{Blob, users::NetworkAddress};

/// Static deployment/build policy; contains no player, group or response data.
#[derive(Clone)]
pub struct Config {
    pub protocol_version: Vec<u8>,
    pub protocol_hash: u64,
    pub ping_site: Vec<u8>,
    pub mode_attribute: Vec<u8>,
    pub telemetry_interval: i64,
}
impl Config {
    pub fn validate(&self) -> Result<(), Error> {
        if [
            &self.protocol_version,
            &self.ping_site,
            &self.mode_attribute,
        ]
        .iter()
        .any(|s| s.is_empty() || s.len() > 64 || !s.iter().all(u8::is_ascii_graphic))
            || self.telemetry_interval <= 0
            || self.telemetry_interval > 600_000_000
        {
            return Err(Error::Config);
        }
        Ok(())
    }
    pub fn from_json(v: &serde_json::Value) -> Result<Self, crate::ContentError> {
        use crate::ContentError::Invalid;
        let keys = [
            "format",
            "version",
            "build_sha256",
            "protocol_version",
            "protocol_hash",
            "ping_site",
            "mode_attribute",
            "telemetry_interval",
        ];
        let o = v.as_object().ok_or(Invalid)?;
        if o.len() != keys.len()
            || !keys.iter().all(|k| o.contains_key(*k))
            || v["format"] != "nfs-group-policy"
            || v["version"] != 1
            || v["build_sha256"] != crate::SUPPORTED_BUILD_SHA256
        {
            return Err(Invalid);
        }
        let text = |key: &str| -> Result<Vec<u8>, crate::ContentError> {
            Ok(v[key].as_str().ok_or(Invalid)?.as_bytes().to_vec())
        };
        let config = Self {
            protocol_version: text("protocol_version")?,
            protocol_hash: v["protocol_hash"].as_u64().ok_or(Invalid)?,
            ping_site: text("ping_site")?,
            mode_attribute: text("mode_attribute")?,
            telemetry_interval: v["telemetry_interval"].as_i64().ok_or(Invalid)?,
        };
        config.validate().map_err(|_| Invalid)?;
        Ok(config)
    }
    pub fn load(path: &std::path::Path) -> Result<Self, crate::ContentError> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| crate::ContentError::Io)?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| crate::ContentError::Io)?;
        if bytes.len() > 4096 {
            return Err(crate::ContentError::TooLarge);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| crate::ContentError::Invalid)?)
    }
    pub(super) fn validate_request(&self, q: &CreateGameRequest<'_>) -> Result<(), Error> {
        let c = q.common_game_data.as_ref().ok_or(Error::Ineligible)?;
        let g = q.game_creation_data.as_ref().ok_or(Error::Ineligible)?;
        let j = q.player_join_data.as_ref().ok_or(Error::Ineligible)?;
        let s = c.scenario_info.as_ref().ok_or(Error::Ineligible)?;
        let roles = g.role_information.as_ref().ok_or(Error::Ineligible)?;
        let players = &j.player_data_list.as_ref().ok_or(Error::Ineligible)?.0;
        let attrs = &g.game_attribs.as_ref().ok_or(Error::Ineligible)?.0;
        let value = |key: &[u8]| attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        let privacy = match value(b"Type") {
            Some(b"Public") => 1_082_397,
            Some(b"Private") => 1_082_384,
            _ => return Err(Error::Ineligible),
        };
        let Some(NetworkAddress::IpPair(pair)) = &c.player_network_address else {
            return Err(Error::Ineligible);
        };
        if [
            pair.external_address.as_ref(),
            pair.internal_address.as_ref(),
        ]
        .iter()
        .any(|a| a.is_none_or(|a| a.ip.is_none() || a.port.is_none() || a.machine_id.is_none()))
            || pair.machine_id.is_none()
            || q.admin_player_list.is_some()
            || q.mesh_attribs.is_some()
            || q.game_ping_site_alias != Some(b"")
            || q.game_report_name != Some(b"")
            || q.game_status_url != Some(b"")
            || q.server_not_resetable != Some(false)
            || q.persisted_game_id != Some(b"")
            || q.persisted_game_id_secret
                .as_ref()
                .is_none_or(|b| !b.0.is_empty())
            || q.slot_capacities
                .as_ref()
                .is_none_or(|v| v.0 != [8, 0, 0, 0])
            || q.team_ids.as_ref().is_none_or(|v| v.0 != [65534])
            || c.game_type != Some(1)
            || c.game_protocol_version_string != Some(self.protocol_version.as_slice())
            || c.originating_scenario_id != Some(0)
            || s.scenario_name != Some(b"")
            || s.scenario_version != Some(0)
            || s.scenario_variant != Some(0)
            || s.sub_session_name != Some(b"")
            || !matches!(c.x_lspnetwork_address, Some(NetworkAddress::Unset))
            || attrs.len() != 4
            || value(b"gameSessionId") != Some(b"0")
            || value(b"Matchmaking") != Some(b"Idle")
            || value(b"mode") != Some(b"Group")
            || g.entry_criteria_map.is_some()
            || g.game_mod_register != Some(0)
            || g.game_name != Some(b"")
            || g.game_settings != Some(privacy)
            || g.network_topology != Some(255)
            || g.max_player_capacity != Some(8)
            || g.min_player_capacity != Some(1)
            || g.presence_mode != Some(1)
            || g.queue_capacity != Some(0)
            || g.external_session_template_name != Some(b"")
            || g.voip_network != Some(2)
            || roles.role_criteria_map.is_some()
            || roles.multi_role_criteria.is_some()
            || j.default_role != Some(b"")
            || j.game_entry_type != Some(0)
            || j.slot_type != Some(0)
            || j.team_id != Some(65534)
            || j.team_index != Some(0)
            || players.len() != 1
            || players[0].is_optional_player != Some(false)
            || players[0].player_attributes.is_some()
            || players[0].role != Some(b"")
        {
            return Err(Error::Ineligible);
        }
        Ok(())
    }

    /// Default empty membership records; request and authenticated/generated
    /// state fill every identity and network field before encoding.
    pub(super) fn empty_setup(&self, capacity: u16) -> NotifyGameSetup<'_> {
        NotifyGameSetup {
            game_data: Some(ReplicatedGameData {
                owns_first_party_presence: Some(true),
                external_session_correlation_id: Some(b""),
                dedicated_server_host_info: Some(super::setup::host(0, ObjectId(0, 0, 0), 0)),
                game_protocol_version_hash: Some(self.protocol_hash),
                game_state: Some(1),
                np_session_id: Some(b""),
                ping_site_alias: Some(&self.ping_site),
                role_information: Some(RoleInformation {
                    role_criteria_map: Some(RoleCriteriaMap(vec![(
                        b"",
                        RoleCriteria {
                            role_capacity: Some(capacity),
                            ..Default::default()
                        },
                    )])),
                    ..Default::default()
                }),
                scid: Some(b""),
                xnet_nonce: Some(Blob(b"")),
                xnet_session: Some(Blob(b"")),
                ..Default::default()
            }),
            is_lockable_for_preferred_joins: Some(false),
            game_mode_attribute_name: Some(&self.mode_attribute),
            game_roster: Some(PlayerRoster(vec![ReplicatedGamePlayer {
                custom_data: Some(Blob(b"")),
                connection_slot_id: Some(0),
                dirty_sock_user_index: Some(0),
                has_join_first_party_game_session_permission: Some(true),
                joined_via_matchmaking: Some(false),
                player_settings: Some(1),
                reservation_creation_timestamp: Some(0),
                slot_id: Some(0),
                slot_type: Some(0),
                player_state: Some(4),
                team_index: Some(0),
                ..Default::default()
            }])),
            qos_settings: Some(QosSettings {
                duration_ms: Some(0),
                interval_ms: Some(0),
                packet_size: Some(0),
                ..Default::default()
            }),
            perform_qos_validation: Some(false),
            game_setup_reason: Some(GameSetupReason::Dataless(DatalessSetupContext {
                setup_context: Some(0),
                ..Default::default()
            })),
            qos_telemetry_interval: Some(self.telemetry_interval),
            ..Default::default()
        }
    }
}
pub(super) fn check_mesh(q: UpdateMeshConnectionRequest<'_>) -> Result<(), Error> {
    if q.player_net_connection_flags != Some(0)
        || q.player_net_connection_status != Some(2)
        || q.qos_info
            .as_ref()
            .is_none_or(|q| q.packet_loss != Some(PacketLossBits(0)) || q.latency_ms != Some(0))
    {
        return Err(Error::Ineligible);
    }
    Ok(())
}
