// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::participant_logic::{Channel, Field as LayoutField, Layout, Owner};
use nfs_world::replication::entity::creation::Asset;
use nfs_world::scoring::program::{Behavior, Identity, Node, Program};

fn node(index: u32, class: &str, behavior: Behavior) -> Node {
    Node {
        identity: Identity {
            asset: "test".into(),
            index,
            instances: vec![],
        },
        class: class.into(),
        created: true,
        events: true,
        properties: true,
        forward: None,
        defaults: vec![],
        behavior,
    }
}

/// Root(0) + SyncedFloat(1) + SyncedBool(2) + NearMiss rpc(3) + Collision rpc_bool(4).
fn participant(local_index: u16) -> Participant {
    let program = Program {
        nodes: vec![
            node(0, "ParticipantLogicEntityData", Behavior::Inert),
            node(
                1,
                "SyncedFloatEntityData",
                Behavior::Synced(synced::Kind::Float),
            ),
            node(
                2,
                "SyncedBoolEntityData",
                Behavior::Synced(synced::Kind::Boolean),
            ),
            node(3, "RaceVehicleNearMissQueryEntityData", Behavior::NearMiss),
            node(
                4,
                "RaceVehicleCollisionQueryEntityData",
                Behavior::Collision(nfs_world::scoring::collision::Outputs {
                    querying: false,
                    value: false,
                    suppress_equal_querying: true,
                }),
            ),
        ],
        cells: vec![],
        connections: vec![],
        startup: vec![],
        unmodeled: vec![],
    };
    let kinds = [
        Kind::Root,
        Kind::FloatProperty,
        Kind::BoolProperty,
        Kind::Rpc,
        Kind::RpcBool,
    ];
    Participant {
        blueprint: format!("test/{local_index}"),
        program: Arc::new(program),
        layout: Layout {
            asset: Asset {
                bundle: 4,
                type_id: 10,
                local_index,
            },
            catalog: vec![(10, 4)],
            scene_content_key: 101,
            owner: Owner::Scene,
            bus_count: 1,
            fields: kinds
                .iter()
                .map(|kind| LayoutField {
                    kind: *kind,
                    channel: None::<Channel>,
                })
                .collect(),
        },
        fields: kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| Field {
                node: i as u16,
                serializer: i,
                kind: *kind,
            })
            .collect(),
    }
}

fn content() -> Content {
    let categories = [
        (10, Category::Total),
        (11, Category::Build),
        (12, Category::Crew),
        (13, Category::Outlaw),
        (14, Category::Speed),
        (15, Category::Style),
    ]
    .into_iter()
    .collect();
    Content::new(Arc::new(ScoringContent {
        acquisition: participant(0),
        scoring: participant(1),
        categories,
    }))
    .unwrap()
}

#[test]
fn credit_sums_per_category_and_rejects_unknown_or_negative_keepers() {
    let content = content();
    let award = |keeper, amount| Award {
        keeper,
        participant: 1,
        amount,
    };
    let credit = content
        .credit(&[award(10, 300), award(14, 200), award(14, 100), award(15, 0)])
        .unwrap()
        .unwrap();
    assert_eq!(
        (credit.total, credit.speed, credit.style, credit.build),
        (300, 300, 0, 0)
    );
    assert_eq!(content.credit(&[award(10, 0), award(15, 0)]).unwrap(), None);
    assert_eq!(content.credit(&[]).unwrap(), None);
    assert!(content.credit(&[award(99, 5)]).is_err());
    assert!(content.credit(&[award(10, 5), award(10, -6)]).is_err());
    assert!(
        content
            .credit(&[award(10, i32::MAX), award(10, i32::MAX)])
            .is_err()
    );
}

#[test]
fn updates_map_dirty_values_to_serializer_fields_with_their_kinds() {
    let content = content();
    let update = content
        .update(
            Which::Scoring,
            &[(2, Value::Boolean(true)), (1, Value::Float(2.5))],
        )
        .unwrap()
        .unwrap();
    let RecordUpdate::Entity { binding, fields } = update else {
        panic!("entity update");
    };
    assert_eq!(binding.asset().local_index, 1);
    assert_eq!(
        fields,
        vec![
            None,
            Some(EntityUpdate::FloatProperty(Some(2.5f32.to_bits()))),
            Some(EntityUpdate::BoolProperty(Some(true))),
            None,
            None
        ]
    );
    assert!(content.update(Which::Scoring, &[]).unwrap().is_none());
    assert!(
        content
            .update(Which::Scoring, &[(1, Value::Boolean(true))])
            .is_err()
    );
    assert!(
        content
            .update(Which::Scoring, &[(3, Value::Float(1.0))])
            .is_err()
    );
    assert!(
        content
            .update(Which::Scoring, &[(9, Value::Float(1.0))])
            .is_err()
    );
}

#[test]
fn content_requires_one_shared_catalog() {
    let mut other = participant(1);
    other.layout.asset.bundle = 5;
    assert!(
        Content::new(Arc::new(ScoringContent {
            acquisition: participant(0),
            scoring: other,
            categories: BTreeMap::new(),
        }))
        .is_err()
    );
}
