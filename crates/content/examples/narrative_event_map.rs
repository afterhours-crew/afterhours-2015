// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Inspect the installed narrative-to-standard event mapping without launching
//! the game or reading account state: every authored pair, and the standard
//! event of each queried record ID or game-mode asset name.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let Some((game, queries)) = args.split_first() else {
        return Err("usage: narrative_event_map <game-root> [record-id|game-mode-id]...".into());
    };
    let mut index = nfs_content::assets::AssetIndex::open(
        std::path::Path::new(game),
        &nfs_content::Options::default(),
    )?;
    let map = index.narrative_event_map()?;
    let queried: Vec<_> = queries
        .iter()
        .map(|query| {
            let event = query
                .parse::<u32>()
                .unwrap_or_else(|_| nfs_content::events::record_id(query));
            serde_json::json!({
                "query": query,
                "event": event,
                "standard_event": map.standard_event(event),
                "mapped": map.pairs().contains_key(&event),
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({"pairs": map.pairs(), "queried": queried})
    );
    Ok(())
}
