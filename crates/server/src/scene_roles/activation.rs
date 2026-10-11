// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_world::world_activation::{Catalog, MAX_VOLUMES, Site};

fn site(v: &Value) -> Result<Site, Failure> {
    fields(v, &["scene_key", "serializer"])?;
    Ok(Site {
        scene_key: number(&v["scene_key"])?,
        serializer: number(&v["serializer"])?,
    })
}
pub(super) fn parse(v: &Value) -> Result<Catalog, Failure> {
    fields(v, &["volumes", "presence", "streamed", "location"])?;
    let volumes = v["volumes"]
        .as_array()
        .filter(|a| a.len() <= MAX_VOLUMES)
        .ok_or(Failure::ProfileConfig)?;
    let catalog = Catalog {
        volumes: volumes.iter().map(site).collect::<Result<_, _>>()?,
        presence: site(&v["presence"])?,
        streamed: site(&v["streamed"])?,
        location: site(&v["location"])?,
    };
    catalog.validate().map_err(|_| Failure::ProfileConfig)?;
    Ok(catalog)
}
pub(super) fn json(catalog: &Catalog) -> Value {
    let site = |s: Site| json!({"scene_key":s.scene_key,"serializer":s.serializer});
    json!({"volumes":catalog.volumes.iter().copied().map(site).collect::<Vec<_>>(),"presence":site(catalog.presence),"streamed":site(catalog.streamed),"location":site(catalog.location)})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_sites_roundtrip_and_reject_duplicates_unknowns_and_overflow() {
        let good = json!({"volumes":[{"scene_key":123,"serializer":1}],"presence":{"scene_key":123,"serializer":2},"streamed":{"scene_key":123,"serializer":3},"location":{"scene_key":123,"serializer":4}});
        assert_eq!(json(&parse(&good).unwrap()), good);
        for (path, value) in [
            ("/volumes/0", good["presence"].clone()),
            ("/volumes/0/serializer", json!(512)),
            ("/volumes/0/scene_key", json!(1)),
            ("/volumes/0/serializer", json!(-1)),
            (
                "/volumes",
                json!(vec![good["volumes"][0].clone(); MAX_VOLUMES + 1]),
            ),
        ] {
            let mut bad = good.clone();
            *bad.pointer_mut(path).unwrap() = value;
            assert!(parse(&bad).is_err(), "{path}");
        }
        let mut bad = good;
        bad["wire_messages"] = json!([]);
        assert!(parse(&bad).is_err());
    }
}
