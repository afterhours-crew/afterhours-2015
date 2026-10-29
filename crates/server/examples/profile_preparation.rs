// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Offline preparation timings. Reads an installation; never opens listeners,
//! a save, or the game. Keep reports local and distinguish debug from release.
use nfs_content::{Options, assets::AssetIndex};
use nfs_server::event_logic::request;
use serde_json::json;
use std::{path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: profile_preparation <game-directory> <event-count 1..26>".into());
    }
    let count: usize = args[1].to_str().ok_or("event count")?.parse()?;
    if !(1..=26).contains(&count) {
        return Err("event count must be 1..26".into());
    }
    let start = Instant::now();
    let mut index = AssetIndex::open(&PathBuf::from(&args[0]), &Options::default())?;
    println!(
        "{}",
        json!({"phase":"index", "elapsed_ms":start.elapsed().as_millis(), "debug_assertions":cfg!(debug_assertions)})
    );
    let start = Instant::now();
    let entries = index.event_roster()?;
    println!(
        "{}",
        json!({"phase":"roster", "elapsed_ms":start.elapsed().as_millis(), "entries":entries.len()})
    );
    for context in entries.iter().filter_map(request::sprint).take(count) {
        let start = Instant::now();
        let resolved = index.event_request(&context)?;
        println!(
            "{}",
            json!({"event":context.game_mode_id, "phase":"request", "elapsed_ms":start.elapsed().as_millis()})
        );
        let event = index.event_with_progress(&resolved, |phase, elapsed| {
            println!("{}", json!({"event":context.game_mode_id, "phase":phase, "elapsed_ms":elapsed.as_millis()}));
        })?;
        let model = format!("{event:?}");
        let hash: String = openssl::sha::sha256(model.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        println!(
            "{}",
            json!({"event":context.game_mode_id, "phase":"complete", "elapsed_ms":start.elapsed().as_millis(), "model_debug_sha256":hash, "checkpoints":event.game_mode.checkpoints.len()})
        );
    }
    Ok(())
}
