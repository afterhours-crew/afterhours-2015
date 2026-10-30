// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compare complete installed scoring programs and keeper bindings, without
//! account data, listeners or a running client. Index-open time is excluded.
use nfs_content::{Options, assets::AssetIndex};
use nfs_server::prepared_content::{Cache, scoring};
use std::{path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if args.len() != 2 {
        return Err("usage: profile_scoring_cache <game-directory> <new-cache-root>".into());
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
        let content = scoring::prepare(&mut index, "levels/genesis01/gameplay", cache.as_ref())?;
        let program_ms = started.elapsed().as_millis();
        let started = Instant::now();
        let keepers = index.score_keepers(
            "levels/genesis01/gameplay",
            &content.categories.keys().copied().collect(),
        )?;
        let keepers_ms = started.elapsed().as_millis();
        let digest: String = openssl::sha::sha256(format!("{content:?}\n{keepers:?}").as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if let Some(prior) = &expected {
            if prior != &digest {
                return Err("scoring model mismatch".into());
            }
        } else {
            expected = Some(digest.clone());
        }
        println!(
            "{}",
            serde_json::json!({"phase":phase,"program_ms":program_ms,"keepers_ms":keepers_ms,
            "acquisition_nodes":content.acquisition.program.nodes.len(),"scoring_nodes":content.scoring.program.nodes.len(),
            "keepers":keepers.len(),"model_sha256":digest,"debug_assertions":cfg!(debug_assertions)})
        );
    }
    Ok(())
}
