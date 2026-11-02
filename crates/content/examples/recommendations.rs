// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Inspect an installed recommendation key without launching the game or
//! reading or changing account state.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [game, metadata, category] = args.as_slice() else {
        return Err("usage: recommendations <game-root> <metadata-asset> <category-u32>".into());
    };
    let category = category.parse::<u32>()?;
    let mut index = nfs_content::assets::AssetIndex::open(
        std::path::Path::new(game),
        &nfs_content::Options::default(),
    )?;
    let map = index.narrative_event_map()?;
    let key = index.recommendation_event_key(metadata, category)?;
    println!(
        "{}",
        serde_json::json!({"mapping_pairs":map.pairs().len(),"category":category,"recommendation_key":key})
    );
    Ok(())
}
