// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::{
    participant_logic::{Field as LayoutField, Layout},
    scoring::{
        ai_deployment::{BooleanSet, Deployment, Pacing},
        content::{Field, Participant},
        events::Identity as ContentIdentity,
        participant_properties::Property,
    },
};
use nfs_world::{
    replication::entity::{Kind, creation::Asset},
    scoring::{
        interpreter::{Category, Inputs, Session},
        participant_pacing::{Library, Sources},
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
            catalog: vec![(10, 3)],
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
fn ai() -> Deployment {
    let property = |port| Property {
        channel: "constructed/channel".into(),
        port,
    };
    let id = |index| ContentIdentity {
        asset: "constructed/services".into(),
        index,
        instances: vec![],
    };
    let sources = Sources {
        pacing: property(1),
        actors: property(2),
        personalities: property(3),
        curves: property(4),
    };
    let mut participant = participant(AI_SCORING, 2);
    let program = Arc::make_mut(&mut participant.program);
    for (class, behavior) in [
        (
            "ParticipantLogicParticipantProviderEntityData",
            Behavior::ParticipantProvider,
        ),
        (
            "ParticipantPacingAccessorEntityData",
            Behavior::ParticipantPacing {
                sources: sources.clone(),
                provider: 1,
            },
        ),
        (
            "BoolParticipantPropertyAccessorEntityData",
            Behavior::ParticipantProperty {
                property: property(5),
                kind: nfs_world::scoring::properties::Kind::Boolean,
                write: true,
            },
        ),
    ] {
        let mut node = program.nodes[0].clone();
        node.identity.index = program.nodes.len() as u32;
        node.class = class.into();
        node.behavior = behavior;
        program.nodes.push(node);
    }
    Deployment {
        participant,
        eligibility: serde_json::from_value(serde_json::json!({
            "ops": [{"Fact": {"States": [{"scene_key": 101, "serializer": 3}]}}],
            "ranges": [], "empty_sets": []
        }))
        .unwrap(),
        membership: vec![],
        pacing: vec![Pacing {
            sources,
            manager: id(1),
            actors: nfs_content::events::discovery::Endpoint {
                scene_key: 101,
                serializer: 4,
            },
            personalities: id(2),
            curves: id(3),
            library: Library {
                personalities: vec![None],
                fallback: None,
            },
        }],
        boolean_sets: vec![BooleanSet {
            property: property(5),
            set: property(6),
            definition: id(4),
        }],
    }
}
fn fixture() -> ScoringContent {
    ScoringContent {
        acquisition: participant(ACQUISITION, 0),
        scoring: participant(SCORING, 1),
        ai: ai(),
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
        (&cold.ai.participant, &warm.ai.participant),
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
    assert!(warm.ai.eligibility.evaluate(|_| Some(true)).unwrap());
    assert!(!warm.ai.eligibility.evaluate(|_| Some(false)).unwrap());
    assert!(warm.ai.eligibility.evaluate(|_| None).is_err());
    assert_eq!(warm.ai.pacing, cold.ai.pacing);
    assert_eq!(warm.ai.boolean_sets, cold.ai.boolean_sets);
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
        ("/ai/participant/fields/0/node", serde_json::json!(999)),
        ("/ai/participant/layout/asset/bundle", serde_json::json!(5)),
        ("/ai/participant/blueprint", serde_json::json!("wrong/ai")),
        ("/ai/eligibility/ops", serde_json::json!([])),
        ("/ai/pacing", serde_json::json!([])),
        ("/ai/pacing/0/sources/actors/port", serde_json::json!(99)),
        ("/ai/pacing/0/actors/scene_key", serde_json::json!(0)),
        ("/ai/pacing/0/manager/asset", serde_json::json!("")),
        ("/ai/pacing/0/personalities/asset", serde_json::json!("")),
        ("/ai/pacing/0/curves/asset", serde_json::json!("")),
        ("/ai/pacing/0/library/personalities", serde_json::json!([])),
        ("/ai/boolean_sets", serde_json::json!([])),
        ("/ai/boolean_sets/0/property/port", serde_json::json!(99)),
        ("/ai/boolean_sets/0/set/port", serde_json::json!(0)),
        ("/ai/boolean_sets/0/definition/asset", serde_json::json!("")),
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

#[test]
fn ai_deployment_is_required_and_duplicate_services_or_query_sets_fail() {
    let valid = serde_json::to_value(fixture()).unwrap();
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("ai");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    for path in ["/ai/pacing", "/ai/boolean_sets"] {
        let mut repeated = valid.clone();
        let rows = repeated.pointer_mut(path).unwrap().as_array_mut().unwrap();
        rows.push(rows[0].clone());
        assert!(
            decode(&serde_json::to_vec(&repeated).unwrap()).is_err(),
            "{path}"
        );
    }
    let mut extra = fixture();
    let mut binding = extra.ai.pacing[0].clone();
    binding.sources.pacing.port = 91;
    extra.ai.pacing.push(binding);
    assert!(decode(&serde_json::to_vec(&extra).unwrap()).is_err());
}

#[test]
fn ai_membership_cache_requires_exact_complete_owner_bindings() {
    use nfs_content::events::discovery::membership::Fact;
    use nfs_content::scoring::ai_deployment::membership::{Binding, Owner};
    let id = |index| ContentIdentity {
        asset: "constructed/owner".into(),
        index,
        instances: vec![],
    };
    let mut content = fixture();
    content.ai.eligibility = serde_json::from_value(serde_json::json!({
        "ops": [{"Fact": {"Mutable": id(1)}}], "ranges": [], "empty_sets": []
    }))
    .unwrap();
    // The old missing-owner state must reject, including after cache restore.
    assert!(decode(&serde_json::to_vec(&content).unwrap()).is_err());
    content.ai.membership.push(Binding {
        fact: Fact::Mutable(id(1)),
        owner: Owner::Cops {
            dispatch: id(2),
            roadblocks: id(3),
        },
    });
    let bytes = serde_json::to_vec(&content).unwrap();
    let restored = decode(&bytes).unwrap();
    assert_eq!(restored.ai.membership, content.ai.membership);
    assert!(restored.ai.eligibility.evaluate(|_| None).is_err());
    assert!(restored.ai.eligibility.evaluate(|_| Some(true)).unwrap());
    assert!(!restored.ai.eligibility.evaluate(|_| Some(false)).unwrap());
    for replacement in [
        Binding {
            fact: Fact::Mutable(id(4)),
            owner: content.ai.membership[0].owner.clone(),
        },
        Binding {
            fact: Fact::Mutable(id(1)),
            owner: Owner::Traffic { manager: id(1) },
        },
        Binding {
            fact: Fact::Mutable(id(1)),
            owner: Owner::Cops {
                dispatch: id(2),
                roadblocks: id(2),
            },
        },
    ] {
        let mut bad = restored.clone();
        bad.ai.membership[0] = replacement;
        assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut repeated = restored;
    repeated
        .ai
        .membership
        .push(repeated.ai.membership[0].clone());
    assert!(decode(&serde_json::to_vec(&repeated).unwrap()).is_err());
    let mut missing = serde_json::to_value(content).unwrap();
    missing["ai"].as_object_mut().unwrap().remove("membership");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
}
