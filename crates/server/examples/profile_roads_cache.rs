// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compare installed uncached/cold/warm static road models without saves or sockets.
use nfs_content::{Options, assets::AssetIndex};
use nfs_server::prepared_content::{Cache, roads};
use std::{path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if args.len() != 2 {
        return Err("usage: profile_roads_cache <game-directory> <new-cache-root>".into());
    }
    if args[1].exists() {
        return Err("cache root must be new".into());
    }
    std::fs::create_dir(&args[1])?;
    let mut expected = None;
    for phase in ["uncached", "cold", "warm"] {
        let mut index = AssetIndex::open(&args[0], &Options::default())?;
        let cache = if phase == "uncached" {
            None
        } else {
            Cache::for_index(&args[1], &index)?
        };
        let started = Instant::now();
        let graph = roads::graph(&mut index, cache.as_ref())?;
        let graph_ms = started.elapsed().as_millis();
        let started = Instant::now();
        let network = roads::network(&mut index, cache.as_ref())?;
        let network_ms = started.elapsed().as_millis();
        let digest: String = openssl::sha::sha256(format!("{graph:?}\n{network:?}").as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if let Some(prior) = &expected {
            if prior != &digest {
                return Err("road model mismatch".into());
            }
        } else {
            expected = Some(digest.clone());
        }
        println!(
            "{}",
            serde_json::json!({"phase":phase,"graph_ms":graph_ms,"network_ms":network_ms,
            "nodes":graph.node_count(),"lanes":graph.lane_count(),"segments":network.segments().len(),
            "model_sha256":digest,"debug_assertions":cfg!(debug_assertions)})
        );
    }
    Ok(())
}
