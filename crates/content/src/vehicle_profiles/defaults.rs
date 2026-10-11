// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{
    fields::*,
    graph::{Node, unique},
};
use crate::{Error, scan::Assets};
use nfs_frostbite::ebx::Pointer;
use serde_json::{Value as Json, json};

pub(super) fn build(assets: &mut Assets, rows: &[Node]) -> Result<Json, Error> {
    let entity = &unique(rows, "RaceVehicleEntityData")?.object.fields;
    let component = unique(rows, "RaceVehicleComponentData")?;
    let (config_key, config) = follow(
        assets,
        &component.key.0,
        &component.object.fields,
        "RaceVehicleConfig",
        "RaceVehicleConfigData",
    )?;
    let (_, chassis) = follow(
        assets,
        &config_key.0,
        &config.fields,
        "Chassis",
        "RaceVehicleChassisConfigData",
    )?;
    let (_, suspension) = follow(
        assets,
        &config_key.0,
        &config.fields,
        "Suspension",
        "RaceVehicleSuspensionConfigData",
    )?;
    let curve = array(&chassis.fields, "FrontWeightBias", 10)?;
    if curve.len() != 10 {
        return Err(bad("front weight curve length"));
    }
    let curve = curve
        .iter()
        .map(|v| {
            let f = v
                .as_fields()
                .ok_or_else(|| bad("front weight curve vector"))?;
            Ok([float(f, "x")?, float(f, "y")?])
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let axle = |suffix: &str| -> Result<[f32; 3], Error> {
        Ok([
            float(&suspension.fields, &format!("RideHeightSpecs{suffix}"))?,
            float(&suspension.fields, &format!("SpringSpecs{suffix}"))?,
            float(&suspension.fields, &format!("SpringProgression{suffix}"))?,
        ])
    };
    let resting = json!({"mass":float(&chassis.fields,"Mass")?,
        "front_axle":float(&chassis.fields,"FrontAxle")?,"wheelbase":float(&chassis.fields,"WheelBase")?,
        "curve":{"min":curve[0],"max":curve[1],"points":curve[2..]},
        "front":axle("Front")?,"rear":axle("Rear")?});
    let wheel = &unique(rows, "WheelCustomizationComponentData")?
        .object
        .fields;
    let app = unique(rows, "RaceVehicleCustomizationComponentData")?;
    let mut palettes = [[[0.; 3]; 4]; 2];
    for (out, axle) in palettes.iter_mut().zip(["Front", "Rear"]) {
        for (slot, (paint, value)) in out.iter_mut().zip([
            ("Primary", "Color"),
            ("Primary", "Material"),
            ("Secondary", "Color"),
            ("Secondary", "Material"),
        ]) {
            *slot = vector(
                &app.object.fields,
                &format!("Default{paint}Paint{axle}Rims{value}"),
                ["x", "y", "z"],
            )?;
        }
    }
    let wrap_counts = if pointer(&app.object.fields, "StaticLiveryTextures")? == Pointer::Null {
        Json::Null
    } else {
        let (_, wraps) = follow(
            assets,
            &app.key.0,
            &app.object.fields,
            "StaticLiveryTextures",
            "StaticLiveryTexturesAsset",
        )?;
        json!([
            array(&wraps.fields, "DiffuseTextures", 256)?.len(),
            array(&wraps.fields, "HighlightTextures", 256)?.len()
        ])
    };
    let health = unique(rows, "RaceVehicleHealthComponentData")?;
    let profile = if pointer(&health.object.fields, "PlayerHealthProfile")? == Pointer::Null {
        Json::Null
    } else {
        let (_, profile) = follow(
            assets,
            &health.key.0,
            &health.object.fields,
            "PlayerHealthProfile",
            "RaceVehicleHealthProfileConfigData",
        )?;
        let points = array(&profile.fields, "Segments", 256)?
            .iter()
            .map(|v| integer(v.as_fields().ok_or_else(|| bad("health segment"))?, "Size"))
            .collect::<Result<Vec<_>, _>>()?;
        json!({"guid":nfs_frostbite::hex(&profile.guid.ok_or_else(|| bad("unexported health profile"))?),"points":points})
    };
    Ok(
        json!({"resting":resting,"team":enumeration(entity,"DefaultTeam")?,
        "mode":enumeration(&config.fields,"VehicleModeAtReset")?,
        "wheel_baseline":[float(wheel,"FrontWheelDiameter")?,float(wheel,"RearWheelDiameter")?],
        "appearance":{"palettes":palettes,"static_wrap_counts":wrap_counts},
        "health":{"maximum":float(&health.object.fields,"MaxHealth")?,"profile":profile}}),
    )
}
