// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed startup topology with authored AllDrive host policy. Role names
//! identify the capabilities this server models; their keys, asset indexes,
//! level IDs and traffic order are resolved from the installation.
use crate::{
    Error,
    assets::{AssetIndex, scene_key},
    scenes,
    world::StartupKind,
};
use nfs_world::content::{
    LoadLevel, Message, RegistrationDefinition, RegistrationsState, SubLevelNames,
    bindings::Bindings,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn build(index: &mut AssetIndex, scenes: &scenes::Catalog) -> Result<Vec<u8>, Error> {
    let bad = |s: &str| Error::Content(format!("world startup: {s}"));
    let level_asset = index.identity.scenes.level.clone();
    let directory = level_asset
        .rsplit_once('/')
        .ok_or_else(|| bad("level path"))?
        .0;
    let entries = index.startup_entries(&level_asset)?;
    if entries.len() > nfs_world::content::MAX_ITEMS {
        return Err(bad("entry bound"));
    }
    let names: BTreeMap<_, _> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.to_lowercase(), i))
        .collect();
    if names.len() != entries.len() {
        return Err(bad("duplicate name"));
    }
    let role = |suffix: &str| -> Result<(String, u32, u16), Error> {
        let name = format!("{directory}/{suffix}").to_lowercase();
        let position = *names
            .get(&name)
            .ok_or_else(|| bad("required role scene is missing"))?;
        if entries[position].kind != StartupKind::SubLevel {
            return Err(bad("role is not a sub-level"));
        }
        let key = scene_key(&name);
        scenes
            .profiles
            .profile(key)
            .map_err(|_| bad("missing role profile"))?;
        Ok((name, key, (position + 1) as u16))
    };
    let gameplay = role("Gameplay")?;
    let startup = role("ParticipantStateMachine")?;
    let garage = role("Garage")?;
    let progression = role("Progression")?;
    let spawn = role("SpawnPoints")?;
    let root = index.scene(&level_asset)?;
    if root.level_references.len() != 10 {
        return Err(bad("root traffic slot count"));
    }
    let mut traffic = Vec::new();
    for name in root.level_references {
        let position = *names
            .get(&name.to_lowercase())
            .ok_or_else(|| bad("traffic bundle is not registered"))?;
        if entries[position].kind != StartupKind::BlueprintBundle
            || entries[position].parent.is_some()
        {
            return Err(bad("traffic bundle is not a root bundle"));
        }
        let key = scene_key(&name);
        if traffic.contains(&key) {
            return Err(bad("duplicate traffic slot"));
        }
        scenes
            .profiles
            .profile(key)
            .map_err(|_| bad("missing traffic profile"))?;
        traffic.push(key);
    }
    let timer = asset(
        index,
        &gameplay.0,
        gameplay.2,
        "gameplay/stats/garagestatslogic",
    )?;
    let gate = asset(index, &garage.0, garage.2, "gameplay/garageloadlogic")?;
    let level = LoadLevel {
        level: level_asset.as_bytes().to_vec(),
        attributes: vec![
            (b"StartupMode".to_vec(), b"Game".to_vec()),
            (b"GameMode".to_vec(), b"ConquestLarge".to_vec()),
        ],
        word: 0,
        text: Vec::new(),
        flags: [false; 3],
        entries: Vec::new(),
        final_word: 1,
    };
    let mut messages = vec![Message::LoadLevel(level.clone())];
    let mut batches = Vec::new();
    // Name batches have at most eight entries and must fit the codec's byte
    // bound. Large modded names split earlier rather than overrunning a frame.
    let mut batch = SubLevelNames {
        word: 1,
        entries: Vec::new(),
    };
    for (position, entry) in entries.iter().enumerate() {
        let row = (position as u16, entry.name.as_bytes().to_vec());
        let mut candidate = batch.clone();
        candidate.entries.push(row.clone());
        if candidate.entries.len() > 8 || Message::Names(candidate.clone()).encode().is_err() {
            if batch.entries.is_empty() {
                return Err(bad("name cannot fit in one frame"));
            }
            batches.push(json!({"kind":"names","word":batch.word,"entries":batch.entries}));
            messages.push(Message::Names(batch));
            batch = SubLevelNames {
                word: 1,
                entries: vec![row],
            };
        } else {
            batch = candidate;
        }
    }
    if !batch.entries.is_empty() {
        batches.push(json!({"kind":"names","word":batch.word,"entries":batch.entries}));
        messages.push(Message::Names(batch));
    }
    let mut allocator = RegistrationsState::default();
    for (chunk, group) in entries.chunks(64).enumerate() {
        let mut definitions = Vec::new();
        for (offset, entry) in group.iter().enumerate() {
            let position = chunk * 64 + offset;
            if entry.parent.is_some_and(|p| p >= position) {
                return Err(bad("parent must precede child"));
            }
            let sublevel = entry.kind == StartupKind::SubLevel;
            definitions.push(RegistrationDefinition {
                region: entry.parent.map_or(0, |p| (p + 1) as u16),
                byte: if sublevel { 254 } else { 0 },
                flag: false,
                enum3: 3,
                word2: 0,
                bit: true,
                enum2: if sublevel { 0 } else { 1 },
                word3: 0,
                text: Vec::new(),
            });
        }
        // These are the supported static-registration policy values. Unknown
        // wire fields retain numeric names; no gameplay meaning is invented.
        messages.push(
            allocator
                .prepare(6, &definitions)
                .map_err(|_| bad("registration encoding"))?,
        );
        batches.push(
            json!({"kind":"registrations","header":6,"definitions":definitions.iter().map(|d|json!({
            "region":d.region,"byte":d.byte,"flag":d.flag,"enum3":d.enum3,"word2":d.word2,
            "bit":d.bit,"enum2":d.enum2,"word3":d.word3,"text":d.text})).collect::<Vec<_>>()}),
        );
    }
    Bindings::from_messages(&messages).map_err(|_| bad("invalid registration topology"))?;
    let mut roles = json!({"level":level_asset,"gameplay":gameplay.1,"startup":startup.1,"garage":garage.1,
        "progression":progression.1,"spawn_points":spawn.1,"traffic":traffic,
        "customization_timer":timer,"streaming_gate":gate});
    let mut bank = None;
    let mut seen_points = None;
    for entry in entries.iter().filter(|e| e.kind == StartupKind::SubLevel) {
        for binding in index.scene_bindings(&entry.name)? {
            if binding.class == "BankEntityData" {
                if bank.is_some() {
                    return Err(bad("ambiguous bank entity"));
                }
                bank =
                    Some(json!({"scene":scene_key(&entry.name),"serializer":binding.serializer}));
            }
            if binding.class == "PersistedIdEntityData" {
                if seen_points.is_some() {
                    return Err(bad("ambiguous seen map points entity"));
                }
                seen_points =
                    Some(json!({"scene":scene_key(&entry.name),"serializer":binding.serializer}));
            }
        }
    }
    if let Some(bank) = bank {
        roles["bank"] = bank;
    }
    if let Some(binding) = seen_points {
        roles["seen_points"] = binding;
    }
    let value = json!({"format":"nfs-world-content","version":5,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "level":{"level":level.level,"attributes":level.attributes,"word":level.word,"text":level.text,
            "flags":level.flags,"entries":level.entries,"final_word":level.final_word},
        "batches":batches,"roles":roles});
    serde_json::to_vec(&value).map_err(|_| bad("JSON encoding"))
}

fn asset(index: &mut AssetIndex, scene: &str, level_id: u16, name: &str) -> Result<Value, Error> {
    let reference = index.reference(scene, name)?;
    if Some(&reference.class_id)
        != index
            .identity
            .blueprint_class_ids
            .get("ParticipantLogicBlueprint")
        || reference.local_index > u16::MAX.into()
        || reference.class_id > u16::MAX.into()
    {
        return Err(Error::Content(
            "world startup role asset has an unsupported class or index".into(),
        ));
    }
    Ok(json!({"bundle":level_id,"type_id":reference.class_id,"local_index":reference.local_index}))
}
