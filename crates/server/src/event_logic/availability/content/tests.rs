// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::event_logic::availability::State;
use nfs_content::events::discovery::{Visibility, membership::Binding};
use serde_json::json;
use std::sync::Arc;

fn site(serializer: usize) -> Endpoint {
    Endpoint {
        scene_key: 700,
        serializer,
    }
}
fn program(ops: serde_json::Value) -> Program {
    serde_json::from_value(json!({"ops":ops,"ranges":[],"empty_sets":[]})).unwrap()
}
fn source() -> Source {
    let manager = Identity {
        asset: "constructed/manager".into(),
        index: 3,
        instances: vec![],
    };
    Source {
        availability: Availability {
            entries: ["normal", "story"]
                .into_iter()
                .map(|id| Entry {
                    id: id.into(),
                    scene: "constructed/scene".into(),
                    game_mode: "constructed/mode".into(),
                    volume: site(9),
                    notifications: vec![site(1)],
                })
                .collect(),
            memberships: vec![
                Binding {
                    notification: site(1),
                    program: program(json!([{"Fact":{"Player":manager}}])),
                },
                Binding {
                    notification: site(2),
                    program: program(json!([{"Fact":{"Boolean":"story"}}])),
                },
            ],
            visibility: [("normal", false), ("story", true)]
                .into_iter()
                .map(|(id, wanted)| {
                    (
                        id.into(),
                        Visibility {
                            conditions: BTreeMap::from([(site(1), true), (site(2), wanted)]),
                        },
                    )
                })
                .collect(),
            integers: BTreeMap::new(),
            copies: BTreeMap::new(),
            channels: BTreeMap::new(),
        },
        conditions: BTreeMap::from([(
            "story".into(),
            Condition::all(&[Progress::Active(30)]).unwrap(),
        )]),
        auxiliary: StateOwners::default(),
    }
}
fn known() -> BTreeSet<Progress> {
    BTreeSet::from([Progress::Active(30), Progress::Completed(30)])
}
fn content() -> Arc<Content> {
    Arc::new(Content::new(source(), &known()).unwrap())
}

/// Constructed definition for exercising the real listener projection.
pub(crate) fn gated_fixture(event: &crate::event_logic::Content) -> Arc<Content> {
    let mut source = source();
    let e = event.event();
    let site = |serializer| Endpoint {
        scene_key: e.scene_content_key,
        serializer,
    };
    let notifications: BTreeSet<_> = e
        .area_notifications
        .iter()
        .chain(&e.scene_notifications)
        .map(|&s| site(s))
        .collect();
    let manager = Identity {
        asset: "constructed/manager".into(),
        index: 3,
        instances: vec![],
    };
    let membership = program(json!([
        {"Fact":{"Player":manager}}, {"Fact":{"Boolean":"story"}},
        {"Combine":["Intersection",0,1]}
    ]));
    source.availability.entries = vec![Entry {
        id: e.game_mode_id.clone(),
        scene: e.scene.clone(),
        game_mode: e.game_mode.layout.blueprint.clone(),
        volume: site(e.start_volume),
        notifications: notifications.iter().copied().collect(),
    }];
    source.availability.memberships = notifications
        .iter()
        .map(|&notification| Binding {
            notification,
            program: membership.clone(),
        })
        .collect();
    source.availability.visibility = BTreeMap::from([(
        e.game_mode_id.clone(),
        Visibility {
            conditions: notifications.into_iter().map(|s| (s, true)).collect(),
        },
    )]);
    Arc::new(Content::new(source, &known()).unwrap())
}

#[test]
fn eligibility_switches_variants_per_participant_and_requires_negative_inputs() {
    let mut state = State::new(content()).unwrap();
    let players = BTreeSet::from([7, 8]);
    let project = |p, f: &Fact| match f {
        Fact::Player(_) => Some(true),
        Fact::Boolean(_) => Some(p == 7),
        _ => None,
    };
    let first = state.update(&players, &[], project).unwrap();
    assert_eq!(first[&7].eligible, BTreeSet::from(["story".into()]));
    assert_eq!(first[&8].eligible, BTreeSet::from(["normal".into()]));
    assert_eq!(first[&7].notifications, BTreeSet::from([site(1), site(2)]));
    assert_eq!(first[&8].notifications, BTreeSet::from([site(1)]));
    assert_eq!(state.update(&players, &[], project).unwrap(), first);
    assert!(
        state
            .update(&players, &[], |_, f| match f {
                Fact::Player(_) => Some(false),
                _ => None,
            })
            .is_err()
    );
    let departed = state.update(&players, &[], |_, _| Some(false)).unwrap();
    assert!(
        departed
            .values()
            .all(|p| p.eligible.is_empty() && p.notifications.is_empty())
    );
    assert!(
        state
            .update(&BTreeSet::new(), &[], |_, _| None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        State::new(content())
            .unwrap()
            .update(&players, &[], project)
            .unwrap(),
        first
    );
}

#[test]
fn cached_definition_retains_private_invariants_and_complete_owner_coverage() {
    let raw = serde_json::to_vec(&source()).unwrap();
    let cached = Content::new(serde_json::from_slice(&raw).unwrap(), &known()).unwrap();
    assert_eq!(cached.entries().len(), 2);
    assert_eq!(cached.notifications(), BTreeSet::from([site(1), site(2)]));
    assert_eq!(cached.facts(), &BTreeSet::from([Progress::Active(30)]));
    let mut missing = source();
    missing.conditions.clear();
    assert!(Content::new(missing, &known()).is_err());
    assert!(Content::new(source(), &BTreeSet::new()).is_err());
    let mut missing = source();
    missing.availability.memberships.pop();
    assert!(Content::new(missing, &known()).is_err());
    let mut duplicate = source();
    duplicate
        .availability
        .entries
        .push(duplicate.availability.entries[0].clone());
    assert!(Content::new(duplicate, &known()).is_err());
    let mut corrupt: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    corrupt["conditions"]["story"]["ops"] = json!([{"Not":0}]);
    assert!(serde_json::from_value::<Source>(corrupt).is_err());
}

#[test]
fn cached_auxiliary_activation_keeps_bounded_parser_and_initial_settings() {
    let mut raw = source();
    raw.auxiliary.objectives.insert(
        30,
        nfs_content::progression::Expr::Not(Box::new(nfs_content::progression::Expr::ConsumerBeta)),
    );
    let bytes = serde_json::to_vec(&raw).unwrap();
    let restored: Source = serde_json::from_slice(&bytes).unwrap();
    let content = Content::new(restored, &BTreeSet::new()).unwrap();
    assert!(
        !content.auxiliary().objectives[&30].initial(nfs_content::progression::Settings {
            consumer_beta: true
        })
    );
    assert!(
        content.auxiliary().objectives[&30].initial(nfs_content::progression::Settings {
            consumer_beta: false
        })
    );
    let mut corrupt: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    corrupt["auxiliary"]["objectives"]["30"] = json!({"completed":0});
    assert!(serde_json::from_value::<Source>(corrupt).is_err());
}

#[test]
fn failed_projection_rolls_back_control_signals_and_full_roster_removes_departed_players() {
    use nfs_content::{
        events::discovery::{
            channels::{Publisher, Signal},
            membership::copies::Control,
        },
        scoring::nodes::Configuration,
    };
    use nfs_world::scoring::{nodes, participant_properties::Property};
    let id = |index| Identity {
        asset: "constructed/controls".into(),
        index,
        instances: vec![],
    };
    let property = Property {
        channel: "constructed/channel".into(),
        port: 91,
    };
    let signal = |event| Signal {
        source: id(20),
        event,
    };
    let mut raw = source();
    raw.availability.copies.insert(
        id(2),
        Control {
            input_owner: id(1),
            input: raw.availability.memberships[0].program.clone(),
            comparison: id(3),
            channel: property.clone(),
            comparison_config: Configuration {
                definition: nodes::Definition::CompareBoolean {
                    on_change: true,
                    on_start: false,
                    always_send: false,
                },
                defaults: vec![],
            },
        },
    );
    raw.availability.channels.insert(
        property,
        vec![Publisher {
            channel: id(4),
            input: id(5),
            configuration: Configuration {
                definition: nodes::Definition::Boolean { default: false },
                defaults: vec![],
            },
            writes: vec![(signal(1), nodes::SET_TRUE), (signal(2), nodes::SET_FALSE)],
        }],
    );
    raw.availability.memberships.push(Binding {
        notification: site(3),
        program: program(json!([{"Fact":{"Copy":id(2)}}])),
    });
    let content = Arc::new(Content::new(raw, &known()).unwrap());
    let mut state = State::new(content.clone()).unwrap();
    let player = BTreeSet::from([7]);
    let input = |_: u64, f: &Fact| match f {
        Fact::Player(_) => Some(true),
        Fact::Boolean(_) => Some(false),
        _ => None,
    };
    let before = state.update(&player, &[], input).unwrap();
    assert!(!before[&7].notifications.contains(&site(3)));
    assert!(
        state
            .update(&player, &[signal(1)], |_, f| match f {
                Fact::Player(_) => Some(true),
                _ => None,
            })
            .is_err()
    );
    assert_eq!(state.update(&player, &[], input).unwrap(), before);
    assert!(
        state.update(&player, &[signal(1)], input).unwrap()[&7]
            .notifications
            .contains(&site(3))
    );
    let joined = state.update(&BTreeSet::from([8]), &[], input).unwrap();
    assert!(!joined.contains_key(&7));
    assert!(joined[&8].notifications.contains(&site(3)));
    assert!(
        state
            .update(&BTreeSet::from([8]), &[signal(999)], input)
            .is_err()
    );
    assert_eq!(
        state.update(&BTreeSet::from([8]), &[], input).unwrap(),
        joined
    );
    assert!(
        !State::new(content)
            .unwrap()
            .update(&player, &[], input)
            .unwrap()[&7]
            .notifications
            .contains(&site(3))
    );
}
