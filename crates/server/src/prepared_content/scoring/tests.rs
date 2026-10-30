// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::{
    participant_logic::{Field as LayoutField, Layout},
    scoring::content::{Field, Participant},
};
use nfs_world::{
    replication::entity::{Kind, creation::Asset},
    scoring::{
        interpreter::{Category, Inputs, Session},
        program::{Behavior, Identity, Node, Program},
    },
};

fn participant(blueprint: &str, local_index: u16) -> Participant {
    Participant {
        blueprint: blueprint.into(),
        program: Arc::new(Program {
            nodes: vec![Node {
                identity: Identity {
                    asset: blueprint.into(),
                    index: 0,
                    instances: vec![],
                },
                class: "ParticipantLogicEntityData".into(),
                created: true,
                events: true,
                properties: true,
                forward: None,
                defaults: vec![],
                behavior: Behavior::Inert,
            }],
            cells: vec![],
            connections: vec![],
            startup: vec![],
            unmodeled: vec![],
        }),
        layout: Layout {
            asset: Asset {
                bundle: 4,
                type_id: 10,
                local_index,
            },
            catalog: vec![(10, 2)],
            scene_content_key: 101,
            owner: Owner::Scene,
            bus_count: 1,
            fields: vec![LayoutField {
                kind: Kind::Root,
                channel: None,
            }],
        },
        fields: vec![Field {
            node: 0,
            serializer: 0,
            kind: Kind::Root,
        }],
        bus_entities: vec![Some(0)],
    }
}
fn fixture() -> ScoringContent {
    ScoringContent {
        acquisition: participant(ACQUISITION, 0),
        scoring: participant(SCORING, 1),
        categories: [
            Category::Total,
            Category::Build,
            Category::Crew,
            Category::Outlaw,
            Category::Speed,
            Category::Style,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, category)| (index as u32 + 10, category))
        .collect(),
    }
}

#[test]
fn scoring_cache_restores_programs_and_bindings_but_never_live_sessions() {
    let dir = crate::prepared_content::tests::Directory::new();
    let bytes = serde_json::to_vec(&fixture()).unwrap();
    let cold = dir
        .cache()
        .prepare("scoring", b"scene", decode, || Ok(bytes.clone()))
        .unwrap();
    let warm = dir
        .cache()
        .prepare("scoring", b"scene", decode, || {
            panic!("warm scoring compiled")
        })
        .unwrap();
    assert_eq!(format!("{cold:?}"), format!("{warm:?}"));
    for (original, cached) in [
        (&cold.acquisition, &warm.acquisition),
        (&cold.scoring, &warm.scoring),
    ] {
        let mut first = Session::new(original.program.clone()).unwrap();
        let second = Session::new(cached.program.clone()).unwrap();
        first
            .start(&Inputs {
                participant: 1,
                vehicle: None,
                others: vec![],
                participants: vec![],
                road: None,
                stats: None,
                now: 0.0,
                time_scale: 1.0,
            })
            .unwrap();
        assert!(first.started());
        assert!(!second.started());
        assert_eq!(original.layout, cached.layout);
        assert_eq!(original.fields, cached.fields);
    }
}

#[test]
fn invalid_scoring_programs_layouts_and_categories_fail_before_publication() {
    let valid = serde_json::to_value(fixture()).unwrap();
    let mutations = [
        (
            "/acquisition/program/nodes/0/forward",
            serde_json::json!(999),
        ),
        ("/scoring/fields/0/node", serde_json::json!(999)),
        ("/scoring/fields/0/serializer", serde_json::json!(1)),
        ("/scoring/layout/catalog", serde_json::json!([])),
        ("/categories", serde_json::json!({})),
        (
            "/acquisition/blueprint",
            serde_json::json!("wrong/blueprint"),
        ),
    ];
    for (path, value) in mutations {
        let mut bad = valid.clone();
        *bad.pointer_mut(path).unwrap() = value;
        let dir = crate::prepared_content::tests::Directory::new();
        assert!(
            dir.cache()
                .prepare("scoring", b"scene", decode, || Ok(
                    serde_json::to_vec(&bad).unwrap()
                ))
                .is_err(),
            "{path}"
        );
        // A failed builder must not poison the key for the repaired model.
        dir.cache()
            .prepare("scoring", b"scene", decode, || {
                Ok(serde_json::to_vec(&valid).unwrap())
            })
            .unwrap();
    }
    let bytes = serde_json::to_vec(&valid).unwrap();
    assert!(decode(&bytes[..bytes.len() - 1]).is_err());
    let mut concatenated = bytes.clone();
    concatenated.extend(&bytes);
    assert!(decode(&concatenated).is_err());
}
