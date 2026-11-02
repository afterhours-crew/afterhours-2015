// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Inspect the complete installed world-volume catalog and its spot predicates.
//! Static content only: no account, listeners, game process or captured replies.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [game, output] = args.as_slice() else {
        return Err("usage: world_volumes <game-directory> <new-output.json>".into());
    };
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut index = nfs_content::assets::AssetIndex::open(
        std::path::Path::new(game),
        &nfs_content::Options::default(),
    )?;
    let volumes = nfs_server::prologue::volumes::WorldVolumes::prepare(&mut index)?;
    serde_json::to_writer_pretty(&mut output, &volumes)?;
    println!(
        "{} events, {} activities, {} spots, {} compiled spot memberships",
        volumes.events.len(),
        volumes.activities.len(),
        volumes.spots.len(),
        volumes.spot_memberships.len()
    );
    Ok(())
}
