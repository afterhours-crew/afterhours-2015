// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Inspect selected installed unlock notifications without opening a save.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let delivery = args.first().is_some_and(|a| a == "--delivery");
    if delivery {
        args.remove(0);
    }
    let [game, assets @ ..] = args.as_slice() else {
        return Err("usage: unlock_messages <game-root> <message-asset>...".into());
    };
    let mut index = nfs_content::assets::AssetIndex::open(
        std::path::Path::new(game),
        &nfs_content::Options::default(),
    )?;
    let selected = assets.iter().cloned().collect();
    let output = if delivery {
        serde_json::to_string_pretty(&index.unlock_delivery(&selected)?)?
    } else {
        serde_json::to_string_pretty(&index.unlock_messages(&selected)?)?
    };
    println!("{output}");
    Ok(())
}
