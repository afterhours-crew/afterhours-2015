// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only installed rival/preset audit. No game process or account access.
use serde_json::json;
use std::{collections::BTreeSet, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 && !(args.len() == 3 && args[2] == "--cached") {
        return Err("usage: rivals GAME_DIRECTORY PREPARED_CONTENT_DIRECTORY [--cached]".into());
    }
    let content = Path::new(&args[1]);
    let items = nfs_services::item_content::ItemContent::load(&content.join("item-content.json"))?;
    let catalog = items.inventory().ok_or("missing inventory catalog")?;
    let slots = items.purchase_offers().ok_or("missing spatial policy")?;
    let parts = nfs_content::vehicle_parts::Catalog::load(
        &content.join("vehicle-parts.json"),
        catalog.bindings(),
    )?;
    let rivals = if args.len() == 3 {
        nfs_content::rivals::Content::load(&content.join("rival-content.json"))?
    } else {
        let mut index = nfs_content::assets::AssetIndex::open(
            Path::new(&args[0]),
            &nfs_content::Options::default(),
        )?;
        nfs_content::rivals::extract(&mut index)?
    };
    let mut personas = Vec::new();
    for persona in &rivals.personas {
        let state = persona
            .preset
            .instantiate(catalog, slots, &parts.wheels)
            .map_err(|e| format!("persona {} preset: {e}", persona.name))?;
        let custom = nfs_world::replication::vehicle::Customization::from_vehicle_with_overrides(
            &state,
            &parts.customization,
            1,
            persona.driving,
        )
        .map_err(|e| format!("persona {} customization: {e:?}", persona.name))?;
        let installed: BTreeSet<_> = state.items[&1].children.iter().copied().collect();
        personas.push(json!({"name":persona.name,"provider_index":persona.provider_index,"personality_index":persona.personality_index,"vehicle":nfs_frostbite::hex(&persona.preset.vehicle),
            "performance":persona.driving.performance,"grip":persona.driving.grip,"wire_performance":custom.value,"tuning":custom.tuning,
            "parts":state.items.values().map(|i|json!({"id":i.id,"owner":i.owner,"definition":nfs_frostbite::hex(&i.definition),"installed":installed.contains(&i.id),
                "children":i.children,"defaults":i.defaults,"derived":nfs_frostbite::hex(&i.derived.encode().expect("validated derived"))})).collect::<Vec<_>>()}));
    }
    let regions = |regions: &[nfs_content::rivals::Region]| {
        regions
            .iter()
            .map(|r| json!({"height":r.height(),"points":r.points()}))
            .collect::<Vec<_>>()
    };
    println!(
        "{}",
        serde_json::to_string(
            &json!({"managers":{"scene":rivals.managers.scene,"participants":rivals.managers.participants,"actors":rivals.managers.actors},"personas":personas,
        "racers":rivals.racers.iter().map(|r|json!({"guid":nfs_frostbite::hex(&r.guid),"personas":r.personas,"event_type":r.event_type,"position":r.position,"basis":r.basis,
            "regions":regions(&r.regions),"secondary_regions":regions(&r.secondary_regions)})).collect::<Vec<_>>()})
        )?
    );
    Ok(())
}
