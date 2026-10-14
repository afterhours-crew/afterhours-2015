// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Authored single-player AllDrive policy. Strings used by the client's request
//! contract are identifiers, not downloaded configuration or serialized replies.
use nfs_services::SUPPORTED_BUILD_SHA256 as BUILD;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The installed menu addresses three daily slots without an empty-list guard.
/// These are fixed local preview objectives, not a rotating service schedule;
/// no rewards or completion are granted by this read-only catalog.
pub(super) fn challenges(types: &BTreeMap<String, u32>) -> Result<Value, String> {
    let selected = [
        ("ChallengeType_GetXAmountOfREP", 1000),
        ("ChallengeType_DoXDoughnuts", 1),
        ("ChallengeType_GetXNearMisses", 5),
    ];
    let definitions = selected.into_iter().enumerate().map(|(i,(name,count))| {
        let kind = types.get(name).ok_or_else(||format!("missing installed challenge type {name}"))?;
        Ok(json!({"id":i+1,"car_id":0,"count_one":count,"count_two":0,"day":"1970/01/01-00:00:00","event_id":0,"kind":kind,"weekly":false,"type_override_id":0}))
    }).collect::<Result<Vec<_>,String>>()?;
    Ok(
        json!({"version":1,"build_sha256":BUILD,"day_id":1,"monthly_start":0,"awards":[],"ranks":[],"challenges":definitions}),
    )
}

pub(super) fn documents() -> BTreeMap<&'static str, Value> {
    let rules = [
        ("experienceRule", "requireExactMatch", "PUBLIC"),
        ("gameMultiplayerTypeRule", "requireDecay", "AllDrive"),
        ("gameSpeedListTypeRule", "requireDecay", "None"),
        ("serverTypeRule", "requireExactMatch", "OFFICIAL"),
    ];
    let player = json!([["multiplayerType", "AllDrive"], ["speedListType", "None"]]);
    BTreeMap::from([
        (
            "bootstrap-config",
            nfs_services::bootstrap::local_configuration(),
        ),
        (
            "auth-config",
            json!({"version":1,"build_sha256":BUILD,
            "first_console":false,"first_login":false,"locale":u32::from_be_bytes(*b"enUS"),
            "namespace":"cem_ea_id","session_type":0,"object_parts":[30722,2],
            "filter":"-UION/****","no_toggle_ok":"US,CA,MX","use_server_time":"Default",
            "service_name":"afterhours-local","ticker_key_length":57}),
        ),
        (
            "group-policy",
            json!({"format":"nfs-group-policy","version":1,"build_sha256":BUILD,
            "protocol_version":"592762504","protocol_hash":2946314223_u64,
            "ping_site":"bio-dub","mode_attribute":"experience","telemetry_interval":20000000}),
        ),
        (
            "matchmaking-admission",
            json!({"version":1,"protocol_version":"592762504",
            "game_rules":rules.iter().map(|(name,threshold,value)|json!({"name":name,"threshold":threshold,"values":[value]})).collect::<Vec<_>>(),
            "ued_rule":"matchmakingSkill_Rule","ued_threshold":"decay",
            "player_attributes":player,"default_role":"AllDrive","max_players":8,
            "player_count_threshold":"matchGame","utilization_threshold":"matchAny",
            "ping_site_threshold":"requireDecay","desired_percent_full":75,
            "creation_settings":2055,"duration_micros":30000000,"starting_decay_ages":[0,1000]}),
        ),
        (
            "matchmaking-policy",
            json!({"version":1,
            "game_rules":rules.iter().map(|(name,_,value)|((*name).to_owned(),json!([value]))).collect::<BTreeMap<_,_>>(),
            "ping_sites":["bio-dub"],"players":8,"host_balance":2,"host_viability":3,
            "rank_flags":15,"virtual_flags":3,"skill":{"rule":"matchmakingSkill_Rule","value":0,"min":0,"max":0}}),
        ),
        (
            "world-policy",
            json!({"version":1,"protocol_version":"592762504","protocol_hash":2946314223_u64,
            "ping_site":"bio-dub","mode_attribute":"experience","telemetry_interval":20000000,
            "game_attributes":[["experience","PUBLIC"],["FPS","30"],["mode","FREEDRIVE"],["multiplayerType","AllDrive"],["serverType","OFFICIAL"],["speedListType","None"]],
            "player_attributes":player,"player_role":"AllDrive","game_settings":33071,
            "max_players":8,"min_players":1,"slot_capacities":[8,0,0,0],"teams":[65534],
            "roles":{"AllDrive":8,"Driver":8,"SpeedList":8},"multi_role_criteria":[["MMSession","!SpeedList || !AllDrive"]],
            "report_name":"frostbite_multiplayer","max_fit":131000}),
        ),
        // Local account ownership does not impersonate remote product purchases.
        (
            "account-policy",
            json!({"format":"nfs-fresh-account-policy","version":1,"build_sha256":BUILD,
            "entitlements":{"scopes":[["NFS16PC"],["NFS11HP360","NFS11HPPC","NFS11HPPS3","NFS12360","NFS12PC","NFS12PS3","NFS13360","NFS13ANDROID","NFS13PC","NFS13PS3","NFS13VITA","NFS13WIIU","NFS13iOS","NFS14360","NFS14PC","NFS14PS3","NFS14PS4","NFS14XONE"]],"grants":[]},"screenshot_count_max":100}),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_challenges_require_installed_types_and_use_local_identity_without_rewards() {
        assert!(challenges(&BTreeMap::new()).is_err());
        let types = [
            ("ChallengeType_GetXAmountOfREP", 91),
            ("ChallengeType_DoXDoughnuts", 92),
            ("ChallengeType_GetXNearMisses", 93),
        ]
        .into_iter()
        .map(|(s, n)| (s.to_owned(), n))
        .collect();
        let catalog = challenges(&types).unwrap();
        nfs_services::challenges::Catalog::from_json(&catalog).unwrap();
        assert_eq!(catalog["challenges"].as_array().unwrap().len(), 3);
        assert_eq!(catalog["challenges"][0]["kind"], 91);
        assert!(catalog["awards"].as_array().unwrap().is_empty());
    }
    #[test]
    fn local_policies_load_without_private_files_or_player_data() {
        let p = documents();
        let bytes = |key| serde_json::to_vec(&p[key]).unwrap();
        nfs_services::bootstrap::Config::from_json(&p["bootstrap-config"]).unwrap();
        nfs_services::authentication::Config::from_json(&p["auth-config"]).unwrap();
        nfs_services::group::Config::from_json(&p["group-policy"]).unwrap();
        nfs_services::matchmaking::Config::from_json(&bytes("matchmaking-admission")).unwrap();
        nfs_services::matchmaking_status::Config::from_slice(&bytes("matchmaking-policy")).unwrap();
        nfs_services::world_setup::Config::from_slice(&bytes("world-policy")).unwrap();
        nfs_services::fresh_account::Policy::from_json(&p["account-policy"]).unwrap();
        assert!(
            p["account-policy"]["entitlements"]["grants"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
