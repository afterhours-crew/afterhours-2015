// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::users::{IpAddress, IpPairAddress, NetworkAddress};
fn address(a: &IpAddress<'_>) -> IpAddress<'static> {
    IpAddress {
        ip: a.ip,
        machine_id: a.machine_id,
        port: a.port,
        ..Default::default()
    }
}
fn pair(a: &IpPairAddress<'_>) -> NetworkAddress<'static> {
    NetworkAddress::IpPair(IpPairAddress {
        external_address: a.external_address.as_ref().map(address),
        internal_address: a.internal_address.as_ref().map(address),
        machine_id: a.machine_id,
        ..Default::default()
    })
}
pub(super) fn host(persona: i64, connection: ObjectId, session: u64) -> HostInfo<'static> {
    HostInfo {
        connection_group_id: Some(connection.2 as u64),
        connection_slot_id: Some(0),
        player_id: Some(persona),
        user_session_id: Some(session),
        slot_id: Some(0),
        ..Default::default()
    }
}

impl Config {
    pub(super) fn create(
        &self,
        body: &[u8],
        identity: &Identity,
        generated: &Generated,
    ) -> Result<(Vec<u8>, Vec<u8>), Error> {
        let q = CreateGameRequest::decode(body, body_limits()).map_err(|_| Error::Ineligible)?;
        if q.unknown_field_count() != 0
            || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != body
        {
            return Err(Error::Ineligible);
        }
        let mut current =
            UserIdentification::decode(&identity.user, body_limits()).map_err(|_| Error::Encode)?;
        current.origin_persona_id = Some(0); // Supported create-query convention.
        let join = q.player_join_data.as_ref().ok_or(Error::Ineligible)?;
        let players = join.player_data_list.as_ref().ok_or(Error::Ineligible)?;
        if join.group_id != Some(identity.connection)
            || players.0.len() != 1
            || players.0[0]
                .user
                .as_ref()
                .ok_or(Error::Ineligible)?
                .encode(body_limits())
                .map_err(|_| Error::Ineligible)?
                != current.encode(body_limits()).map_err(|_| Error::Encode)?
        {
            return Err(Error::Ineligible);
        }
        self.validate_request(&q)?;
        let common = q.common_game_data.ok_or(Error::Ineligible)?;
        let creation = q.game_creation_data.ok_or(Error::Ineligible)?;
        let join = q.player_join_data.ok_or(Error::Ineligible)?;
        let players = join.player_data_list.ok_or(Error::Ineligible)?;
        let NetworkAddress::IpPair(net) = common.player_network_address.ok_or(Error::Ineligible)?
        else {
            return Err(Error::Ineligible);
        };
        let mut s = self.empty_setup(creation.max_player_capacity.ok_or(Error::Ineligible)?);
        let g = s.game_data.as_mut().ok_or(Error::Encode)?;
        let p = &mut s.game_roster.as_mut().ok_or(Error::Encode)?.0[0];
        let persona = current.blaze_id.ok_or(Error::Encode)?;
        g.admin_player_list = Some(PlayerIds(vec![persona]));
        g.create_time = Some(generated.create_time);
        g.external_session_name = Some(&generated.external_session_name);
        g.game_id = Some(generated.game_id);
        g.game_reporting_id = Some(generated.reporting_id);
        g.shared_seed = Some(generated.seed);
        g.uuid = Some(&generated.game_uuid);
        g.platform_host_info = Some(host(
            persona,
            identity.connection,
            generated.player_session_id,
        ));
        g.topology_host_info = Some(host(
            persona,
            identity.connection,
            generated.player_session_id,
        ));
        g.topology_host_network_address_list = Some(NetworkAddressList(vec![pair(&net)]));
        let mut qos =
            NetworkQosData::decode(&identity.qos, body_limits()).map_err(|_| Error::Encode)?;
        qos.bandwidth_error_code = Some(0);
        qos.nat_error_code = Some(0);
        g.network_qos_data = Some(qos);
        g.game_attribs = creation.game_attribs;
        g.slot_capacities = q.slot_capacities;
        g.game_type = common.game_type;
        g.game_mod_register = creation.game_mod_register;
        g.game_name = creation.game_name;
        g.game_settings = creation.game_settings;
        g.network_topology = creation.network_topology;
        g.presence_mode = creation.presence_mode;
        g.queue_capacity = creation.queue_capacity;
        g.external_session_template_name = creation.external_session_template_name;
        g.voip_network = creation.voip_network;
        g.max_player_capacity = creation.max_player_capacity;
        g.min_player_capacity = creation.min_player_capacity;
        g.game_report_name = q.game_report_name;
        g.game_status_url = q.game_status_url;
        g.server_not_resetable = q.server_not_resetable;
        g.persisted_game_id = q.persisted_game_id;
        g.persisted_game_id_secret = q.persisted_game_id_secret;
        g.team_ids = q.team_ids;
        g.game_protocol_version_string = common.game_protocol_version_string;
        p.connection_group_id = Some(identity.connection.2 as u64);
        p.external_blob = current.external_blob;
        p.external_id = current.external_id;
        p.game_id = Some(generated.game_id);
        p.account_locale = current.account_locale;
        p.player_name = current.name;
        p.persona_namespace = current.persona_namespace;
        p.player_id = Some(persona);
        p.network_address = Some(pair(&net));
        p.role_name = players.0[0].role;
        p.joined_game_timestamp = Some(generated.join_time);
        p.user_group_id = Some(identity.connection);
        p.player_session_id = Some(generated.player_session_id);
        p.uuid = Some(&generated.player_uuid);
        let reply = CreateGameResponse {
            game_id: Some(generated.game_id),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        let setup = s.encode(body_limits()).map_err(|_| Error::Encode)?;
        Ok((reply, setup))
    }
}
