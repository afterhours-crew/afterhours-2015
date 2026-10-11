// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Deployment-selected scene and asset roles, separate from runtime object IDs.
//! Validated static role bindings for a deployment's scene and asset catalogs.
//! These keys identify content, not allocated session objects or captured ghosts.
use crate::Failure;
use nfs_world::replication::{entity::creation::Asset, sublevel::root};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneRoles {
    pub level: Vec<u8>,
    pub gameplay: u32,
    pub startup: u32,
    pub garage: u32,
    pub progression: u32,
    pub traffic: [u32; 10],
    pub customization_timer: Asset,
    pub streaming_gate: Asset,
    /// The level's SpawnPoints scene (`levels/Genesis01/SpawnPoints` in the
    /// official session). Optional: without it, world spawns stay unsupported.
    pub spawn_points: Option<u32>,
}

fn fields(v: &Value, expected: &[&str]) -> Result<(), Failure> {
    fields_with(v, expected, &[])
}
fn fields_with(v: &Value, required: &[&str], optional: &[&str]) -> Result<(), Failure> {
    let object = v.as_object().ok_or(Failure::ProfileConfig)?;
    if required.iter().any(|k| !object.contains_key(*k))
        || object
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err(Failure::ProfileConfig);
    }
    Ok(())
}
fn number<T: TryFrom<u64>>(v: &Value) -> Result<T, Failure> {
    T::try_from(v.as_u64().ok_or(Failure::ProfileConfig)?).map_err(|_| Failure::ProfileConfig)
}
fn asset(v: &Value) -> Result<Asset, Failure> {
    fields(v, &["bundle", "type_id", "local_index"])?;
    Ok(Asset {
        bundle: number(&v["bundle"])?,
        type_id: number(&v["type_id"])?,
        local_index: number(&v["local_index"])?,
    })
}
fn asset_json(a: Asset) -> Value {
    json!({"bundle":a.bundle,"type_id":a.type_id,"local_index":a.local_index})
}
impl SceneRoles {
    pub fn validate(&self) -> Result<(), Failure> {
        if self.level.is_empty()
            || self.level.len() > 1023
            || !self.level.is_ascii()
            || self.level.contains(&0)
        {
            return Err(Failure::ProfileConfig);
        }
        root::validate_traffic_keys(&self.traffic).map_err(|_| Failure::ProfileConfig)?;
        let mut seen = BTreeSet::new();
        for key in [self.gameplay, self.startup, self.garage, self.progression]
            .into_iter()
            .chain(self.traffic)
            .chain(self.spawn_points)
        {
            if key <= 1 || !seen.insert(key) {
                return Err(Failure::ProfileConfig);
            }
        }
        for a in [self.customization_timer, self.streaming_gate] {
            if a.bundle == 0 || a.bundle > 8191 {
                return Err(Failure::ProfileConfig);
            }
        }
        if self.customization_timer == self.streaming_gate {
            return Err(Failure::ProfileConfig);
        }
        Ok(())
    }
    pub fn from_json(v: &Value) -> Result<Self, Failure> {
        fields_with(
            v,
            &[
                "level",
                "gameplay",
                "startup",
                "garage",
                "progression",
                "traffic",
                "customization_timer",
                "streaming_gate",
            ],
            &["spawn_points"],
        )?;
        let level = v["level"]
            .as_str()
            .ok_or(Failure::ProfileConfig)?
            .as_bytes()
            .to_vec();
        let traffic = v["traffic"]
            .as_array()
            .filter(|a| a.len() == 10)
            .ok_or(Failure::ProfileConfig)?;
        let result = Self {
            level,
            gameplay: number(&v["gameplay"])?,
            startup: number(&v["startup"])?,
            garage: number(&v["garage"])?,
            progression: number(&v["progression"])?,
            traffic: traffic
                .iter()
                .map(number)
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| Failure::ProfileConfig)?,
            customization_timer: asset(&v["customization_timer"])?,
            streaming_gate: asset(&v["streaming_gate"])?,
            spawn_points: v.get("spawn_points").map(number).transpose()?,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn to_json(&self) -> Value {
        let mut v = json!({"level":String::from_utf8_lossy(&self.level),"gameplay":self.gameplay,"startup":self.startup,"garage":self.garage,"progression":self.progression,"traffic":self.traffic,"customization_timer":asset_json(self.customization_timer),"streaming_gate":asset_json(self.streaming_gate)});
        if let Some(key) = self.spawn_points {
            v["spawn_points"] = json!(key);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roles_are_explicit_bounded_and_round_trip_without_runtime_ids() {
        let v = json!({"level":"test/world","gameplay":101,"startup":102,"garage":103,"progression":104,"traffic":[11,12,13,14,15,16,17,18,19,20],"customization_timer":{"bundle":2,"type_id":3,"local_index":4},"streaming_gate":{"bundle":5,"type_id":6,"local_index":7}});
        assert_eq!(SceneRoles::from_json(&v).unwrap().to_json(), v);
        for (pointer, value) in [
            ("/gameplay", json!(1)),
            ("/startup", json!(101)),
            ("/garage", json!(11)),
            ("/traffic/9", json!(11)),
            ("/traffic/0", json!(0)),
            ("/customization_timer/bundle", json!(8192)),
            ("/level", json!("")),
            ("/level", json!("a".repeat(1024))),
        ] {
            let mut bad = v.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(SceneRoles::from_json(&bad).is_err(), "{pointer}");
        }
        let mut bad = v.clone();
        bad["runtime_object_id"] = json!(1);
        assert!(SceneRoles::from_json(&bad).is_err());
        let mut spawn = v;
        spawn["spawn_points"] = json!(105);
        let roles = SceneRoles::from_json(&spawn).unwrap();
        assert_eq!(roles.spawn_points, Some(105));
        assert_eq!(roles.to_json(), spawn);
        for value in [json!(101), json!(11), json!(1), json!(-1), json!("105")] {
            let mut bad = spawn.clone();
            bad["spawn_points"] = value;
            assert!(SceneRoles::from_json(&bad).is_err());
        }
    }
}
