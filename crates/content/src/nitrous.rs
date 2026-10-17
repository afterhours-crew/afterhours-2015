// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed nitrous definitions. Curves and action boosts are read from the
//! owner's installation, never inferred from item prices or shipped as assets.
use crate::{Error, assets::AssetIndex, graph::bad};
use nfs_frostbite::ebx::{Fields, Object, Value};
use nfs_world::{
    items::Guid,
    nitrous_fuel::{Boosts, Configuration, Curve},
};
use std::{collections::BTreeMap, sync::Arc};

pub type Catalog = BTreeMap<Guid, Arc<Configuration>>;
fn curve(value: &Value) -> Result<Curve, Error> {
    let rows = value
        .as_array()
        .filter(|r| r.len() == 10)
        .ok_or_else(|| bad("nitrous eight-knot curve"))?;
    let vectors = rows
        .iter()
        .map(|r| {
            let f = r.as_fields().ok_or_else(|| bad("nitrous curve vector"))?;
            Ok([scalar(f, "x")?, scalar(f, "y")?, scalar(f, "z")?])
        })
        .collect::<Result<Vec<_>, Error>>()?;
    if vectors[1][2] != 8.0 {
        return Err(bad("nitrous curve count header"));
    }
    let [x0, y0, _] = vectors[0];
    let [x1, y1, _] = vectors[1];
    Curve::new(
        vectors[2..]
            .iter()
            .map(|v| [(x1 - x0) * v[0] + x0, (y1 - y0) * v[1] + y0])
            .collect(),
    )
    .map_err(|e| bad(&format!("nitrous curve: {e}")))
}
fn field<'a>(f: &'a Fields, name: &str) -> Result<&'a Value, Error> {
    f.get(name).ok_or_else(|| bad("nitrous field missing"))
}
fn scalar(f: &Fields, name: &str) -> Result<f32, Error> {
    match field(f, name)? {
        Value::Float32(v) if v.is_finite() => Ok(*v),
        _ => Err(bad("nitrous finite float")),
    }
}
pub fn compile(config: &Object, buildup: &Object) -> Result<Configuration, Error> {
    if config.class != "RaceVehicleNOSConfigData"
        || buildup.class != "RaceVehicleNOSBuildupConfigData"
    {
        return Err(bad("nitrous configuration class"));
    }
    let f = &config.fields;
    let b = &buildup.fields;
    Configuration::new(
        scalar(f, "Capacity")?,
        curve(field(f, "FlowRate")?)?,
        curve(field(f, "RechargeRate")?)?,
        Boosts {
            drift: scalar(b, "DriftBoost")?,
            draft: scalar(b, "DraftBoost")?,
            near_miss: scalar(b, "NearMissBoost")?,
            oncoming: scalar(b, "WrongLaneBoost")?,
            airborne: scalar(b, "AirborneBoost")?,
            cop_slam: scalar(b, "SlamCopBoost")?,
            draft_speed: scalar(b, "DraftSpeedThreshold")?,
            oncoming_speed: scalar(b, "WrongLaneSpeedThreshold")?,
            airborne_speed: scalar(b, "AirborneSpeedThreshold")?,
        },
    )
    .map_err(|e| bad(&format!("nitrous: {e}")))
}
impl AssetIndex {
    pub fn nitrous_configurations(&mut self) -> Result<Catalog, Error> {
        let names = self.assets.named("RaceVehicleNOSUpgradesData");
        // The supported installation has one shared list. Ambiguity must not
        // silently select another car's ordinal-to-configuration mapping.
        if names.len() != 1 {
            return Err(bad("nitrous upgrade list count"));
        }
        let list = &names[0];
        let root = self
            .assets
            .objects(list)?
            .first()
            .cloned()
            .ok_or_else(|| bad("nitrous list root"))?;
        let rows = field(&root.fields, "NOSUpgrades")?
            .as_array()
            .filter(|r| !r.is_empty() && r.len() <= 16)
            .ok_or_else(|| bad("nitrous list bound"))?;
        let mut configs = Vec::new();
        for row in rows {
            let target = self
                .assets
                .target(
                    list,
                    row.as_pointer()
                        .ok_or_else(|| bad("nitrous list pointer"))?,
                )?
                .ok_or_else(|| bad("null nitrous configuration"))?;
            configs.push(self.assets.objects(&target.0)?[target.1].clone());
        }
        let items = self.assets.named("NosTuningItemData");
        if items.len() > nfs_world::items::MAX_DEFINITIONS {
            return Err(bad("nitrous definition bound"));
        }
        let mut result = Catalog::new();
        for name in items {
            let item = self
                .assets
                .objects(&name)?
                .first()
                .cloned()
                .ok_or_else(|| bad("nitrous item root"))?;
            let index = match field(&item.fields, "NOSUpgradeIndex")? {
                Value::UInt32(v) => *v as usize,
                Value::Int32(v) if *v >= 0 => *v as usize,
                _ => return Err(bad("nitrous upgrade index")),
            };
            let config = configs
                .get(index)
                .ok_or_else(|| bad("nitrous index outside list"))?;
            let target = self
                .assets
                .target(
                    &name,
                    field(&item.fields, "NOSBuildupConfig")?
                        .as_pointer()
                        .ok_or_else(|| bad("nitrous buildup pointer"))?,
                )?
                .ok_or_else(|| bad("null nitrous buildup"))?;
            let buildup = self.assets.objects(&target.0)?[target.1].clone();
            let guid = item.guid.ok_or_else(|| bad("nitrous item GUID"))?;
            if result
                .insert(guid, Arc::new(compile(config, &buildup)?))
                .is_some()
            {
                return Err(bad("duplicate nitrous GUID"));
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vector(x: f32, y: f32, z: f32) -> Value {
        Value::Struct(Fields(vec![
            ("x".into(), Value::Float32(x)),
            ("y".into(), Value::Float32(y)),
            ("z".into(), Value::Float32(z)),
        ]))
    }
    fn rows() -> Value {
        let mut v = vec![vector(0.0, 0.6, 1.0), vector(70.0, 0.2, 8.0)];
        v.extend((0..8).map(|n| vector(n as f32 / 7.0, n as f32 / 7.0, 99.0)));
        Value::Array(v)
    }
    #[test]
    fn vehicle_curve_expands_descending_endpoints_and_ignores_cubic_coefficients() {
        let c = curve(&rows()).unwrap();
        assert_eq!(c.evaluate(-5.0).unwrap(), 0.6);
        assert!((c.evaluate(35.0).unwrap() - 0.4).abs() < 1e-6);
        assert!((c.evaluate(100.0).unwrap() - 0.2).abs() < 1e-6);
        let Value::Array(mut v) = rows() else {
            panic!()
        };
        v[1] = vector(70.0, 0.2, 7.0);
        assert!(curve(&Value::Array(v.clone())).is_err());
        v.pop();
        assert!(curve(&Value::Array(v)).is_err());
    }
    #[test]
    fn configuration_requires_typed_complete_fields() {
        let mut c = Object {
            class: "RaceVehicleNOSConfigData".into(),
            guid: None,
            fields: Fields(vec![
                ("Capacity".into(), Value::Float32(9.0)),
                ("FlowRate".into(), rows()),
                ("RechargeRate".into(), rows()),
            ]),
        };
        let mut b = Object {
            class: "RaceVehicleNOSBuildupConfigData".into(),
            guid: None,
            fields: Fields(
                [
                    "DriftBoost",
                    "DraftBoost",
                    "NearMissBoost",
                    "WrongLaneBoost",
                    "AirborneBoost",
                    "SlamCopBoost",
                    "DraftSpeedThreshold",
                    "WrongLaneSpeedThreshold",
                    "AirborneSpeedThreshold",
                ]
                .map(|n| (n.into(), Value::Float32(0.0)))
                .to_vec(),
            ),
        };
        assert_eq!(compile(&c, &b).unwrap().capacity(), 9.0);
        c.fields.0[0].1 = Value::Float32(f32::NAN);
        assert!(compile(&c, &b).is_err());
        c.fields.0[0].1 = Value::Float32(9.0);
        b.fields.0.pop();
        assert!(compile(&c, &b).is_err());
    }
}
