// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use crate::{
    Error,
    graph::{Node, bad},
};
use nfs_world::crash::route::Config;

pub(super) fn extract(nodes: &[Node]) -> Result<Option<Config>, Error> {
    let mut providers = nodes
        .iter()
        .filter(|n| n.object.class == "VehicleResetRouteEntityData");
    let Some(node) = providers.next() else {
        return Ok(None);
    };
    if providers.next().is_some()
        || node
            .object
            .fields
            .get("Realm")
            .and_then(|v| v.as_enum_name())
            != Some("Realm_Server")
    {
        return Err(bad("event recovery provider realm/count"));
    }
    let value = |name| super::finite(node.object.fields.get(name), "event recovery parameter");
    Config {
        time_before_corner: value("TimeBeforeCorner")?,
        sharp_corner_degrees: value("SharpCornerThreshold")?,
        distance_before_checkpoint: value("DistanceBeforeCheckpoint")?,
        time_before_checkpoint: value("TimeBeforeCheckpoint")?,
        min_speed: value("MinResetSpeed")?,
        max_speed_percent: value("MaxResetSpeedPercent")?,
        max_delta_time: value("MaxDeltaTime")?,
    }
    .validate()
    .map(Some)
    .map_err(|_| bad("event recovery bounds"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Site, tests::Memory};
    use nfs_frostbite::ebx::Value;
    fn node() -> Node {
        let mut memory = Memory::default();
        memory.put(
            "synthetic",
            1,
            "VehicleResetRouteEntityData",
            &[
                (
                    "Realm",
                    Value::Enum {
                        value: 0,
                        name: Some("Realm_Server".into()),
                    },
                ),
                ("TimeBeforeCorner", Value::Float32(2.)),
                ("SharpCornerThreshold", Value::Float32(65.)),
                ("DistanceBeforeCheckpoint", Value::Float32(12.)),
                ("TimeBeforeCheckpoint", Value::Float32(0.75)),
                ("MinResetSpeed", Value::Float32(25.)),
                ("MaxResetSpeedPercent", Value::Float32(0.6)),
                ("MaxDeltaTime", Value::Float32(3.)),
            ],
        );
        Node {
            site: Site {
                key: ("synthetic".into(), 1),
                ..Site::root("synthetic")
            },
            object: memory.0[&("synthetic".into(), 1)].clone(),
        }
    }
    #[test]
    fn extracts_authored_values_and_rejects_ambiguous_or_invalid_provider() {
        let n = node();
        let c = extract(std::slice::from_ref(&n)).unwrap().unwrap();
        assert_eq!(c.min_speed, 25.);
        assert_eq!(c.max_speed_percent, 0.6);
        assert_eq!(c.max_delta_time, 3.);
        assert!(extract(&[]).unwrap().is_none());
        assert!(extract(&[n.clone(), n.clone()]).is_err());
        for (field, value) in [
            (
                "Realm",
                Value::Enum {
                    value: 1,
                    name: Some("Realm_Client".into()),
                },
            ),
            ("MaxResetSpeedPercent", Value::Float32(2.)),
            ("MaxDeltaTime", Value::Float32(0.)),
            ("TimeBeforeCorner", Value::Float32(f32::NAN)),
        ] {
            let mut n = n.clone();
            n.object
                .fields
                .0
                .iter_mut()
                .find(|(key, _)| key == field)
                .unwrap()
                .1 = value;
            assert!(extract(&[n]).is_err());
        }
    }
}
