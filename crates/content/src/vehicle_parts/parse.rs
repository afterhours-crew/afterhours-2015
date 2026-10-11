// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::items::{Guid, MAX_DEFINITIONS};
use nfs_world::replication::Error;

type Parsed = (
    customization::Definitions,
    nos::Definitions,
    wheel_customization::Definitions,
    appearance::Definitions,
    mesh::Definitions,
);
pub(super) fn parse(p: &Value, classes: &ItemCatalog) -> Result<Parsed, ParseError> {
    let mut tune = Vec::new();
    let mut custom = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for row in array(&p["customization"], MAX_DEFINITIONS)? {
        keys(row, &["guid", "class_name", "tuning", "chassis"], &[])?;
        let id = guid(&row["guid"])?;
        check_class(row, classes, id)?;
        if !seen.insert(id) {
            return Err(ParseError::Invalid);
        }
        let t = &row["tuning"];
        let obj = t.as_object().ok_or(ParseError::Invalid)?;
        if !obj.is_empty() {
            keys(t, &["asset_present"], &["transmission_values"])?;
            let transmission_values = if let Some(v) = t.get("transmission_values") {
                Some(
                    array(v, 2)?
                        .iter()
                        .map(uint)
                        .collect::<Result<Vec<u8>, _>>()?
                        .try_into()
                        .map_err(|_| ParseError::Invalid)?,
                )
            } else {
                None
            };
            tune.push((
                id,
                tuning::PartInput {
                    asset_present: boolean(&t["asset_present"])?,
                    transmission_values,
                },
            ));
        }
        let d = &row["chassis"];
        let input = match d["family"].as_str() {
            Some("other") => {
                keys(d, &["family"], &[])?;
                customization::Input::Other
            }
            Some("performance") => {
                keys(
                    d,
                    &["family", "performance_value"],
                    &["list_a", "list_b", "index"],
                )?;
                customization::Input::Performance {
                    value: float(&d["performance_value"])?,
                    group_a: optional_byte(d, "list_a")?,
                    clutch_group: optional_byte(d, "list_b")?,
                    induction: optional_byte(d, "index")?,
                }
            }
            Some("customization") => {
                keys(
                    d,
                    &["family", "flag"],
                    &["list_a", "list_b", "rim_values", "rear"],
                )?;
                let rim = if let Some(v) = d.get("rim_values") {
                    Some(customization::Rim {
                        rear: boolean(&d["rear"])?,
                        values: array(v, 256)?.iter().map(float).collect::<Result<_, _>>()?,
                    })
                } else {
                    if d.get("rear").is_some() {
                        return Err(ParseError::Invalid);
                    }
                    None
                };
                customization::Input::Appearance {
                    spoiler_group: optional_byte(d, "list_a")?,
                    group_b: optional_byte(d, "list_b")?,
                    flag: boolean(&d["flag"])?,
                    rim,
                }
            }
            _ => return Err(ParseError::Invalid),
        };
        custom.push((id, input));
    }
    let custom = model(customization::Definitions::new(
        model(tuning::Definitions::new(classes.clone(), tune))?,
        custom,
    ))?;
    let ns = array(&p["nos"], MAX_DEFINITIONS)?
        .iter()
        .map(|r| {
            keys(r, &["guid", "variant"], &[])?;
            Ok((guid(&r["guid"])?, uint(&r["variant"])?))
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    let nos = model(nos::Definitions::new(classes.clone(), ns))?;
    let wheels = array(&p["wheels"], MAX_DEFINITIONS)?
        .iter()
        .map(|r| {
            let id = guid(&r["guid"])?;
            check_class(r, classes, id)?;
            use wheel_customization::{Geometry, Input, Rim};
            let geometry = match r["class_name"].as_str() {
                Some("RimsItemData") => {
                    keys(
                        r,
                        &[
                            "guid",
                            "class_name",
                            "rear",
                            "diameter",
                            "width",
                            "scale",
                            "sizes",
                            "flag",
                        ],
                        &[],
                    )?;
                    Geometry::Rim(Rim {
                        diameter: float(&r["diameter"])?,
                        width: float(&r["width"])?,
                        scale: float(&r["scale"])?,
                        sizes: array(&r["sizes"], 256)?
                            .iter()
                            .map(float)
                            .collect::<Result<_, _>>()?,
                        flag: boolean(&r["flag"])?,
                    })
                }
                Some("TiresItemData") => {
                    keys(r, &["guid", "class_name", "rear", "alternatives"], &[])?;
                    Geometry::Tire(
                        array(&r["alternatives"], 256)?
                            .iter()
                            .map(floats)
                            .collect::<Result<_, _>>()?,
                    )
                }
                Some("FendersItemData") => {
                    keys(r, &["guid", "class_name", "rear", "width", "enabled"], &[])?;
                    Geometry::Fender {
                        width: float(&r["width"])?,
                        enabled: boolean(&r["enabled"])?,
                    }
                }
                _ => return Err(ParseError::Invalid),
            };
            Ok((
                id,
                Input {
                    rear: boolean(&r["rear"])?,
                    geometry,
                },
            ))
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    let wheels = model(wheel_customization::Definitions::new(
        classes.clone(),
        wheels,
    ))?;
    let appearance = array(&p["appearance"], MAX_DEFINITIONS)?
        .iter()
        .map(|r| {
            use appearance::Input;
            let id = guid(&r["guid"])?;
            check_class(r, classes, id)?;
            let signed = |v: &Value| {
                i32::try_from(v.as_i64().ok_or(ParseError::Invalid)?)
                    .map_err(|_| ParseError::Invalid)
            };
            let input = match r["class_name"].as_str() {
                Some("RimsItemData") => {
                    keys(r, &["guid", "class_name", "rear"], &[])?;
                    Input::Rim {
                        rear: boolean(&r["rear"])?,
                    }
                }
                Some("LicensePlateBackgroundItemData") => {
                    keys(r, &["guid", "class_name", "indices"], &[])?;
                    let [n] = array(&r["indices"], 1)? else {
                        return Err(ParseError::Invalid);
                    };
                    Input::Plate(signed(n)?)
                }
                Some("LicensePlateFrameItemData") => {
                    keys(r, &["guid", "class_name", "indices"], &[])?;
                    let [a, b] = array(&r["indices"], 2)? else {
                        return Err(ParseError::Invalid);
                    };
                    Input::Frame([signed(a)?, signed(b)?])
                }
                Some("StaticLiveryCustomizationItemData") => {
                    keys(r, &["guid", "class_name", "indices"], &[])?;
                    let [a, b] = array(&r["indices"], 2)? else {
                        return Err(ParseError::Invalid);
                    };
                    Input::StaticWrap([uint(a)?, uint(b)?])
                }
                Some("LiveryCustomizationItemData") => {
                    keys(r, &["guid", "class_name", "data_id", "version"], &[])?;
                    Input::DynamicWrap {
                        data_id: text(&r["data_id"])?
                            .parse()
                            .map_err(|_| ParseError::Invalid)?,
                        version: signed(&r["version"])?,
                    }
                }
                _ => return Err(ParseError::Invalid),
            };
            Ok((id, input))
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    let appearance = model(appearance::Definitions::new(classes.clone(), appearance))?;
    let meshes = array(&p["mesh"], MAX_DEFINITIONS)?
        .iter()
        .map(|r| {
            let id = guid(&r["guid"])?;
            check_class(r, classes, id)?;
            let input = match r["kind"].as_str() {
                Some("ignore") => {
                    keys(r, &["guid", "class_name", "kind"], &[])?;
                    mesh::Input::Ignore
                }
                Some("part") => {
                    keys(r, &["guid", "class_name", "kind", "guids", "rim"], &[])?;
                    mesh::Input::Part {
                        guids: guids(&r["guids"])?,
                        rim: if r["rim"].is_null() {
                            None
                        } else {
                            Some(floats(&r["rim"])?)
                        },
                    }
                }
                Some("tire") => {
                    keys(
                        r,
                        &["guid", "class_name", "kind", "rear", "alternatives"],
                        &[],
                    )?;
                    mesh::Input::Tire {
                        rear: boolean(&r["rear"])?,
                        alternatives: array(&r["alternatives"], 256)?
                            .iter()
                            .map(guids)
                            .collect::<Result<_, _>>()?,
                    }
                }
                _ => return Err(ParseError::Invalid),
            };
            Ok((id, input))
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    Ok((
        custom,
        nos,
        wheels,
        appearance,
        model(mesh::Definitions::new(classes.clone(), meshes))?,
    ))
}
fn optional_byte(v: &Value, k: &str) -> Result<Option<u8>, ParseError> {
    v.get(k).map(uint).transpose()
}
fn check_class(row: &Value, catalog: &ItemCatalog, id: Guid) -> Result<(), ParseError> {
    if row["class_name"].as_str()
        != Some(catalog.class(&id).map_err(|_| ParseError::Invalid)?.name())
    {
        return Err(ParseError::Invalid);
    }
    Ok(())
}
fn guids(v: &Value) -> Result<Vec<Guid>, ParseError> {
    array(v, 256)?.iter().map(guid).collect()
}

pub(super) fn keys(v: &Value, required: &[&str], optional: &[&str]) -> Result<(), ParseError> {
    let obj = v.as_object().ok_or(ParseError::Invalid)?;
    if required.iter().any(|k| !obj.contains_key(*k))
        || obj
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err(ParseError::Invalid);
    }
    Ok(())
}
pub(super) fn array(v: &Value, max: usize) -> Result<&[Value], ParseError> {
    let a = v.as_array().ok_or(ParseError::Invalid)?;
    if a.len() > max {
        return Err(ParseError::Limit);
    }
    Ok(a)
}
pub(super) fn uint<T: TryFrom<u64>>(v: &Value) -> Result<T, ParseError> {
    T::try_from(v.as_u64().ok_or(ParseError::Invalid)?).map_err(|_| ParseError::Invalid)
}
pub(super) fn boolean(v: &Value) -> Result<bool, ParseError> {
    v.as_bool().ok_or(ParseError::Invalid)
}
pub(super) fn float(v: &Value) -> Result<f32, ParseError> {
    let n = v.as_f64().ok_or(ParseError::Invalid)? as f32;
    n.is_finite().then_some(n).ok_or(ParseError::Invalid)
}
pub(super) fn floats<const N: usize>(v: &Value) -> Result<[f32; N], ParseError> {
    array(v, N)?
        .iter()
        .map(float)
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| ParseError::Invalid)
}
pub(super) fn text(v: &Value) -> Result<&str, ParseError> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.contains('\0'))
        .ok_or(ParseError::Invalid)
}
pub(super) fn guid(v: &Value) -> Result<Guid, ParseError> {
    let s = text(v)?;
    if s.len() != 32 || !s.is_ascii() {
        return Err(ParseError::Invalid);
    }
    let mut bytes = [0; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| ParseError::Invalid)?;
    }
    Ok(bytes)
}
pub(super) fn model<T>(r: Result<T, Error>) -> Result<T, ParseError> {
    r.map_err(|_| ParseError::Invalid)
}
