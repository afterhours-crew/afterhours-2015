// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only installed-content preflight of every roaming rival lifetime.
//! Synthetic focus points exercise allocation and the host's road-graph
//! simulation; this does not simulate client driving.
use nfs_server::{content::WorldContent, rival_content, vehicle_content};
use nfs_world::{
    content::bindings::Bindings,
    replication::{
        players::{Players, Request},
        section, sublevel,
    },
};
use std::{path::Path, sync::Arc};

fn run(cache: &Path, game: &Path) -> Result<serde_json::Value, String> {
    let error = |e| format!("world: {e:?}");
    let data = nfs_content::rivals::Content::load(&cache.join("rival-content.json"))
        .map_err(|e| e.to_string())?;
    let mut focus = Vec::new();
    for (index, racer) in data.racers.iter().enumerate() {
        let mut found = None;
        for region in &racer.regions {
            let pts = region.points();
            let y = pts.iter().map(|p| p[1]).sum::<f32>() / pts.len() as f32
                + region.height().max(0.) * 0.5;
            let min: [f32; 3] =
                std::array::from_fn(|i| pts.iter().map(|p| p[i]).fold(f32::INFINITY, f32::min));
            let max: [f32; 3] =
                std::array::from_fn(|i| pts.iter().map(|p| p[i]).fold(f32::NEG_INFINITY, f32::max));
            for x in 1..64 {
                for z in 1..64 {
                    let p = [
                        min[0] + (max[0] - min[0]) * x as f32 / 64.,
                        y,
                        min[2] + (max[2] - min[2]) * z as f32 / 64.,
                    ];
                    if region.contains(p) {
                        found = Some(p);
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        focus.push(rival_content::population::Focus {
            id: index as u16 + 1,
            position: found.ok_or("no interior focus point")?,
        });
    }
    let items = nfs_services::item_content::ItemContent::load(&cache.join("item-content.json"))
        .map_err(|e| format!("items: {e:?}"))?;
    let definitions = vehicle_content::Content::load_with_parts(
        &cache.join("vehicle-profiles.json"),
        items.inventory().ok_or("inventory catalog")?.bindings(),
        Some(&cache.join("vehicle-parts.json")),
    )
    .map_err(|e| format!("vehicles: {e:?}"))?;
    let graph = nfs_content::assets::AssetIndex::open(game, &nfs_content::Options::default())
        .and_then(|mut index| index.road_graph())
        .map_err(|e| format!("road graph: {e:?}"))?;
    let rivals = Arc::new(
        rival_content::Content::new(data, definitions, &items)
            .map_err(|e| format!("rivals: {e:?}"))?
            .with_graph(Arc::new(graph)),
    );
    let expected = rivals.racers();
    let scenes = nfs_content::scenes::Catalog::load(&cache.join("scene-catalog.json"))
        .map_err(|e| e.to_string())?;
    let content = WorldContent::load_with_scenes(&cache.join("world-content.json"), Some(&scenes))
        .map_err(|e| format!("content: {e:?}"))?;
    let messages = content.generate().map_err(|e| format!("messages: {e:?}"))?;
    let bindings = Bindings::from_messages(&messages).map_err(|e| format!("bindings: {e:?}"))?;
    let mut registrations = vehicle_content::registrations::Registry::from_messages(&messages)
        .map_err(|e| format!("registrations: {e:?}"))?;
    let mut population = rival_content::population::Population::new(&bindings, rivals)
        .map_err(|e| format!("population initialization: {e:?}"))?;
    let mut world = Players::default();
    let player = world
        .create(
            1,
            1,
            Request {
                name: b"Audit".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .map_err(error)?
        .ok_or("player")?;
    world.join(1, 1, player.id).map_err(error)?;
    let profiles = content.scene_profiles.as_ref().ok_or("profiles")?;
    let creations = sublevel::genesis::hierarchy(
        sublevel::Rpc {
            selector: 0,
            serial: nfs_protocol::world::rpc::Serial::new(0).ok_or("serial")?,
        },
        &bindings,
        profiles,
        &content.roles.as_ref().ok_or("roles")?.traffic,
    )
    .map_err(error)?;
    let levels = creations
        .iter()
        .map(|c| c.prefix.level_id)
        .collect::<Vec<_>>();
    world
        .create_scenes(&levels, creations, profiles)
        .map_err(error)?;
    let before = world.objects().snapshot();
    let mut new_scenes = std::collections::BTreeSet::new();
    let mut updates = Vec::new();
    for now in 0..(expected as u64 + 20) {
        let changes = population
            .advance(
                &mut world,
                &mut registrations,
                1,
                1,
                rival_content::population::Input {
                    now: now * 50,
                    focus: &focus,
                    random: now as u32,
                },
            )
            .map_err(|e| format!("population tick {now}: {e:?}"))?;
        for m in &changes.messages {
            m.encode().map_err(|e| format!("message: {e:?}"))?;
        }
        for r in &changes.records {
            if matches!(
                r.initial,
                Some(nfs_world::replication::Initial::SubLevel { .. })
            ) {
                new_scenes.insert(r.id);
            }
        }
        let sections = section::split_timed_records(
            changes.records,
            nfs_world::application::OUTBOUND_FRAME_BITS - 104,
        )
        .map_err(error)?;
        if changes.messages.len() > 32
            || sections.len() + usize::from(!changes.calls.is_empty()) > 32
        {
            return Err("edge publication queue bound".into());
        }
        if let Some(ids) = changes.spawned {
            updates.push(serde_json::json!({"participant":ids.participant,"actor":ids.actor,"vehicle":ids.vehicle,"messages":changes.messages.len(),"sections":sections.len()}));
        }
    }
    // Every racer activates on a road near its spawn; cars exist only for
    // racers within the hand-over distance of a focus.
    let cars = world.ai_count();
    if population.count() != expected || world.ai_slot_count() != expected || cars == 0 {
        return Err(format!(
            "activated {} of {expected} with {cars} cars",
            population.count()
        ));
    }
    let retired = population
        .advance(
            &mut world,
            &mut registrations,
            1,
            1,
            rival_content::population::Input {
                now: (expected as u64 + 21) * 50,
                focus: &[],
                random: 0,
            },
        )
        .map_err(error)?;
    let after: Vec<_> = world
        .objects()
        .snapshot()
        .into_iter()
        .filter(|r| !new_scenes.contains(&r.id))
        .collect();
    if population.count() != 0
        || world.ai_count() != 0
        || world.ai_slot_count() != 0
        || before != after
    {
        return Err("cleanup mismatch".into());
    }
    Ok(
        serde_json::json!({"result":"activated and retired all rivals","racers":expected,"cars":cars,"spawned":updates,"deleted":retired.deleted.len(),"session_bundle_scenes":new_scenes.len(),"live_validation":false}),
    )
}
fn main() -> Result<(), String> {
    let usage = "usage: rival_population PREPARED_CONTENT_DIRECTORY GAME_DIRECTORY";
    let path = std::env::args().nth(1).ok_or(usage)?;
    let game = std::env::args().nth(2).ok_or(usage)?;
    println!("{}", run(Path::new(&path), Path::new(&game))?);
    Ok(())
}
