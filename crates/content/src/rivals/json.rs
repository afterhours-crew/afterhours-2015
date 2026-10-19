// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{preset::*, *};
use serde_json::{Value as Json, json};
use std::collections::BTreeSet;

const MAX_BYTES: u64 = 16 << 20;
const MAX_POINTS: usize = 131072;

fn keys(v: &Json, names: &[&str]) -> Result<(), Error> {
    if v.as_object()
        .is_none_or(|o| o.len() != names.len() || names.iter().any(|n| !o.contains_key(*n)))
    {
        return Err(bad("JSON fields"));
    }
    Ok(())
}
fn array(v: &Json, max: usize) -> Result<&[Json], Error> {
    v.as_array()
        .filter(|a| a.len() <= max)
        .map(Vec::as_slice)
        .ok_or_else(|| bad("array bound"))
}
fn number<T: TryFrom<u64>>(v: &Json) -> Result<T, Error> {
    T::try_from(v.as_u64().ok_or_else(|| bad("integer"))?).map_err(|_| bad("integer range"))
}
fn float(v: &Json) -> Result<f32, Error> {
    v.as_f64()
        .map(|v| v as f32)
        .filter(|v| v.is_finite() && v.abs() <= 1_000_000.)
        .ok_or_else(|| bad("finite float"))
}
fn fixed<T, const N: usize>(
    v: &Json,
    read: impl Fn(&Json) -> Result<T, Error>,
) -> Result<[T; N], Error> {
    array(v, N)?
        .iter()
        .map(read)
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| bad("array length"))
}
fn vector(v: &Json) -> Result<[f32; 3], Error> {
    fixed(v, float)
}
fn optional<T>(v: &Json, read: impl FnOnce(&Json) -> Result<T, Error>) -> Result<Option<T>, Error> {
    if v.is_null() {
        Ok(None)
    } else {
        read(v).map(Some)
    }
}
fn guid(v: &Json) -> Result<[u8; 16], Error> {
    let s = v
        .as_str()
        .filter(|s| s.len() == 32 && s.is_ascii())
        .ok_or_else(|| bad("GUID"))?;
    let mut g = [0; 16];
    for (i, b) in g.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| bad("GUID"))?;
    }
    if g == [0; 16] {
        return Err(bad("null GUID"));
    }
    Ok(g)
}
fn parts(v: &Json) -> Result<Vec<[u8; 16]>, Error> {
    array(v, MAX_PARTS)?.iter().map(guid).collect()
}
fn rim(v: &Json) -> Result<RimStyle, Error> {
    keys(
        v,
        &[
            "selection",
            "primary_color",
            "primary_material",
            "secondary_color",
            "secondary_material",
        ],
    )?;
    Ok(RimStyle {
        selection: optional(&v["selection"], number)?,
        primary_color: optional(&v["primary_color"], vector)?,
        primary_material: optional(&v["primary_material"], vector)?,
        secondary_color: optional(&v["secondary_color"], vector)?,
        secondary_material: optional(&v["secondary_material"], vector)?,
    })
}
fn style(v: &Json) -> Result<Style, Error> {
    keys(
        v,
        &[
            "parts",
            "color",
            "material",
            "window_tint",
            "license_plate",
            "alignment",
            "rims",
        ],
    )?;
    let style = Style {
        parts: parts(&v["parts"])?,
        color: optional(&v["color"], vector)?,
        material: optional(&v["material"], vector)?,
        window_tint: optional(&v["window_tint"], float)?,
        license_plate: optional(&v["license_plate"], |v| fixed(v, number))?,
        alignment: fixed(&v["alignment"], |v| optional(v, number))?,
        rims: fixed(&v["rims"], rim)?,
    };
    style.validate()?;
    Ok(style)
}
fn preset(v: &Json) -> Result<Preset, Error> {
    keys(v, &["vehicle", "color", "style", "additional"])?;
    Ok(Preset {
        vehicle: guid(&v["vehicle"])?,
        color: optional(&v["color"], vector)?,
        style: optional(&v["style"], style)?,
        additional: parts(&v["additional"])?,
    })
}
fn regions(v: &Json, total: &mut usize) -> Result<Vec<Region>, Error> {
    array(v, MAX_REGIONS)?
        .iter()
        .map(|v| {
            keys(v, &["points", "height"])?;
            let points = array(&v["points"], MAX_REGION_POINTS)?;
            *total = total
                .checked_add(points.len())
                .filter(|n| *n <= MAX_POINTS)
                .ok_or_else(|| bad("total region points"))?;
            Region::new(
                points.iter().map(vector).collect::<Result<_, _>>()?,
                float(&v["height"])?,
            )
        })
        .collect()
}

impl Content {
    pub fn load(path: &std::path::Path) -> Result<Self, Error> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(bad("content byte bound"));
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| bad("JSON"))?)
    }

    pub fn from_json(v: &Json) -> Result<Self, Error> {
        keys(
            v,
            &[
                "format",
                "version",
                "build_sha256",
                "managers",
                "personality_count",
                "personas",
                "racers",
            ],
        )?;
        if v["format"] != "nfs-rival-content"
            || v["version"] != 2
            || v["build_sha256"] != nfs_services::SUPPORTED_BUILD_SHA256
        {
            return Err(bad("content identity"));
        }
        let m = &v["managers"];
        keys(m, &["scene", "participants", "actors", "alive"])?;
        let managers = nfs_world::replication::players::ai::Managers {
            alive: crate::ai::state_from_json(&m["alive"])?,
            scene: number(&m["scene"])?,
            participants: number(&m["participants"])?,
            actors: number(&m["actors"])?,
        };
        if managers.participants == managers.actors
            || (managers.alive.scene == managers.scene
                && [managers.participants, managers.actors].contains(&managers.alive.serializer))
            || [managers.participants, managers.actors]
                .into_iter()
                .any(|n| n >= nfs_world::replication::sublevel::MAX_SERIALIZERS)
        {
            return Err(bad("manager ordinals"));
        }
        let personality_count: u16 = number(&v["personality_count"])?;
        if personality_count == 0 || usize::from(personality_count) > MAX_PERSONALITIES {
            return Err(bad("personality count"));
        }
        let mut personas = Vec::new();
        for p in array(&v["personas"], MAX_RACERS)? {
            keys(
                p,
                &[
                    "name",
                    "provider_index",
                    "personality_index",
                    "preset",
                    "performance",
                    "grip",
                ],
            )?;
            let name = p["name"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 64 && !s.contains('\0'))
                .ok_or_else(|| bad("persona name"))?
                .to_owned();
            let performance = float(&p["performance"])?;
            let grip: Option<u8> = optional(&p["grip"], number)?;
            if performance < 0. || grip.is_some_and(|n| n > 10) {
                return Err(bad("driving override"));
            }
            let personality_index: u16 = number(&p["personality_index"])?;
            if personality_index != u16::MAX && personality_index >= personality_count {
                return Err(bad("personality index"));
            }
            personas.push(Persona {
                name,
                provider_index: number(&p["provider_index"])?,
                personality_index,
                preset: preset(&p["preset"])?,
                driving: nfs_world::replication::vehicle::customization::SpawnOverrides {
                    performance,
                    grip,
                },
            });
        }
        let mut racers = Vec::new();
        let mut identities = BTreeSet::new();
        let mut used = BTreeSet::new();
        let mut points = 0;
        for r in array(&v["racers"], MAX_RACERS)? {
            keys(
                r,
                &[
                    "guid",
                    "event_type",
                    "personas",
                    "regions",
                    "secondary_regions",
                    "position",
                    "basis",
                ],
            )?;
            let guid = guid(&r["guid"])?;
            let choices = array(&r["personas"], MAX_RACERS)?
                .iter()
                .map(number::<usize>)
                .collect::<Result<Vec<_>, _>>()?;
            if !identities.insert(guid)
                || choices.is_empty()
                || choices.iter().any(|&n| n >= personas.len())
            {
                return Err(bad("racer identity/providers"));
            }
            used.extend(choices.iter().copied());
            let basis = fixed(&r["basis"], vector)?;
            nfs_world::replication::vehicle::orientation::quaternion_from_basis(basis)
                .map_err(|_| bad("spawn orientation"))?;
            racers.push(Racer {
                guid,
                event_type: number(&r["event_type"])?,
                personas: choices,
                regions: regions(&r["regions"], &mut points)?,
                secondary_regions: regions(&r["secondary_regions"], &mut points)?,
                position: vector(&r["position"])?,
                basis,
            });
        }
        if racers.is_empty() || personas.is_empty() || used.len() != personas.len() {
            return Err(bad("racer/persona closure"));
        }
        Ok(Self {
            managers,
            personality_count,
            personas,
            racers,
        })
    }

    pub fn to_json(&self) -> Json {
        let region = |r: &Region| json!({"points":r.points(),"height":r.height()});
        let guids = |v: &[[u8; 16]]| v.iter().map(|g| nfs_frostbite::hex(g)).collect::<Vec<_>>();
        let style = |s: &Style| json!({"parts":guids(&s.parts),"color":s.color,"material":s.material,"window_tint":s.window_tint,"license_plate":s.license_plate,"alignment":s.alignment,"rims":s.rims.iter().map(|r| json!({"selection":r.selection,"primary_color":r.primary_color,"primary_material":r.primary_material,"secondary_color":r.secondary_color,"secondary_material":r.secondary_material})).collect::<Vec<_>>()});
        json!({"format":"nfs-rival-content","version":2,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
            "personality_count":self.personality_count, "managers":{"scene":self.managers.scene,"participants":self.managers.participants,"actors":self.managers.actors,"alive":crate::ai::state_json(self.managers.alive)},
            "personas": self.personas.iter().map(|p| json!({"name":p.name,"provider_index":p.provider_index,"personality_index":p.personality_index,"preset":{"vehicle":nfs_frostbite::hex(&p.preset.vehicle),"color":p.preset.color,"style":p.preset.style.as_ref().map(style),"additional":guids(&p.preset.additional)},"performance":p.driving.performance,"grip":p.driving.grip})).collect::<Vec<_>>(),
            "racers": self.racers.iter().map(|r| json!({"guid":nfs_frostbite::hex(&r.guid),"event_type":r.event_type,"personas":r.personas,"regions":r.regions.iter().map(region).collect::<Vec<_>>(),"secondary_regions":r.secondary_regions.iter().map(region).collect::<Vec<_>>(),"position":r.position,"basis":r.basis})).collect::<Vec<_>>()})
    }
}

pub(crate) fn build(index: &mut crate::assets::AssetIndex) -> Result<Vec<u8>, Error> {
    let content = super::extract(index)?.to_json();
    Content::from_json(&content)?;
    let bytes = serde_json::to_vec_pretty(&content).map_err(|_| bad("serialize"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(bad("content byte bound"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn content() -> Content {
        Content {
            personality_count: 2,
            managers: nfs_world::replication::players::ai::Managers {
                alive: nfs_world::replication::players::ai::State {
                    scene: 8,
                    serializer: 3,
                },
                scene: 7,
                participants: 2,
                actors: 5,
            },
            personas: vec![Persona {
                name: "ID_TEST_RACER".into(),
                provider_index: 11,
                personality_index: 1,
                preset: Preset {
                    vehicle: [3; 16],
                    color: Some([0.1, 0.2, 0.3]),
                    additional: vec![[4; 16]],
                    style: Some(Style {
                        parts: vec![[5; 16]],
                        color: Some([1., 0., 0.]),
                        material: Some([0., 0.5, 1.]),
                        window_tint: Some(0.7),
                        license_plate: Some(*b"SYNTHETC"),
                        alignment: [Some(4); 9],
                        rims: std::array::from_fn(|_| RimStyle {
                            selection: Some(3),
                            primary_color: Some([0.3; 3]),
                            primary_material: Some([0.2; 3]),
                            secondary_color: Some([0.5; 3]),
                            secondary_material: Some([0.8; 3]),
                        }),
                    }),
                },
                driving: nfs_world::replication::vehicle::customization::SpawnOverrides {
                    performance: 0.4,
                    grip: Some(4),
                },
            }],
            racers: vec![Racer {
                guid: [1; 16],
                event_type: 0,
                personas: vec![0],
                regions: vec![
                    Region::new(vec![[0., 0., 0.], [10., 0., 0.], [0., 0., 10.]], 20.).unwrap(),
                ],
                secondary_regions: vec![],
                position: [1., 2., 3.],
                basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            }],
        }
    }
    #[test]
    fn cached_personas_keep_presets_indices_geometry_and_overrides() {
        let c = content();
        let loaded = Content::from_json(&c.to_json()).unwrap();
        assert_eq!(loaded.to_json(), c.to_json());
        assert_eq!(loaded.personas[0].preset, c.personas[0].preset);
        assert_eq!(loaded.personas[0].provider_index, 11);
        assert_eq!(loaded.racers[0].regions, c.racers[0].regions);
    }
    #[test]
    fn malformed_identity_closure_shape_and_bounds_are_rejected() {
        let baseline = content().to_json();
        for (path, replacement) in [
            ("/version", json!(1)),
            ("/managers/alive/scene", json!(0)),
            ("/managers/alive/serializer", json!(65535)),
            ("/build_sha256", json!("unknown")),
            ("/personality_count", json!(0)),
            ("/personality_count", json!(1025)),
            ("/personas/0/personality_index", json!(2)),
            ("/personas/0/personality_index", json!(65536)),
            ("/managers/actors", json!(2)),
            ("/managers/participants", json!(65535)),
            ("/personas/0/name", json!("")),
            ("/personas/0/grip", json!(11)),
            ("/personas/0/performance", json!(-1.)),
            ("/personas/0/provider_index", json!(65536)),
            ("/personas/0/preset/vehicle", json!("0".repeat(32))),
            ("/personas/0/preset/style/alignment", json!([1, 2])),
            ("/personas/0/preset/style/rims/0/selection", json!(-1)),
            ("/racers/0/personas", json!([1])),
            ("/racers/0/personas", json!([])),
            ("/racers/0/position", json!([0., 1e9, 0.])),
            (
                "/racers/0/basis",
                json!([[0., 0., 0.], [0., 0., 0.], [0., 0., 0.]]),
            ),
            ("/racers/0/regions/0/points", json!([[0., 0., 0.]])),
        ] {
            let mut v = baseline.clone();
            *v.pointer_mut(path).unwrap() = replacement;
            assert!(Content::from_json(&v).is_err(), "{path}");
        }
        let mut v = baseline.clone();
        v["racers"]
            .as_array_mut()
            .unwrap()
            .push(baseline["racers"][0].clone());
        assert!(Content::from_json(&v).is_err());
        let mut v = baseline.clone();
        v["personas"]
            .as_array_mut()
            .unwrap()
            .push(baseline["personas"][0].clone());
        assert!(Content::from_json(&v).is_err());
        let mut v = baseline;
        v["racers"][0]["invented"] = json!(true);
        assert!(Content::from_json(&v).is_err());
    }
}
