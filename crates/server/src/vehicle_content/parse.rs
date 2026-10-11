// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

pub(super) fn keys(v: &Value, required: &[&str], optional: &[&str]) -> Result<(), Failure> {
    let obj = v.as_object().ok_or(Failure::ProfileConfig)?;
    if required.iter().any(|k| !obj.contains_key(*k))
        || obj
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err(Failure::ProfileConfig);
    }
    Ok(())
}
pub(super) fn array(v: &Value, max: usize) -> Result<&[Value], Failure> {
    let a = v.as_array().ok_or(Failure::ProfileConfig)?;
    if a.len() > max {
        return Err(Failure::BodyLimit);
    }
    Ok(a)
}
pub(super) fn uint<T: TryFrom<u64>>(v: &Value) -> Result<T, Failure> {
    T::try_from(v.as_u64().ok_or(Failure::ProfileConfig)?).map_err(|_| Failure::ProfileConfig)
}
pub(super) fn boolean(v: &Value) -> Result<bool, Failure> {
    v.as_bool().ok_or(Failure::ProfileConfig)
}
pub(super) fn float(v: &Value) -> Result<f32, Failure> {
    let n = v.as_f64().ok_or(Failure::ProfileConfig)? as f32;
    n.is_finite().then_some(n).ok_or(Failure::ProfileConfig)
}
pub(super) fn floats<const N: usize>(v: &Value) -> Result<[f32; N], Failure> {
    array(v, N)?
        .iter()
        .map(float)
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| Failure::ProfileConfig)
}
pub(super) fn text(v: &Value) -> Result<&str, Failure> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.contains('\0'))
        .ok_or(Failure::ProfileConfig)
}
pub(super) fn guid(v: &Value) -> Result<Guid, Failure> {
    let s = text(v)?;
    if s.len() != 32 || !s.is_ascii() {
        return Err(Failure::ProfileConfig);
    }
    let mut bytes = [0; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| Failure::ProfileConfig)?;
    }
    Ok(bytes)
}
pub(super) fn model<T>(r: Result<T, Error>) -> Result<T, Failure> {
    r.map_err(|_| Failure::ProfileConfig)
}
pub(super) fn mesh_kind(v: &Value) -> Result<MeshKind, Failure> {
    Ok(MeshKind {
        index_bits: uint(&v["index_bits"])?,
        extra: match v["extra"].as_str() {
            Some("none") => MeshExtra::None,
            Some("rim") => MeshExtra::Rim,
            Some("light") => MeshExtra::Light,
            _ => return Err(Failure::ProfileConfig),
        },
    })
}
fn profile(v: &Value) -> Result<Profile, Failure> {
    let mut kinds = Vec::new();
    for k in array(v, MAX_SERIALIZERS)? {
        let (kind, extra) = match k["kind"].as_str() {
            Some("root") => (
                Kind::Root {
                    property_owner: boolean(&k["property_owner"])?,
                },
                Some("property_owner"),
            ),
            Some("part") => (
                Kind::Part {
                    variants: uint(&k["variants"])?,
                },
                Some("variants"),
            ),
            Some("mesh") => {
                let meshes = array(&k["entries"], MAX_MESHES)?
                    .iter()
                    .map(|m| {
                        keys(m, &["index_bits", "extra"], &[])?;
                        mesh_kind(m)
                    })
                    .collect::<Result<_, Failure>>()?;
                (Kind::Mesh(meshes), Some("entries"))
            }
            Some("chassis") => (Kind::Chassis, None),
            Some("triple_nibbles") => (Kind::TripleNibbles, None),
            Some("bool") => (Kind::Bool, None),
            Some("wheel") => (Kind::Wheel, None),
            Some("index") => (Kind::Index, None),
            Some("tuning") => (Kind::Tuning, None),
            Some("tagged") => (Kind::Tagged, None),
            Some("nibbles_guid") => (Kind::NibblesGuid, None),
            Some("four_bit") => (Kind::FourBit, None),
            Some("appearance") => (Kind::Appearance, None),
            Some("guid_float") => (Kind::GuidFloat, None),
            _ => return Err(Failure::ProfileConfig),
        };
        keys(k, &["kind"], &extra.into_iter().collect::<Vec<_>>())?;
        kinds.push(kind);
    }
    model(Profile::new(kinds))
}
pub(super) fn definition(v: &Value) -> Result<Definition, Failure> {
    keys(
        v,
        &[
            "definitions",
            "blueprint",
            "bundle",
            "asset",
            "catalog",
            "profile",
            "parts",
            "buses",
            "resting",
            "team",
            "mode",
            "wheel_baseline",
            "appearance",
            "meshes",
            "health",
        ],
        &[],
    )?;
    let profile = profile(&v["profile"])?;
    for kind in [
        Kind::Chassis,
        Kind::TripleNibbles,
        Kind::Bool,
        Kind::Index,
        Kind::Tuning,
        Kind::Tagged,
        Kind::NibblesGuid,
        Kind::Appearance,
        Kind::GuidFloat,
    ] {
        if profile.kinds().iter().filter(|k| **k == kind).count() != 1 {
            return Err(Failure::ProfileConfig);
        }
    }
    if profile
        .kinds()
        .iter()
        .filter(|k| matches!(k, Kind::Wheel))
        .count()
        != 4
        || profile
            .kinds()
            .iter()
            .filter(|k| matches!(k, Kind::Mesh(_)))
            .count()
            != 1
    {
        return Err(Failure::ProfileConfig);
    }
    let mut parts = BTreeMap::new();
    for row in array(&v["parts"], MAX_SERIALIZERS)? {
        keys(row, &["ordinal", "variants", "networkable"], &[])?;
        let ordinal = uint(&row["ordinal"])?;
        if profile.kinds().get(ordinal)
            != Some(&Kind::Part {
                variants: uint(&row["variants"])?,
            })
            || parts
                .insert(ordinal, boolean(&row["networkable"])?)
                .is_some()
        {
            return Err(Failure::ProfileConfig);
        }
    }
    if parts.is_empty()
        || parts.len()
            != profile
                .kinds()
                .iter()
                .filter(|k| matches!(k, Kind::Part { .. }))
                .count()
    {
        return Err(Failure::ProfileConfig);
    }
    let buses = array(&v["buses"], nfs_world::replication::MAX_OBJECTS)?
        .iter()
        .map(|b| {
            keys(b, &["path", "flags"], &[])?;
            Ok(root::Bus {
                path: array(&b["path"], 31)?
                    .iter()
                    .map(uint)
                    .collect::<Result<_, _>>()?,
                flags: uint(&b["flags"])?,
            })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    let resting = resting(&v["resting"])?;
    model(resting.position_at([0.; 3]))?;
    let team = uint(&v["team"])?;
    let mode = uint(&v["mode"])?;
    if team > 16 || mode > 7 {
        return Err(Failure::ProfileConfig);
    }
    let wheel_baseline = floats(&v["wheel_baseline"])?;
    if wheel_baseline
        .iter()
        .any(|v| *v <= 0. || !(1. / v).is_finite())
    {
        return Err(Failure::ProfileConfig);
    }
    let app = &v["appearance"];
    keys(app, &["palettes", "static_wrap_counts"], &[])?;
    let mut palettes = [[[0.; 3]; 4]; 2];
    if array(&app["palettes"], 2)?.len() != 2 {
        return Err(Failure::ProfileConfig);
    }
    for (out, row) in palettes.iter_mut().zip(app["palettes"].as_array().unwrap()) {
        if array(row, 4)?.len() != 4 {
            return Err(Failure::ProfileConfig);
        }
        for (o, r) in out.iter_mut().zip(row.as_array().unwrap()) {
            *o = floats(r)?;
        }
    }
    let counts = &app["static_wrap_counts"];
    let static_wrap_counts: Option<[u16; 2]> = if counts.is_null() {
        None
    } else {
        let a = array(counts, 2)?
            .iter()
            .map(uint)
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| Failure::ProfileConfig)?;
        Some(a)
    };
    if static_wrap_counts.is_some_and(|v| v.into_iter().any(|n| n > 256)) {
        return Err(Failure::ProfileConfig);
    }
    let meshes = parts::components(&v["meshes"])?;
    let Some(Kind::Mesh(expected)) = profile.kinds().iter().find(|k| matches!(k, Kind::Mesh(_)))
    else {
        return Err(Failure::ProfileConfig);
    };
    if meshes.iter().map(|m| &m.kind).ne(expected) {
        return Err(Failure::ProfileConfig);
    }
    keys(&v["asset"], &["type_id", "local_index"], &[])?;
    let asset_type = uint(&v["asset"]["type_id"])?;
    let asset_index = uint(&v["asset"]["local_index"])?;
    let catalog = array(&v["catalog"], 4096)?
        .iter()
        .map(|r| {
            let [a, b] = array(r, 2)? else {
                return Err(Failure::ProfileConfig);
            };
            Ok((uint(a)?, uint(b)?))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    model(nfs_world::replication::entity::creation::Catalog::new(
        1, &catalog,
    ))?;
    if !catalog
        .iter()
        .any(|(t, n)| *t == asset_type && asset_index < *n)
    {
        return Err(Failure::ProfileConfig);
    }
    Ok(Definition {
        blueprint: text(&v["blueprint"])?.to_owned(),
        bundle: text(&v["bundle"])?.to_owned(),
        asset_type,
        asset_index,
        catalog,
        profile,
        root: model(root::Blueprint::new(buses))?,
        resting,
        team,
        mode,
        parts,
        wheel_baseline,
        appearance: appearance::Defaults {
            palettes,
            static_wrap_counts,
        },
        meshes,
        health: health(&v["health"])?,
    })
}
fn resting(v: &Value) -> Result<placement::RestingConfig, Failure> {
    keys(
        v,
        &["mass", "front_axle", "wheelbase", "curve", "front", "rear"],
        &[],
    )?;
    keys(&v["curve"], &["min", "max", "points"], &[])?;
    let axle = |name| {
        let [ride_height_inches, spring_rate, spring_progression] = floats(&v[name])?;
        Ok(placement::Suspension {
            ride_height_inches,
            spring_rate,
            spring_progression,
        })
    };
    let points = array(&v["curve"]["points"], 8)?
        .iter()
        .map(floats)
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| Failure::ProfileConfig)?;
    Ok(placement::RestingConfig {
        mass: float(&v["mass"])?,
        front_axle: float(&v["front_axle"])?,
        wheelbase: float(&v["wheelbase"])?,
        front_weight_bias_percent: placement::Curve {
            min: floats(&v["curve"]["min"])?,
            max: floats(&v["curve"]["max"])?,
            points,
        },
        front: axle("front")?,
        rear: axle("rear")?,
    })
}
fn health(v: &Value) -> Result<health::State, Failure> {
    keys(v, &["maximum", "profile"], &[])?;
    let p = &v["profile"];
    let profile = if p.is_null() {
        None
    } else {
        keys(p, &["guid", "points"], &[])?;
        let g = guid(&p["guid"])?;
        let words =
            std::array::from_fn(|i| u32::from_be_bytes(g[i * 4..i * 4 + 4].try_into().unwrap()));
        let points = array(&p["points"], 256)?
            .iter()
            .map(|v| {
                i32::try_from(v.as_i64().ok_or(Failure::ProfileConfig)?)
                    .map_err(|_| Failure::ProfileConfig)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Some(model(health::Profile::new(words, &points))?)
    };
    model(health::State::new(float(&v["maximum"])?, profile))
}
