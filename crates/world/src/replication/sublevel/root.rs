// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Body, Content, Creation, Error, Initial, Kind, Prefix, Profile, Rpc, Update};

pub const REFERENCES: usize = 10;

/// Validate the distinct deployment keys assigned to the root's traffic slots.
pub fn validate_traffic_keys(keys: &[u32; REFERENCES]) -> Result<(), Error> {
    for (i, key) in keys.iter().enumerate() {
        if *key <= 1 || keys[..i].contains(key) {
            return Err(Error::Shape);
        }
    }
    Ok(())
}

pub fn traffic_content(traffic_keys: &[u32; REFERENCES]) -> Result<Content, Error> {
    validate_traffic_keys(traffic_keys)?;
    let mut profiles = vec![(1, content()?.profile(1)?.clone())];
    for &key in traffic_keys {
        profiles.push((key, Profile::new(false, &[Kind::Noop])?));
    }
    Content::new(profiles)
}

pub fn traffic_creation(
    traffic_keys: &[u32; REFERENCES],
    slot: usize,
    level_id: u16,
) -> Result<Creation, Error> {
    if [0, u16::MAX].contains(&level_id) {
        return Err(Error::Shape);
    }
    let result = Creation {
        prefix: Prefix {
            level_id,
            content_key: *traffic_keys.get(slot).ok_or(Error::Bound)?,
            blueprint: None,
            word_a: 0,
            word_b: 0,
            constructor_word: None,
        },
        body: Body {
            initial: Some(vec![Initial::Empty]),
            updates: vec![Some(Update::Noop)],
        },
    };
    result.encode(&traffic_content(traffic_keys)?)?;
    Ok(result)
}

pub fn linked_creations(
    rpc: Rpc,
    bindings: &crate::content::bindings::Bindings,
    traffic_keys: &[u32; REFERENCES],
) -> Result<Vec<Creation>, Error> {
    let mut levels = [None; REFERENCES];
    let mut traffic = Vec::with_capacity(REFERENCES);
    for (slot, key) in traffic_keys.iter().copied().enumerate() {
        let level = bindings.root_child(key).map_err(|_| Error::UnknownObject)?;
        levels[slot] = Some(level.id);
        traffic.push(traffic_creation(traffic_keys, slot, level.id)?);
    }
    let mut creations = vec![creation(rpc, levels)?];
    creations.extend(traffic);
    Ok(creations)
}

pub fn content() -> Result<Content, Error> {
    let mut kinds = vec![Kind::Noop, Kind::Rpc];
    kinds.extend([Kind::LevelReference; REFERENCES]);
    Content::new(vec![(1, Profile::new(true, &kinds)?)])
}

pub fn creation(rpc: Rpc, levels: [Option<u16>; REFERENCES]) -> Result<Creation, Error> {
    let mut initial = vec![Initial::Empty, Initial::Rpc(rpc)];
    initial.extend([const { Initial::Empty }; REFERENCES]);
    let mut updates = vec![Some(Update::Noop), Some(Update::Noop)];
    updates.extend(levels.map(|level| level.map(|id| Update::LevelReference(Some(id)))));
    let result = Creation {
        prefix: Prefix {
            level_id: 0,
            content_key: 1,
            blueprint: None,
            word_a: 0,
            word_b: 0,
            constructor_word: None,
        },
        body: Body {
            initial: Some(initial),
            updates,
        },
    };
    result.encode(&content()?)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::{Update as RecordUpdate, state::World};
    use nfs_protocol::world::rpc::Serial;

    fn rpc(selector: u16) -> Rpc {
        Rpc {
            selector,
            serial: Serial::new(0).unwrap(),
        }
    }

    #[test]
    fn fresh_root_owns_only_explicit_links_and_checks_registration() {
        let mut world = World::default();
        world.register_levels(&[0]).unwrap();
        let mut levels = [None; REFERENCES];
        levels[3] = Some(41);
        let creation = creation(rpc(7), levels).unwrap();
        assert_eq!(
            world.spawn_scene(creation.clone(), &content().unwrap()),
            Err(Error::UnknownObject)
        );
        assert!(world.is_empty());
        world.register_levels(&[41]).unwrap();
        let record = world.spawn_scene(creation, &content().unwrap()).unwrap();
        assert_eq!(record.id, 1);
        assert_eq!(world.snapshot(), vec![record.clone()]);
        let RecordUpdate::SubLevel { fields, .. } = record.update else {
            panic!()
        };
        assert_eq!(fields[5], Some(Update::LevelReference(Some(41))));
        assert!(fields[2..5].iter().all(Option::is_none));
        assert!(fields[6..].iter().all(Option::is_none));
    }

    #[test]
    fn fresh_root_is_deterministic_and_does_not_supply_captured_rpc_values() {
        let value = creation(rpc(0), [None; REFERENCES]).unwrap();
        let wire = value.encode(&content().unwrap()).unwrap();
        assert_eq!(wire.len(), 144);
        assert_eq!(
            Creation::decode(wire.span(), &content().unwrap())
                .unwrap()
                .creation,
            value
        );
        assert!(creation(rpc(512), [None; REFERENCES]).is_err());
    }
}
