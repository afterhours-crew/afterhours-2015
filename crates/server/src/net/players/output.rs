// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_world::{replication::section::Section, session::coalesce_motion};

/// Pack only adjacent snapshots produced by one input frame. This does not
/// combine queued frames across callbacks or alter any reliable notification.
pub(super) fn pack_motion(sections: Vec<Section>) -> Vec<Section> {
    let mut packed = Vec::new();
    for section in sections {
        if let Some(tail) = packed.last_mut()
            && let Some(combined) =
                coalesce_motion(tail, &section, nfs_world::application::OUTBOUND_FRAME_BITS)
        {
            *tail = combined;
        } else {
            packed.push(section);
        }
    }
    packed
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world::replication::{
        Record, Update,
        entity::creation::{Asset, Catalog, Content},
        section::Setup,
        vehicle,
    };
    use std::sync::Arc;

    fn snapshots() -> impl Fn(u16, u32) -> Section {
        let asset = Asset {
            bundle: 3,
            type_id: 4,
            local_index: 0,
        };
        let content = Content::new(vec![Catalog::new(3, &[(4, 1)]).unwrap()], vec![])
            .unwrap()
            .with_vehicles(vec![(
                asset,
                vehicle::Profile::new(vec![
                    vehicle::Kind::Root {
                        property_owner: false,
                    },
                    vehicle::Kind::Chassis,
                ])
                .unwrap(),
            )])
            .unwrap();
        let binding = Arc::new(content.binding(asset).unwrap());
        move |id, revision| {
            let physics = vehicle::chassis::Motion {
                identity: 1f64.to_bits(),
                revision,
                position: [id as f32, 0., 0.],
                quaternion: [0., 0., 0., 1.],
                velocity: [1., 0., 0.],
                angular_velocity: [0.; 3],
            }
            .encode([0.; 3], &vehicle::chassis::Controls::new(4).unwrap())
            .unwrap();
            Section {
                float_bits: None,
                flag: false,
                deleted: vec![],
                setup: Some(Setup::RawEscape([0; 3])),
                records: vec![Record {
                    id,
                    initial: None,
                    update: Update::Vehicle {
                        binding: binding.clone(),
                        fields: vec![
                            None,
                            Some(vehicle::Update::Chassis(Box::new(vehicle::ChassisUpdate {
                                physics: Some(physics),
                                flag: None,
                                custom: None,
                                pair: None,
                            }))),
                        ],
                    },
                }],
            }
        }
    }

    #[test]
    fn a_delegated_movement_frame_does_not_require_one_queue_slot_per_car() {
        let snapshot = snapshots();
        let count = super::super::MAX_PENDING as u16;
        let packed = pack_motion((1..=count).map(|id| snapshot(id, 1)).collect());
        assert!(packed.len() < super::super::MAX_PENDING);
        assert_eq!(
            packed.iter().map(|s| s.records.len()).sum::<usize>(),
            count as usize
        );
        for mut section in packed {
            section.float_bits = Some(0);
            section.flag = true;
            assert!(
                nfs_world::session::replication_frame(&section, &[], 0)
                    .unwrap()
                    .len()
                    <= nfs_world::application::OUTBOUND_FRAME_BITS
            );
        }
    }

    #[test]
    fn lifecycle_and_origin_changes_preserve_their_snapshot_boundaries() {
        let snapshot = snapshots();
        let mut deletion = snapshot(1, 2);
        deletion.records.clear();
        deletion.deleted.push(3);
        let mut relative = snapshot(1, 4);
        relative.setup = Some(Setup::Packed {
            tag: None,
            width: None,
            axes: [None; 3],
        });
        let before = snapshot(1, 1);
        let after = snapshot(1, 3);
        let last = snapshot(1, 5);
        assert_eq!(
            pack_motion(vec![
                before.clone(),
                deletion.clone(),
                after.clone(),
                relative.clone(),
                last.clone()
            ]),
            [before, deletion, after, relative, last]
        );
    }
}
