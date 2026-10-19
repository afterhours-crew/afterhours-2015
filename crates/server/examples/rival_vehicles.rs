// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only check of complete installed rival vehicle component construction.
use nfs_server::vehicle_content::{Content, Request};
use nfs_world::replication::vehicle::{self, authority};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("usage: rival_vehicles GAME_DIRECTORY PREPARED_CONTENT_DIRECTORY".into());
    }
    let cache = Path::new(&args[1]);
    let items = nfs_services::item_content::ItemContent::load(&cache.join("item-content.json"))?;
    let catalog = items.inventory().ok_or("inventory catalog")?;
    let slots = items.purchase_offers().ok_or("spatial policy")?;
    let parts = nfs_content::vehicle_parts::Catalog::load(
        &cache.join("vehicle-parts.json"),
        catalog.bindings(),
    )?;
    let content = Content::load_with_parts(
        &cache.join("vehicle-profiles.json"),
        catalog.bindings(),
        Some(&cache.join("vehicle-parts.json")),
    )
    .map_err(|e| format!("profiles: {e:?}"))?;
    let mut index = nfs_content::assets::AssetIndex::open(
        Path::new(&args[0]),
        &nfs_content::Options::default(),
    )?;
    let rivals = nfs_content::rivals::extract(&mut index)?;
    let mut result = Vec::new();
    for persona in &rivals.personas {
        let items = persona.preset.instantiate(catalog, slots, &parts.wheels)?;
        let definition = content
            .definition(&persona.preset.vehicle)
            .ok_or("rival vehicle profile")?;
        let components = (1..=definition
            .profile
            .kinds()
            .iter()
            .filter(|k| matches!(k, vehicle::Kind::FourBit))
            .count() as u64)
            .collect::<Vec<_>>();
        let request = Request {
            items: &items,
            vehicle: 1,
            locator: [0.; 3],
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            origin: [0.; 3],
            speed: Some(0.),
            connection: None,
            owner: vehicle::Resource {
                words: [0; 2],
                // This is the wrap owner's online resource, not the AI actor's
                // localized persona name. Offline presets have no such owner.
                name: vec![],
            },
            customization_attached: true,
            placement: nfs_server::vehicle_content::Placement::Rival,
            authority_group: 1,
            authority_components: &components,
        };
        // Mesh resolver identities are deliberately synthetic: this audit tests
        // component construction/encoding, not live bundle registration.
        let body = content
            .construct_with_overrides(
                request,
                persona.driving,
                &mut authority::Registry::default(),
                |_| Some(1),
            )
            .map_err(|e| format!("{} vehicle: {e:?}", persona.name))?;
        body.encode(&definition.profile)
            .map_err(|e| format!("{} encoding: {e:?}", persona.name))?;
        result.push(
            serde_json::json!({"name":persona.name,"components":definition.profile.kinds().len()}),
        );
    }
    println!(
        "{}",
        serde_json::json!({"result":"constructed and encoded","personas":result,"live_validation":false})
    );
    Ok(())
}
