// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compare the ordinary compiler with cold and warm persisted prologue models.
//! Static installation inputs only: no account database, listeners or game.
use nfs_content::{Options, assets::AssetIndex};
use nfs_server::{
    install_content, prepared_content::Cache, progression, prologue, vehicle_content,
};
use serde_json::json;
use std::{path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [game, data, output] = args.as_slice() else {
        return Err("usage: profile_prologue_cache <game-directory> <static-content-root> <new-output-root>".into());
    };
    if output.exists() {
        return Err("output root must be new".into());
    }
    std::fs::create_dir(output)?;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let generated = install_content::resolve(data, game, None, &Options::default())?;
    let items = nfs_services::item_content::ItemContent::load(&generated.item_content)?;
    let vehicles = vehicle_content::Content::load_with_parts(
        &generated.vehicle_profiles,
        &items.catalog,
        Some(&generated.vehicle_parts),
    )
    .map_err(|e| format!("vehicles: {e:?}"))?;
    let progression = progression::Content::load(&generated.progression_content)
        .map_err(|e| format!("progression: {e:?}"))?;
    let parent = progression
        .construction
        .blueprint(progression::Branch::Intro)
        .ok_or("intro blueprint")?
        .asset();
    let mut expected = None;
    for phase in ["uncached", "cold", "warm"] {
        let mut index = AssetIndex::open(game, &Options::default())?;
        let cache = Cache::for_index(output, &index)?;
        let started = Instant::now();
        let content = prologue::Content::prepare_cached(
            &mut index,
            parent,
            items.inventory().ok_or("inventory definitions")?,
            &vehicles,
            &items,
            if phase == "uncached" {
                None
            } else {
                cache.as_ref()
            },
        )?;
        let elapsed = started.elapsed().as_millis();
        let model = format!("{content:?}");
        let digest: String = openssl::sha::sha256(model.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if expected.as_ref().is_some_and(|value| value != &digest) {
            return Err("prepared prologue model mismatch".into());
        }
        expected = Some(digest.clone());
        println!(
            "{}",
            json!({"phase":phase,"elapsed_ms":elapsed,"debug_assertions":cfg!(debug_assertions),"model_sha256":digest,
            "event_volumes":content.volumes.events.len(),"starter_cars":content.previews.as_ref().ok_or("previews")?.items.len()})
        );
    }
    Ok(())
}
