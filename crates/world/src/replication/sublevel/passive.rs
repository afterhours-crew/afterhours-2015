// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Body, Content, Creation, Error, Initial, Kind, Prefix, Rpc, Update};
use nfs_protocol::world::rpc::Serial;

pub fn creation(
    level_id: u16,
    content_key: u32,
    serial: Serial,
    content: &Content,
) -> Result<Creation, Error> {
    let profile = content.profile(content_key)?;
    if profile.is_root() || [0, u16::MAX].contains(&level_id) {
        return Err(Error::Shape);
    }
    let mut next = 0u16;
    let mut initial = Vec::with_capacity(profile.kinds().len());
    for kind in profile.kinds() {
        initial.push(match kind {
            Kind::Noop => Initial::Empty,
            Kind::Rpc => {
                if next >= 512 {
                    return Err(Error::Bound);
                }
                let rpc = Rpc {
                    selector: next,
                    serial,
                };
                next += 1;
                Initial::Rpc(rpc)
            }
            _ => return Err(Error::Unsupported),
        });
    }
    let result = Creation {
        prefix: Prefix {
            level_id,
            content_key,
            blueprint: None,
            word_a: 0,
            word_b: 0,
            constructor_word: None,
        },
        body: Body {
            initial: Some(initial),
            updates: vec![Some(Update::Noop); profile.kinds().len()],
        },
    };
    result.encode(content)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::{state::World, sublevel::Profile};

    #[test]
    fn endpoint_allocation_is_per_scene_and_skips_serializers_without_endpoints() {
        let content = Content::new(vec![(
            7,
            Profile::new(false, &[Kind::Noop, Kind::Rpc, Kind::Noop, Kind::Rpc]).unwrap(),
        )])
        .unwrap();
        let baseline = Serial::new(1023).unwrap();
        let a = creation(17, 7, baseline, &content).unwrap();
        let b = creation(29, 7, baseline, &content).unwrap();
        assert_eq!(a.body, b.body);
        assert_eq!(
            a.body.initial,
            Some(vec![
                Initial::Empty,
                Initial::Rpc(Rpc {
                    selector: 0,
                    serial: baseline
                }),
                Initial::Empty,
                Initial::Rpc(Rpc {
                    selector: 1,
                    serial: baseline
                })
            ])
        );
        let mut world = World::default();
        world.register_levels(&[17, 29]).unwrap();
        assert_eq!(world.spawn_scene(a.clone(), &content).unwrap().id, 1);
        assert_eq!(world.spawn_scene(b, &content).unwrap().id, 2);
        assert!(world.spawn_scene(a, &content).is_err());
        assert_eq!(world.len(), 2);
    }

    #[test]
    fn no_gameplay_values_are_silently_initialized() {
        for kind in [
            Kind::LevelReference,
            Kind::BoolProperty,
            Kind::FloatProperty,
            Kind::I32Property,
            Kind::RpcReferences,
            Kind::RpcBool,
            Kind::RpcMap,
            Kind::RpcSpawn,
            Kind::RpcFourReferences,
        ] {
            let content =
                Content::new(vec![(7, Profile::new(false, &[Kind::Noop, kind]).unwrap())]).unwrap();
            assert_eq!(
                creation(1, 7, Serial::new(0).unwrap(), &content),
                Err(Error::Unsupported)
            );
        }
        let content = Content::new(vec![(7, Profile::new(true, &[Kind::Noop]).unwrap())]).unwrap();
        assert_eq!(
            creation(1, 7, Serial::new(0).unwrap(), &content),
            Err(Error::Shape)
        );
    }

    #[test]
    fn largest_supported_endpoint_profile_is_bounded_and_roundtrips() {
        let mut kinds = vec![Kind::Noop];
        kinds.extend([Kind::Rpc; super::super::MAX_SERIALIZERS - 1]);
        let content = Content::new(vec![(7, Profile::new(false, &kinds).unwrap())]).unwrap();
        let value = creation(1, 7, Serial::new(0).unwrap(), &content).unwrap();
        let wire = value.encode(&content).unwrap();
        assert_eq!(
            Creation::decode(wire.span(), &content).unwrap().creation,
            value
        );
        for id in [0, u16::MAX] {
            assert_eq!(
                creation(id, 7, Serial::new(0).unwrap(), &content),
                Err(Error::Shape)
            );
        }
    }
}
