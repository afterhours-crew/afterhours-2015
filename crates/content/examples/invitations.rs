// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compile one installed invitation group without loading account state.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [game, kind, first, second, third] = args.as_slice() else {
        return Err("usage: invitations <game-root> <power|result> <first-asset> <second-asset> <third-asset>".into());
    };
    let mut index = nfs_content::assets::AssetIndex::open(
        std::path::Path::new(game),
        &nfs_content::Options::default(),
    )?;
    let selected = [first.as_str(), second.as_str(), third.as_str()];
    match kind.as_str() {
        "power" => println!("{:#?}", index.power_objective_messages(selected)?),
        "result" => println!("{:#?}", index.result_objective_messages(selected)?),
        _ => return Err("unknown invitation kind".into()),
    }
    Ok(())
}
