// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only inspection of a supported installation's distance binding.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [game, blueprint] = args.as_slice() else {
        return Err("usage: driving_distance <game directory> <logic blueprint>".into());
    };
    let mut index = nfs_content::assets::AssetIndex::open(
        std::path::Path::new(game),
        &nfs_content::Options::default(),
    )?;
    let binding = index.driving_distance_binding(blueprint)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "fingerprint":index.fingerprint(), "binding":binding,
            "scope":"Static authored binding only; does not establish runtime or game acceptance."
        }))?
    );
    Ok(())
}
