// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{
    Content, Error, MAX_RACERS, MAX_REGIONS, Persona, Racer, Region, bad,
    preset::{Preset, RimStyle, Style},
};
use crate::{
    assets::AssetIndex,
    graph::{
        Graph, Key, Site, Source, bits32,
        scenes::{SceneLookup, scene_roots},
    },
};
use nfs_frostbite::ebx::{Object, Value};
use std::collections::{BTreeMap, BTreeSet};

fn field<'a>(o: &'a Object, name: &str) -> Result<&'a Value, Error> {
    o.fields
        .get(name)
        .ok_or_else(|| bad(&format!("missing {}.{name}", o.class)))
}
fn number(v: &Value) -> Result<f32, Error> {
    match v {
        Value::Float32(n) if n.is_finite() && n.abs() <= 1_000_000. => Ok(*n),
        _ => Err(bad("finite float")),
    }
}
fn vector(v: &Value) -> Result<[f32; 3], Error> {
    let f = v.as_fields().ok_or_else(|| bad("vector"))?;
    let mut out = [0.; 3];
    for (i, k) in ["x", "y", "z"].into_iter().enumerate() {
        out[i] = number(f.get(k).ok_or_else(|| bad("vector field"))?)?;
    }
    Ok(out)
}
fn boolean(o: &Object, name: &str) -> Result<bool, Error> {
    field(o, name)?.as_bool().ok_or_else(|| bad("boolean"))
}
fn text(o: &Object, name: &str) -> Result<String, Error> {
    field(o, name)?
        .as_str()
        .filter(|s| s.len() <= 512 && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or_else(|| bad("text"))
}
fn refs<S: Source>(
    graph: &mut Graph<S>,
    key: &Key,
    o: &Object,
    name: &str,
    max: usize,
) -> Result<Vec<Key>, Error> {
    field(o, name)?
        .as_array()
        .filter(|a| a.len() <= max)
        .ok_or_else(|| bad("reference count"))?
        .iter()
        .map(|v| {
            graph
                .source
                .target(&key.0, v.as_pointer().ok_or_else(|| bad("reference"))?)?
                .ok_or_else(|| bad("null array reference"))
        })
        .collect()
}
fn reference<S: Source>(
    graph: &mut Graph<S>,
    key: &Key,
    o: &Object,
    name: &str,
) -> Result<Option<Key>, Error> {
    graph.source.target(
        &key.0,
        field(o, name)?.as_pointer().ok_or_else(|| bad("pointer"))?,
    )
}
fn guid<S: Source>(graph: &mut Graph<S>, key: &Key) -> Result<[u8; 16], Error> {
    graph
        .source
        .object(key)?
        .guid
        .ok_or_else(|| bad("unexported item"))
}
fn tags<S: Source>(graph: &mut Graph<S>, key: &Key, o: &Object) -> Result<BTreeSet<u32>, Error> {
    refs(graph, key, o, "VehicleTags", 128)?
        .into_iter()
        .map(|key| {
            let tag = graph.source.object(&key)?;
            if tag.class != "VehicleTag" {
                return Err(bad("tag class"));
            }
            // Providers compare the installed TagHash, not pointer/GUID identity.
            bits32(field(&tag, "TagHash")?)
        })
        .collect()
}
fn one<S: Source>(
    graph: &mut Graph<S>,
    site: &Site,
    port: u32,
    class: &str,
) -> Result<Site, Error> {
    let linked = graph.links(site, port)?;
    if linked.len() != 1 || graph.object(&linked[0].site)?.class != class {
        return Err(bad(&format!("expected one linked {class}")));
    }
    Ok(linked[0].site.clone())
}

fn color(o: &Object, name: &str) -> Result<Option<[f32; 3]>, Error> {
    let value = vector(field(o, name)?)?;
    let d = value.map(|n| (n + 1.) * (n + 1.));
    Ok(((d[0] + d[1] + d[2]).sqrt() > 0.0001).then_some(value))
}
fn setting(o: &Object, name: &str) -> Result<Option<u32>, Error> {
    let n = bits32(field(o, name)?)?;
    Ok((n != u32::MAX).then_some(n))
}
fn style<S: Source>(graph: &mut Graph<S>, key: &Key) -> Result<Style, Error> {
    let o = graph.source.object(key)?;
    if o.class != "StyleItemData" {
        return Err(bad("style class"));
    }
    let parts = refs(graph, key, &o, "SubItems", super::preset::MAX_PARTS)?
        .iter()
        .map(|k| guid(graph, k))
        .collect::<Result<_, _>>()?;
    let window_tint = number(field(&o, "WindowTint")?)?;
    let plate = text(&o, "LicensePlate")?;
    let license_plate = if plate.is_empty() {
        None
    } else {
        let mut out = [0; 8];
        let n = plate.len().min(8);
        out[..n].copy_from_slice(&plate.as_bytes()[..n]);
        Some(out)
    };
    let mut alignment = [None; 9];
    for (i, name) in [
        "Caster",
        "CamberFront",
        "CamberRear",
        "RideHeight",
        "RideHeightRake",
        "ToeFront",
        "ToeRear",
        "TrackWidthFront",
        "TrackWidthRear",
    ]
    .into_iter()
    .enumerate()
    {
        alignment[i] = setting(&o, name)?
            .map(|v| u8::try_from(v).map_err(|_| bad("alignment")))
            .transpose()?;
    }
    let mut rims = [RimStyle::default(), RimStyle::default()];
    for (i, side) in ["Front", "Rear"].into_iter().enumerate() {
        rims[i] = RimStyle {
            selection: setting(&o, &format!("RimSize{side}"))?,
            primary_color: color(&o, &format!("RimPrimaryColor{side}"))?,
            primary_material: color(&o, &format!("RimPrimaryMaterial{side}"))?,
            secondary_color: color(&o, &format!("RimSecondaryColor{side}"))?,
            secondary_material: color(&o, &format!("RimSecondaryMaterial{side}"))?,
        };
    }
    let style = Style {
        parts,
        color: color(&o, "VehicleColor")?,
        material: color(&o, "VehicleMaterial")?,
        window_tint: ((window_tint + 1.).abs() > 0.0001).then_some(window_tint),
        license_plate,
        alignment,
        rims,
    };
    style.validate()?;
    Ok(style)
}
fn persona<S: Source>(
    graph: &mut Graph<S>,
    key: &Key,
    provider_index: usize,
) -> Result<Persona, Error> {
    let o = graph.source.object(key)?;
    if o.class != "PersonaEntityData" {
        return Err(bad("persona class"));
    }
    // The native provider prefers the localization ID; the client resolves it.
    let id = text(&o, "PersonaNameId")?;
    let name = if id.is_empty() {
        text(&o, "PersonaName")?
    } else {
        id
    };
    if name.is_empty() || name.len() > 64 {
        return Err(bad("persona name"));
    }
    let vehicle =
        reference(graph, key, &o, "VehicleItemData")?.ok_or_else(|| bad("persona vehicle"))?;
    if graph.source.object(&vehicle)?.class != "RaceVehicleItemData" {
        return Err(bad("persona vehicle class"));
    }
    let vehicle = guid(graph, &vehicle)?;
    let color = if boolean(&o, "OverrideCarColor")? {
        Some(vector(field(&o, "DefaultPaint")?)?)
    } else {
        None
    };
    let style = reference(graph, key, &o, "VehicleStyle")?
        .as_ref()
        .map(|k| style(graph, k))
        .transpose()?;
    let additional = refs(graph, key, &o, "AdditionalItems", super::preset::MAX_PARTS)?
        .iter()
        .map(|k| guid(graph, k))
        .collect::<Result<_, _>>()?;
    let performance = if boolean(&o, "OverridePerformance")? {
        number(field(&o, "Performance")?)?
    } else {
        0.
    };
    let grip = if boolean(&o, "OverrideGripValue")? {
        Some(u8::try_from(bits32(field(&o, "GripValue")?)?).map_err(|_| bad("grip"))?)
    } else {
        None
    };
    if performance < 0. || grip.is_some_and(|n| n > 10) {
        return Err(bad("driving overrides"));
    }
    Ok(Persona {
        name,
        provider_index: u16::try_from(provider_index).map_err(|_| bad("provider index"))?,
        preset: Preset {
            vehicle,
            color,
            style,
            additional,
        },
        driving: nfs_world::replication::vehicle::customization::SpawnOverrides {
            performance,
            grip,
        },
    })
}
fn regions<S: Source>(graph: &mut Graph<S>, site: &Site, port: u32) -> Result<Vec<Region>, Error> {
    let linked = graph.links(site, port)?;
    if linked.len() > MAX_REGIONS {
        return Err(bad("region count"));
    }
    linked
        .into_iter()
        .map(|endpoint| {
            let o = graph.object(&endpoint.site)?;
            if o.class != "VolumeVectorShapeData" {
                return Err(bad("region class"));
            }
            let points = field(&o, "Points")?
                .as_array()
                .filter(|p| p.len() <= super::MAX_REGION_POINTS)
                .ok_or_else(|| bad("region points"))?
                .iter()
                .map(vector)
                .collect::<Result<_, _>>()?;
            Region::new(points, number(field(&o, "Height")?)?)
        })
        .collect()
}

/// Resolve the installed roaming manager through its actual subworld/prefab
/// interfaces, preserving provider order independently of catalog or file order.
pub fn extract(index: &mut AssetIndex) -> Result<Content, Error> {
    let rules = index.identity.scenes.clone();
    let ports = index.identity.rival_links;
    let startup = index.startup_entries(&rules.level)?;
    let mut graph = Graph::new(&mut index.assets);
    let roots = scene_roots(&mut graph, &rules.level, &startup)?;
    let mut manager = None;
    for root in roots.values() {
        for node in graph.read(root.clone())? {
            if node.object.class == "RoamingRacerManagerEntityData"
                && manager.replace(node.site).is_some()
            {
                return Err(bad("multiple roaming managers"));
            }
        }
    }
    let manager = manager.ok_or_else(|| bad("missing roaming manager"))?;
    let mut lookup = SceneLookup {
        roots,
        rules: &rules,
        endpoints: BTreeMap::new(),
        loaded: BTreeSet::new(),
    };
    let pm = one(
        &mut graph,
        &manager,
        ports.participants,
        "AiParticipantManagerEntityData",
    )?;
    let am = one(
        &mut graph,
        &manager,
        ports.actors,
        "AiActorManagerEntityData",
    )?;
    let (scene, participants) = lookup.endpoint(&mut graph, &pm)?;
    let (actor_scene, actors) = lookup.endpoint(&mut graph, &am)?;
    if scene != actor_scene || participants == actors {
        return Err(bad("manager scene identity"));
    }
    let linked = graph.links(&manager, ports.racers)?;
    if linked.is_empty() || linked.len() > MAX_RACERS {
        return Err(bad("racer count"));
    }
    let mut personas = Vec::new();
    let mut persona_ids = BTreeMap::new();
    let mut racers = Vec::new();
    let mut ids = BTreeSet::new();
    for endpoint in linked {
        let site = endpoint.site;
        let o = graph.object(&site)?;
        if o.class != "RoamingRacerEntityData" {
            return Err(bad("racer class"));
        }
        let guid = o.guid.ok_or_else(|| bad("racer GUID"))?;
        if !ids.insert(guid) {
            return Err(bad("duplicate racer"));
        }
        let mut candidates = Vec::new();
        for provider in graph.links(&site, ports.provider)? {
            let p = graph.object(&provider.site)?;
            if p.class != "SpecificVehicleProviderEntityData" {
                return Err(bad("racer provider class"));
            }
            let required = tags(&mut graph, &provider.site.key, &p)?;
            let parent = one(
                &mut graph,
                &provider.site,
                ports.provider,
                "PersonaSetEntityData",
            )?;
            for (ordinal, e) in graph
                .links(&parent, ports.personas)?
                .into_iter()
                .enumerate()
            {
                if ordinal >= MAX_RACERS {
                    return Err(bad("provider persona count"));
                }
                let o = graph.object(&e.site)?;
                if o.class != "PersonaEntityData" {
                    return Err(bad("provider member class"));
                }
                if !required.is_subset(&tags(&mut graph, &e.site.key, &o)?) {
                    continue;
                }
                let identity = (e.site.key.clone(), parent.clone(), ordinal);
                let id = if let Some(&id) = persona_ids.get(&identity) {
                    id
                } else {
                    if personas.len() >= MAX_RACERS {
                        return Err(bad("persona count"));
                    }
                    let id = personas.len();
                    personas.push(persona(&mut graph, &e.site.key, ordinal)?);
                    persona_ids.insert(identity, id);
                    id
                };
                candidates.push(id);
            }
        }
        if candidates.is_empty() || candidates.len() > MAX_RACERS {
            return Err(bad("racer has no available personas"));
        }
        let spawn = one(&mut graph, &site, ports.spawn, "RacerSpawnPointEntityData")?;
        let s = graph.object(&spawn)?;
        let transform = field(&s, "Transform")?
            .as_fields()
            .ok_or_else(|| bad("spawn transform"))?;
        let get = |name| {
            transform
                .get(name)
                .ok_or_else(|| bad("transform field"))
                .and_then(vector)
        };
        let basis = [get("right")?, get("up")?, get("forward")?];
        nfs_world::replication::vehicle::orientation::quaternion_from_basis(basis)
            .map_err(|_| bad("spawn orientation"))?;
        racers.push(Racer {
            guid,
            event_type: bits32(field(&o, "EventType")?)?,
            personas: candidates,
            regions: regions(&mut graph, &site, ports.regions)?,
            secondary_regions: regions(&mut graph, &site, ports.secondary_regions)?,
            position: get("trans")?,
            basis,
        });
    }
    Ok(Content {
        managers: nfs_world::replication::players::ai::Managers {
            scene,
            participants,
            actors,
        },
        personas,
        racers,
    })
}

#[cfg(test)]
mod tests;
