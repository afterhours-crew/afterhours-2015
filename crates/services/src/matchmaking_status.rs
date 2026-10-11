// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Initial local matchmaking diagnostic. This does not allocate a world or
//! claim that a match succeeded. Policy is separate from accepted session IDs.
use crate::bootstrap::{body_limits, frame_limits};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::{autolog::StringList, gamemanager::*};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    ProfileConfig,
    IneligibleContext,
    Pending,
    Stopped,
    Encode,
}
#[derive(Clone)]
pub struct Config {
    rules: Vec<(String, Vec<String>)>,
    sites: Vec<String>,
    players: u16,
    balance: i32,
    viability: i32,
    rank: u8,
    virtualized: u8,
    skill_rule: String,
    skill: i64,
    skill_min: i64,
    skill_max: i64,
}
impl Config {
    /// Bounded deployment policy, never a wire response or an account snapshot.
    /// Skill values describe the local initial search policy only; callers must
    /// replace policy when implementing progression-dependent matchmaking.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Failure> {
        let bad = Failure::ProfileConfig;
        if bytes.len() > 4096 {
            return Err(bad);
        }
        let v: Value = serde_json::from_slice(bytes).map_err(|_| bad)?;
        let obj = v.as_object().ok_or(bad)?;
        const KEYS: &[&str] = &[
            "version",
            "game_rules",
            "ping_sites",
            "players",
            "host_balance",
            "host_viability",
            "rank_flags",
            "virtual_flags",
            "skill",
        ];
        if obj.len() != KEYS.len()
            || KEYS.iter().any(|k| !obj.contains_key(*k))
            || v["version"].as_u64() != Some(1)
        {
            return Err(bad);
        }
        fn string(v: &Value) -> Result<String, Failure> {
            let s = v.as_str().ok_or(Failure::ProfileConfig)?;
            if s.is_empty() || s.len() > 64 || !s.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(Failure::ProfileConfig);
            }
            Ok(s.to_owned())
        }
        fn strings(v: &Value) -> Result<Vec<String>, Failure> {
            let a = v.as_array().ok_or(Failure::ProfileConfig)?;
            if a.is_empty() || a.len() > 8 {
                return Err(Failure::ProfileConfig);
            }
            let out = a.iter().map(string).collect::<Result<Vec<_>, _>>()?;
            let unique: std::collections::BTreeSet<_> = out.iter().collect();
            if unique.len() != out.len() {
                return Err(Failure::ProfileConfig);
            }
            Ok(out)
        }
        let map = v["game_rules"].as_object().ok_or(bad)?;
        if map.is_empty() || map.len() > 8 {
            return Err(bad);
        }
        let rules = map
            .iter()
            .map(|(k, v)| Ok((string(&Value::String(k.clone()))?, strings(v)?)))
            .collect::<Result<Vec<_>, Failure>>()?;
        let skill = v["skill"].as_object().ok_or(bad)?;
        if skill.len() != 4
            || ["rule", "value", "min", "max"]
                .iter()
                .any(|k| !skill.contains_key(*k))
        {
            return Err(bad);
        }
        let integer = |key: &str| v[key].as_i64().ok_or(bad);
        let n = |key: &str| {
            v["skill"][key]
                .as_i64()
                .filter(|n| i32::try_from(*n).is_ok())
                .ok_or(bad)
        };
        let out = Self {
            rules,
            sites: strings(&v["ping_sites"])?,
            players: u16::try_from(integer("players")?).map_err(|_| bad)?,
            balance: i32::try_from(integer("host_balance")?).map_err(|_| bad)?,
            viability: i32::try_from(integer("host_viability")?).map_err(|_| bad)?,
            rank: u8::try_from(integer("rank_flags")?).map_err(|_| bad)?,
            virtualized: u8::try_from(integer("virtual_flags")?).map_err(|_| bad)?,
            skill_rule: string(&v["skill"]["rule"])?,
            skill: n("value")?,
            skill_min: n("min")?,
            skill_max: n("max")?,
        };
        if out.players == 0
            || out.players > 64
            || out.skill_min > out.skill
            || out.skill > out.skill_max
        {
            return Err(bad);
        }
        Ok(out)
    }
    pub fn load(path: &std::path::Path) -> Result<Self, Failure> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| Failure::ProfileConfig)?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::ProfileConfig)?;
        Self::from_slice(&bytes)
    }
}
/// Supplied by the owner of an accepted matchmaking request after G1 init.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub group_id: u64,
    pub session_id: u64,
    pub user_session_id: u64,
    pub scenario_id: u64,
}
impl Accepted {
    fn valid(self) -> bool {
        [self.group_id, self.session_id, self.user_session_id]
            .iter()
            .all(|v| *v > 0 && *v <= i64::MAX as u64)
            && self.group_id != self.session_id
            && self.scenario_id <= i64::MAX as u64
    }
}
pub struct Session {
    config: Config,
    current: Accepted,
    pending: bool,
    published: bool,
    closed: bool,
}
impl Session {
    pub fn new(config: &Config, current: Accepted) -> Result<Self, Failure> {
        if !current.valid() {
            return Err(Failure::IneligibleContext);
        }
        Ok(Self {
            config: config.clone(),
            current,
            pending: false,
            published: false,
            closed: false,
        })
    }
    pub fn has_pending_write(&self) -> bool {
        self.pending
    }
    pub fn published(&self) -> bool {
        self.published && !self.closed
    }
    /// Queue after the initial M1 response. Only the edge's complete-batch write
    /// may call commit_after_write; construction alone does not publish status.
    pub fn emit_after_m1(&mut self, accepted: Accepted) -> Result<Option<Vec<u8>>, Failure> {
        if self.closed {
            return Err(Failure::Stopped);
        }
        let result = self.emit(accepted);
        if result.is_err() {
            self.abort_write();
        }
        result
    }
    fn emit(&mut self, accepted: Accepted) -> Result<Option<Vec<u8>>, Failure> {
        if !accepted.valid() || accepted != self.current {
            return Err(Failure::IneligibleContext);
        }
        if self.pending {
            return Err(Failure::Pending);
        }
        if self.published {
            return Ok(None);
        }
        let c = &self.config;
        let status = MatchmakingAsyncStatus {
            create_game_status: Some(CreateGameStatus {
                evaluate_status: Some(0),
                num_of_matchmaking_session: Some(0),
                num_of_matched_players: Some(0),
                ..Default::default()
            }),
            find_game_status: Some(FindGameStatus {
                num_of_games: Some(0),
                ..Default::default()
            }),
            game_attribute_rule_status_map: Some(GameAttributeRuleStatusMap(
                c.rules
                    .iter()
                    .map(|(key, values)| {
                        (
                            key.as_bytes(),
                            GameAttributeRuleStatus {
                                rule_name: Some(key.as_bytes()),
                                matched_values: Some(StringList(
                                    values.iter().map(|v| v.as_bytes()).collect(),
                                )),
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
            )),
            geo_location_rule_status: Some(GeoLocationRuleStatus {
                max_distance: Some(0),
                ..Default::default()
            }),
            host_balance_rule_status: Some(HostBalanceRuleStatus {
                matched_host_balance_value: Some(c.balance),
                ..Default::default()
            }),
            host_viability_rule_status: Some(HostViabilityRuleStatus {
                matched_host_viability_value: Some(c.viability),
                ..Default::default()
            }),
            player_count_rule_status: Some(PlayerCountRuleStatus {
                max_player_count_accepted: Some(c.players),
                min_player_count_accepted: Some(c.players),
                ..Default::default()
            }),
            player_slot_utilization_rule_status: Some(PlayerSlotUtilizationRuleStatus {
                max_percent_full_accepted: Some(100),
                min_percent_full_accepted: Some(0),
                ..Default::default()
            }),
            ping_site_rule_status: Some(PingSiteRuleStatus {
                matched_values: Some(StringList(c.sites.iter().map(|v| v.as_bytes()).collect())),
                ..Default::default()
            }),
            rank_rule_status: Some(RankRuleStatus {
                matched_rank_flags: Some(c.rank),
                ..Default::default()
            }),
            team_balance_rule_status: Some(TeamBalanceRuleStatus {
                max_team_size_difference_accepted: Some(0),
                ..Default::default()
            }),
            team_composition_rule_status: Some(TeamCompositionRuleStatus {
                rule_name: Some(b""),
                ..Default::default()
            }),
            team_min_size_rule_status: Some(TeamMinSizeRuleStatus {
                team_min_size_accepted: Some(0),
                ..Default::default()
            }),
            total_player_slots_rule_status: Some(TotalPlayerSlotsRuleStatus {
                max_total_player_slots_accepted: Some(0),
                min_total_player_slots_accepted: Some(0),
                ..Default::default()
            }),
            team_ued_position_parity_rule_status: Some(TeamUEDPositionParityRuleStatus {
                max_ued_difference_accepted_bottom_players: Some(0),
                bottom_players_counted: Some(0),
                rule_name: Some(b""),
                max_ued_difference_accepted_top_players: Some(0),
                top_players_counted: Some(0),
                ..Default::default()
            }),
            team_ued_balance_rule_status: Some(TeamUEDBalanceRuleStatus {
                my_ued_value: Some(0),
                rule_name: Some(b""),
                max_team_ued_difference_accepted: Some(0),
                ..Default::default()
            }),
            ued_rule_status_map: Some(UEDRuleStatusMap(vec![(
                c.skill_rule.as_bytes(),
                UEDRuleStatus {
                    max_ued_accepted: Some(c.skill_max),
                    min_ued_accepted: Some(c.skill_min),
                    my_ued_value: Some(c.skill),
                    rule_name: Some(c.skill_rule.as_bytes()),
                    ..Default::default()
                },
            )])),
            virtual_game_rule_status: Some(VirtualGameRuleStatus {
                matched_virtualized_flags: Some(c.virtualized),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = NotifyMatchmakingAsyncStatus {
            matchmaking_async_status_list: Some(MatchmakingAsyncStatusList(vec![status])),
            scenario_id: Some(accepted.scenario_id),
            session_id: Some(accepted.session_id),
            user_session_id: Some(accepted.user_session_id),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Failure::Encode)?;
        let wire = nfs_fire2::encode(
            Frame {
                fields: Fields {
                    routing_a: 4,
                    routing_b: 12,
                    category: 2,
                    ..Fields::default()
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .map_err(|_| Failure::Encode)?;
        self.pending = true;
        Ok(Some(wire))
    }
    pub fn commit_after_write(&mut self) -> Result<(), Failure> {
        if self.closed {
            return Err(Failure::Stopped);
        }
        if self.pending {
            self.pending = false;
            self.published = true;
        }
        Ok(())
    }
    pub fn abort_write(&mut self) {
        self.pending = false;
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> Value {
        serde_json::json!({"version":1,"game_rules":{"mode":["practice"]},"ping_sites":["local"],"players":4,"host_balance":1,"host_viability":2,"rank_flags":7,"virtual_flags":1,"skill":{"rule":"ability","value":12,"min":8,"max":16}})
    }
    fn config() -> Config {
        Config::from_slice(&serde_json::to_vec(&policy()).unwrap()).unwrap()
    }
    fn current() -> Accepted {
        Accepted {
            group_id: 11,
            session_id: 22,
            user_session_id: 33,
            scenario_id: 44,
        }
    }
    #[test]
    fn publication_requires_write_and_retries_do_not_republish() {
        let mut s = Session::new(&config(), current()).unwrap();
        let bytes = s.emit_after_m1(current()).unwrap().unwrap();
        assert!(!s.published());
        assert!(s.has_pending_write());
        let f = nfs_fire2::decode(&bytes, frame_limits())
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
            (4, 12, 2, 0)
        );
        let body = NotifyMatchmakingAsyncStatus::decode(f.body, body_limits()).unwrap();
        assert_eq!(
            (body.session_id, body.user_session_id, body.scenario_id),
            (Some(22), Some(33), Some(44))
        );
        let a = &body.matchmaking_async_status_list.as_ref().unwrap().0[0];
        assert_eq!(
            a.player_count_rule_status
                .as_ref()
                .unwrap()
                .max_player_count_accepted,
            Some(4)
        );
        assert_eq!(
            a.game_attribute_rule_status_map.as_ref().unwrap().0[0]
                .1
                .matched_values
                .as_ref()
                .unwrap()
                .0,
            vec![b"practice".as_slice()]
        );
        assert_eq!(body.unknown_field_count(), 0);
        s.commit_after_write().unwrap();
        assert!(s.published());
        for _ in 0..100 {
            assert!(s.emit_after_m1(current()).unwrap().is_none());
        }
    }
    #[test]
    fn premature_retry_and_partial_write_close() {
        for retry in [true, false] {
            let mut s = Session::new(&config(), current()).unwrap();
            s.emit_after_m1(current()).unwrap();
            if retry {
                assert_eq!(s.emit_after_m1(current()), Err(Failure::Pending));
            } else {
                s.abort_write();
            }
            assert!(!s.published());
            assert!(!s.has_pending_write());
            assert_eq!(s.commit_after_write(), Err(Failure::Stopped));
            assert_eq!(s.emit_after_m1(current()), Err(Failure::Stopped));
        }
    }
    #[test]
    fn session_and_policy_isolation() {
        let mut a = Session::new(&config(), current()).unwrap();
        let other = Accepted {
            session_id: 55,
            ..current()
        };
        let mut p = policy();
        p["players"] = 2.into();
        let mut b = Session::new(
            &Config::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap(),
            other,
        )
        .unwrap();
        let x = a.emit_after_m1(current()).unwrap().unwrap();
        let y = b.emit_after_m1(other).unwrap().unwrap();
        assert_ne!(x, y);
        a.commit_after_write().unwrap();
        assert!(a.published());
        assert!(!b.published());
        assert_eq!(b.emit_after_m1(current()), Err(Failure::IneligibleContext));
        assert!(a.published());
    }
    #[test]
    fn invalid_bindings_rejected() {
        for c in [
            Accepted {
                group_id: 0,
                ..current()
            },
            Accepted {
                session_id: 11,
                ..current()
            },
            Accepted {
                user_session_id: u64::MAX,
                ..current()
            },
            Accepted {
                scenario_id: u64::MAX,
                ..current()
            },
        ] {
            assert!(matches!(
                Session::new(&config(), c),
                Err(Failure::IneligibleContext)
            ));
        }
    }
    #[test]
    fn policy_bounds_and_schema() {
        for (key, value) in [
            ("players", serde_json::json!(0)),
            ("players", serde_json::json!(65)),
            ("rank_flags", serde_json::json!(256)),
            ("ping_sites", serde_json::json!(["x", "x"])),
            ("game_rules", serde_json::json!({})),
            ("version", serde_json::json!(2)),
            ("extra", serde_json::json!(0)),
            (
                "skill",
                serde_json::json!({"rule":"a","value":20,"min":1,"max":10}),
            ),
        ] {
            let mut p = policy();
            p[key] = value;
            assert!(
                Config::from_slice(&serde_json::to_vec(&p).unwrap()).is_err(),
                "{key}"
            );
        }
        let raw = serde_json::to_vec(&policy()).unwrap();
        for end in 0..raw.len() {
            assert!(Config::from_slice(&raw[..end]).is_err());
        }
        assert!(Config::from_slice(&vec![b' '; 4097]).is_err());
    }
}
