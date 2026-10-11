// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Accepted, Error};
use crate::group::{Context, body_limits};
use nfs_protocol::{
    gamemanager::*,
    users::{NetworkAddress, ObjectId},
};
use std::collections::BTreeSet;

/// Named local rule data. No account IDs, serialized requests or reply bodies.
#[derive(Clone)]
pub struct AttributeRule {
    pub name: Vec<u8>,
    pub threshold: Vec<u8>,
    pub values: Vec<Vec<u8>>,
}
#[derive(Clone)]
pub struct Config {
    pub protocol_version: Vec<u8>,
    pub game_rules: Vec<AttributeRule>,
    pub ued_rule: Vec<u8>,
    pub ued_threshold: Vec<u8>,
    pub player_attributes: Vec<(Vec<u8>, Vec<u8>)>,
    pub default_role: Vec<u8>,
    pub max_players: u16,
    pub player_count_threshold: Vec<u8>,
    pub utilization_threshold: Vec<u8>,
    pub ping_site_threshold: Vec<u8>,
    pub desired_percent_full: u8,
    pub creation_settings: u32,
    pub duration_micros: i64,
    pub starting_decay_ages: Vec<i64>,
}
fn text(v: &[u8]) -> bool {
    !v.is_empty() && v.len() <= 128 && v.iter().all(u8::is_ascii_graphic)
}
impl Config {
    pub fn validate(&self) -> Result<(), Error> {
        if [
            &self.protocol_version,
            &self.ued_rule,
            &self.ued_threshold,
            &self.default_role,
            &self.player_count_threshold,
            &self.utilization_threshold,
            &self.ping_site_threshold,
        ]
        .iter()
        .any(|v| !text(v))
            || self.max_players == 0
            || self.max_players > 64
            || self.desired_percent_full > 100
            || self.creation_settings > 0x00ff_ffff
            || self.duration_micros <= 0
            || self.duration_micros > 600_000_000
            || self.starting_decay_ages.is_empty()
            || self.starting_decay_ages.len() > 8
            || self
                .starting_decay_ages
                .iter()
                .any(|v| *v < 0 || *v > self.duration_micros)
            || self
                .starting_decay_ages
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != self.starting_decay_ages.len()
            || self.game_rules.is_empty()
            || self.game_rules.len() > 16
            || self
                .game_rules
                .iter()
                .map(|r| &r.name)
                .collect::<BTreeSet<_>>()
                .len()
                != self.game_rules.len()
            || self.game_rules.iter().any(|r| {
                !text(&r.name)
                    || !text(&r.threshold)
                    || r.values.is_empty()
                    || r.values.len() > 8
                    || r.values.iter().any(|v| !text(v))
                    || r.values.iter().collect::<BTreeSet<_>>().len() != r.values.len()
            })
            || self.player_attributes.is_empty()
            || self.player_attributes.len() > 16
            || self
                .player_attributes
                .iter()
                .any(|(k, v)| !text(k) || !text(v))
            || self
                .player_attributes
                .iter()
                .map(|(k, _)| k)
                .collect::<BTreeSet<_>>()
                .len()
                != self.player_attributes.len()
        {
            return Err(Error::Config);
        }
        Ok(())
    }
    /// Admit a single local member of a committed, initialized startup group.
    /// Field-presence and disabled-rule checks reject unsupported semantics.
    pub fn admit(&self, group: &Context, body: &[u8]) -> Result<Accepted, Error> {
        self.validate()?;
        let q =
            StartMatchmakingRequest::decode(body, body_limits()).map_err(|_| Error::Ineligible)?;
        if q.unknown_field_count() != 0
            || q.encode(body_limits()).map_err(|_| Error::Ineligible)? != body
        {
            return Err(Error::Ineligible);
        }
        self.check(&q, group.persona())?;
        let j = q.player_join_data.as_ref().ok_or(Error::Ineligible)?;
        let user = j.player_data_list.as_ref().ok_or(Error::Ineligible)?.0[0]
            .user
            .as_ref()
            .ok_or(Error::Ineligible)?;
        if j.group_id != Some(ObjectId(4, 2, group.group() as i64))
            || user.encode(body_limits()).map_err(|_| Error::Ineligible)? != group.user()
        {
            return Err(Error::Ineligible);
        }
        let c = q.common_game_data.ok_or(Error::Ineligible)?;
        let scenario = c.originating_scenario_id.ok_or(Error::Ineligible)?;
        let network = CommonGameRequestData {
            player_network_address: c.player_network_address,
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Ineligible)?;
        if network != group.network() {
            return Err(Error::Ineligible);
        }
        Ok(Accepted {
            group: group.group(),
            persona: group.persona(),
            user_session: group.player_session(),
            connection: group.connection(),
            scenario,
        })
    }
    fn check(&self, q: &StartMatchmakingRequest<'_>, persona: i64) -> Result<(), Error> {
        let c = q.common_game_data.as_ref().ok_or(Error::Ineligible)?;
        let s = c.scenario_info.as_ref().ok_or(Error::Ineligible)?;
        let g = q.game_creation_data.as_ref().ok_or(Error::Ineligible)?;
        let r = g.role_information.as_ref().ok_or(Error::Ineligible)?;
        let j = q.player_join_data.as_ref().ok_or(Error::Ineligible)?;
        let players = &j.player_data_list.as_ref().ok_or(Error::Ineligible)?.0;
        let session = q.session_data.as_ref().ok_or(Error::Ineligible)?;
        if q.unknown_field_count() != 0
            || c.game_type != Some(0)
            || c.game_protocol_version_string != Some(self.protocol_version.as_slice())
            || c.originating_scenario_id != Some(0)
            || !matches!(c.x_lspnetwork_address, Some(NetworkAddress::Unset))
            || s.scenario_name != Some(b"")
            || s.scenario_version != Some(0)
            || s.scenario_variant != Some(0)
            || s.sub_session_name != Some(b"")
            || g.game_attribs.is_some()
            || g.entry_criteria_map.is_some()
            || g.game_mod_register != Some(0)
            || g.game_name != Some(b"")
            || g.game_settings != Some(self.creation_settings)
            || g.network_topology != Some(0)
            || g.max_player_capacity != Some(0)
            || g.min_player_capacity != Some(1)
            || g.presence_mode != Some(1)
            || g.queue_capacity != Some(0)
            || r.role_criteria_map.is_some()
            || r.multi_role_criteria.is_some()
            || g.external_session_template_name != Some(b"")
            || g.voip_network != Some(0)
            || j.default_role != Some(self.default_role.as_slice())
            || j.game_entry_type != Some(0)
            || j.slot_type != Some(0)
            || j.team_id != Some(65534)
            || j.team_index != Some(65535)
            || players.len() != 1
            || players[0].is_optional_player != Some(false)
            || players[0].role != Some(b"")
            || session.session_duration != Some(self.duration_micros)
            || session.debug_freeze_decay != Some(false)
            || session.session_mode != Some(1)
            || session.external_mm_session_template_name != Some(b"")
            || session.pseudo_request != Some(false)
            || session.start_delay != Some(0)
            || session
                .starting_decay_age
                .is_none_or(|v| !self.starting_decay_ages.contains(&v))
        {
            return Err(Error::Ineligible);
        }
        let attrs = &players[0]
            .player_attributes
            .as_ref()
            .ok_or(Error::Ineligible)?
            .0;
        if attrs.len() != self.player_attributes.len()
            || self.player_attributes.iter().any(|(k, v)| {
                attrs
                    .iter()
                    .filter(|(a, b)| *a == k.as_slice() && *b == v.as_slice())
                    .count()
                    != 1
            })
        {
            return Err(Error::Ineligible);
        }
        let c = q.criteria_data.as_ref().ok_or(Error::Ineligible)?;
        let rules = &c
            .game_attribute_rule_criteria_map
            .as_ref()
            .ok_or(Error::Ineligible)?
            .0;
        if c.variable_custom_rule_prefs.is_some()
            || c.player_attribute_rule_criteria_map.is_some()
            || rules.len() != self.game_rules.len()
        {
            return Err(Error::Ineligible);
        }
        for rule in &self.game_rules {
            let matches: Vec<_> = rules.iter().filter(|(k, _)| *k == rule.name).collect();
            if matches.len() != 1 {
                return Err(Error::Ineligible);
            }
            let value = &matches[0].1;
            if value.min_fit_threshold_name != Some(rule.threshold.as_slice())
                || value.desired_values.as_ref().is_none_or(|v| {
                    v.0.iter()
                        .copied()
                        .ne(rule.values.iter().map(Vec::as_slice))
                })
            {
                return Err(Error::Ineligible);
            }
        }
        let ued = &c.ued_rule_criteria_map.as_ref().ok_or(Error::Ineligible)?.0;
        if ued.len() != 1
            || ued[0].0 != self.ued_rule
            || ued[0].1.client_ued_search_value != Some(i64::MIN)
            || ued[0].1.override_ued_value != Some(i64::MIN)
            || ued[0].1.threshold_name != Some(self.ued_threshold.as_slice())
        {
            return Err(Error::Ineligible);
        }
        // The minimum signed values are opaque sentinels, never player skill.
        macro_rules! fields {
            ($parent:ident,$field:ident; $($name:ident=$value:expr),+ $(,)?)=>{{
                let v=$parent.$field.as_ref().ok_or(Error::Ineligible)?;
                if $(v.$name!=$value)||+ { return Err(Error::Ineligible); }
            }};
        }
        if c.avoid_games_rule_criteria
            .as_ref()
            .ok_or(Error::Ineligible)?
            .game_id_list
            .is_some()
        {
            return Err(Error::Ineligible);
        }
        let avoid = c
            .avoid_players_rule_criteria
            .as_ref()
            .ok_or(Error::Ineligible)?;
        if avoid.avoid_list.is_some() || avoid.avoid_list_ids.is_some() {
            return Err(Error::Ineligible);
        }
        fields!(c,free_player_slots_rule_criteria; max_free_player_slots=Some(65535),min_free_player_slots=Some(0));
        fields!(c,geo_location_rule_criteria; min_fit_threshold_name=Some(b"".as_slice()));
        fields!(c,game_name_rule_criteria; search_string=Some(b"".as_slice()));
        fields!(c,mod_rule_criteria; is_enabled=Some(false),desired_mod_register=Some(0));
        fields!(c,host_balancing_rule_prefs; min_fit_threshold_name=Some(b"".as_slice()));
        fields!(c,player_count_rule_criteria; is_single_group_match=Some(0),max_player_count=Some(self.max_players),desired_player_count=Some(self.max_players),min_player_count=Some(1),range_offset_list_name=Some(self.player_count_threshold.as_slice()));
        fields!(c,player_slot_utilization_rule_criteria; desired_percent_full=Some(self.desired_percent_full),max_percent_full=Some(100),min_percent_full=Some(0),range_offset_list_name=Some(self.utilization_threshold.as_slice()));
        if c.preferred_games_rule_criteria
            .as_ref()
            .ok_or(Error::Ineligible)?
            .preferred_list
            .is_some()
            || c.preferred_players_rule_criteria
                .as_ref()
                .ok_or(Error::Ineligible)?
                .preferred_list
                .is_some()
        {
            return Err(Error::Ineligible);
        }
        fields!(c,preferred_games_rule_criteria; require_preferred_game=Some(false));
        fields!(c,preferred_players_rule_criteria; preferred_list_id=Some(ObjectId(25,1,persona)),require_preferred_player=Some(false));
        fields!(c,ping_site_rule_prefs; min_fit_threshold_name=Some(self.ping_site_threshold.as_slice()));
        fields!(c,ranked_game_rule_prefs; min_fit_threshold_name=Some(b"".as_slice()),desired_ranked_game_value=Some(2));
        fields!(c,reputation_rule_prefs; reputation_requirement=Some(1));
        fields!(c,roster_size_rule_prefs; max_player_count=Some(65535),min_player_count=Some(0));
        fields!(c,team_balance_rule_prefs; max_team_size_difference_allowed=Some(0),range_offset_list_name=Some(b"".as_slice()));
        fields!(c,team_count_rule_prefs; team_count=Some(0));
        fields!(c,team_composition_rule_prefs; rule_name=Some(b"".as_slice()),min_fit_threshold_name=Some(b"".as_slice()));
        fields!(c,team_min_size_rule_prefs; team_min_size=Some(0),range_offset_list_name=Some(b"".as_slice()));
        fields!(c,total_player_slots_rule_criteria; desired_total_player_slots=Some(1),max_total_player_slots=Some(1),min_total_player_slots=Some(1),range_offset_list_name=Some(b"".as_slice()));
        fields!(c,team_ued_position_parity_rule_prefs; rule_name=Some(b"".as_slice()),range_offset_list_name=Some(b"".as_slice()));
        fields!(c,team_ued_balance_rule_prefs; rule_name=Some(b"".as_slice()),range_offset_list_name=Some(b"".as_slice()));
        fields!(c,host_viability_rule_prefs; min_fit_threshold_name=Some(b"".as_slice()));
        fields!(c,virtual_game_rule_prefs; min_fit_threshold_name=Some(b"".as_slice()),desired_virtual_game_value=Some(8));
        Ok(())
    }
}
mod config;
