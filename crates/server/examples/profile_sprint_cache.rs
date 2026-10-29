// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Offline cold/warm compiler check. Opens static installation content only;
//! does not open account state, bind listeners or start the game.
use nfs_content::{Options, assets::AssetIndex};
use nfs_server::{
    event_logic::catalog::prepare, install_content, prepared_content::Cache, scoring,
    vehicle_content,
};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Instant};

fn digest(bytes: &[u8]) -> String {
    openssl::sha::sha256(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [game, data, output] = args.as_slice() else {
        return Err(
            "usage: profile_sprint_cache <game-directory> <static-content-root> <new-output-root>"
                .into(),
        );
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
    .map_err(|e| format!("vehicle content: {e:?}"))?;
    let mut expected = None;
    for run in 0..2 {
        let mut index = AssetIndex::open(game, &Options::default())?;
        let installed = index.scoring_content("levels/genesis01/gameplay")?;
        let cache = Cache::for_index(output, &index)?;
        let started = Instant::now();
        let prepared = prepare::sprints_cached(
            &mut index,
            &installed.categories,
            Some(&vehicles),
            Some(&items),
            cache.as_ref(),
        )?;
        let elapsed = started.elapsed().as_millis();
        let scoring =
            scoring::Content::new(Arc::new(installed)).map_err(|e| format!("scoring: {e:?}"))?;
        let (catalog, scoring) = prepared
            .attach(scoring)
            .map_err(|e| format!("catalog: {e:?}"))?;
        let mut models = BTreeMap::new();
        for content in catalog.iter() {
            let program = scoring
                .program(scoring::Which::EventFor(content.record_id()))
                .map_err(|e| format!("program: {e:?}"))?;
            models.insert(
                content.record_id(),
                digest(
                    format!(
                        "{:?}\n{program:?}\n{:?}",
                        content.event(),
                        content.creation_content()
                    )
                    .as_bytes(),
                ),
            );
        }
        if expected
            .as_ref()
            .is_some_and(|previous| previous != &models)
        {
            return Err("warm model mismatch".into());
        }
        println!(
            "{}",
            json!({"run":run, "debug_assertions":cfg!(debug_assertions), "elapsed_ms":elapsed,
            "events":models.len(), "models_sha256":digest(&serde_json::to_vec(&models)?)})
        );
        expected = Some(models);
    }
    Ok(())
}
