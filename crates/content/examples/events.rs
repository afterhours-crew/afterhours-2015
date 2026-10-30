// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Inspect the installed normal-event roster without launching the game.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [game, output] = args.as_slice() else {
        return Err("usage: events <game-root> <output.json>".into());
    };
    let mut index = nfs_content::assets::AssetIndex::open(
        std::path::Path::new(game),
        &nfs_content::Options::default(),
    )?;
    let entries = index.event_roster()?;
    let mut scenes = std::collections::BTreeMap::new();
    for entry in &entries {
        if !scenes.contains_key(&entry.scene) {
            use nfs_world::replication::sublevel::Kind;
            let scene = index.scene(&entry.scene)?;
            let mut next = 0u16;
            let selectors: Vec<_> = scene
                .profile
                .kinds()
                .iter()
                .map(|kind| {
                    if matches!(
                        kind,
                        Kind::Noop
                            | Kind::BoolProperty
                            | Kind::FloatProperty
                            | Kind::I32Property
                            | Kind::LevelReference
                    ) {
                        None
                    } else {
                        let selector = next;
                        next += 1;
                        Some(selector)
                    }
                })
                .collect();
            scenes.insert(
                entry.scene.clone(),
                (index.scene_bindings(&entry.scene)?, selectors),
            );
        }
    }
    let mut rows = Vec::new();
    for entry in &entries {
        let (bindings, selectors) = &scenes[&entry.scene];
        let mut sources = Vec::new();
        for endpoint in &entry.notifications {
            if endpoint.scene_key != nfs_content::assets::scene_key(&entry.scene) {
                return Err("notification outside event scene".into());
            }
            let binding = bindings
                .iter()
                .find(|b| b.serializer == endpoint.serializer)
                .ok_or("missing notification binding")?;
            let selector = selectors
                .get(endpoint.serializer)
                .copied()
                .flatten()
                .ok_or("notification has no RPC selector")?;
            if binding.class != "SetClientNotificationEntityData" {
                return Err("wrong notification class".into());
            }
            sources.push(serde_json::json!({"scene_key":endpoint.scene_key,"serializer":endpoint.serializer,
                "selector":selector,"asset":binding.asset,"object":binding.object,"class":binding.class}));
        }
        rows.push(serde_json::json!({"id":entry.id,"scene":entry.scene,"game_mode":entry.game_mode,
            "volume":[entry.volume.scene_key,entry.volume.serializer],
            "notifications":entry.notifications.iter().map(|n| (n.scene_key,n.serializer)).collect::<Vec<_>>(),
            "sources":sources}));
    }
    std::fs::write(output, serde_json::to_vec_pretty(&rows)?)?;
    println!("{} normal event areas", entries.len());
    Ok(())
}
