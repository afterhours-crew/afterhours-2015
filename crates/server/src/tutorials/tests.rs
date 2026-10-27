// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_content::{
    participant_logic::{Channel, Field, Layout, Owner},
    tutorials::Context,
};
use nfs_protocol::world::rpc::Serial;
use nfs_world::replication::{
    entity::{self, Kind as EntityKind, creation::Asset},
    players::Request,
    sublevel::{self, Kind, Profile},
};

const SCENE_KEY: u32 = 909;
const HOST: u8 = 4;
const PERSONA: u64 = 0x1_0000_0002;
const RETURN: &str = "test/intro/return";
const CLUB: &str = "test/intro/club";
const INTRO_FINISH: &str = "test/tutorial/introfinish";
const REMINDER: &str = "test/tutorial/reminder";
const CATEGORY: u32 = 380_569_058;

fn layout(local_index: u16, serializer: usize) -> Layout {
    Layout {
        asset: Asset {
            bundle: 32,
            type_id: 10,
            local_index,
        },
        catalog: vec![(10, 8)],
        scene_content_key: SCENE_KEY,
        owner: Owner::Scene,
        bus_count: 1,
        fields: vec![
            Field {
                kind: EntityKind::Root,
                channel: None,
            },
            Field {
                kind: EntityKind::RpcReference,
                channel: Some(Channel::Scene {
                    content_key: SCENE_KEY,
                    serializer,
                }),
            },
        ],
    }
}

fn tutorial(name: &str, channel: usize, gate: Gate) -> Tutorial {
    Tutorial {
        blueprint: name.into(),
        channel,
        seen: channel,
        gate,
        notification: None,
        accept: Some(1000 + channel as u32),
        context: None,
    }
}

/// An ungated, notified race-prompt tutorial (channel 3, notification 9), an
/// intro-finish tutorial 5 s after the return objective completes (channel
/// 4) and a reminder while the club objective is active after its launch
/// (channel 5).
fn content() -> Content {
    let mut a = tutorial("test/tutorial/a", 3, Gate::None);
    a.notification = Some(9);
    a.context = Some(Context {
        categories: [CATEGORY].into(),
        on_true: vec![(1, 3)],
        on_false: vec![(1, 0)],
    });
    let b = tutorial("test/tutorial/b", 4, Gate::Identified(INTRO_FINISH.into()));
    let c = tutorial("test/tutorial/c", 5, Gate::Identified(REMINDER.into()));
    let rules = vec![
        Rule {
            gate: INTRO_FINISH.into(),
            all: vec![Term::Since {
                trigger: Trigger::PrefabProperty {
                    prefab: RETURN.into(),
                    property: OBJECTIVE_COMPLETED,
                },
                seconds: 5.0,
            }],
        },
        Rule {
            gate: REMINDER.into(),
            all: vec![
                Term::While(Trigger::Objective {
                    prefab: CLUB.into(),
                    property: OBJECTIVE_ACTIVE,
                }),
                Term::Since {
                    trigger: Trigger::LauncherStarted {
                        prefab: CLUB.into(),
                    },
                    seconds: 0.0,
                },
            ],
        },
    ];
    let tutorials = [(a, 0), (b, 1), (c, 2)]
        .into_iter()
        .map(|(t, i)| {
            let spawn = Definition::new(layout(i, t.channel)).unwrap();
            (t, spawn)
        })
        .collect();
    Content::new(SCENE_KEY, tutorials, rules).unwrap()
}

fn world() -> (Players, u16) {
    let mut players = Players::default();
    let player = players
        .create(
            HOST,
            PERSONA,
            Request {
                name: b"Tutorial".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players.join(HOST, PERSONA, player).unwrap().unwrap().id;
    let mut startup = vec![Kind::RpcReferences; 12];
    startup[0] = Kind::Noop;
    let scenes =
        sublevel::Content::new(vec![(SCENE_KEY, Profile::new(false, &startup).unwrap())]).unwrap();
    players
        .create_scenes(
            &[77],
            vec![
                sublevel::ordinary::creation(
                    77,
                    SCENE_KEY,
                    None,
                    Serial::new(0).unwrap(),
                    &sublevel::ordinary::unpopulated(scenes.profile(SCENE_KEY).unwrap()).unwrap(),
                    &scenes,
                )
                .unwrap(),
            ],
            &scenes,
        )
        .unwrap();
    (players, participant)
}

fn blueprints(content: &Content, players: &Players, output: &Output) -> Vec<u16> {
    let _ = content;
    output
        .records
        .iter()
        .map(|record| {
            let Some(Initial::Entity { prefix, .. }) =
                players.objects().get(record.id).map(|o| &o.initial)
            else {
                panic!("tutorial creation is an entity");
            };
            prefix.asset.local_index
        })
        .collect()
}

fn notified(call: &HostRpc) -> (u16, bool) {
    let HostRpc::ModeNotification(n) = call else {
        panic!("a scene bool notification");
    };
    (n.endpoint.selector, n.enabled)
}

#[test]
fn gates_create_and_delete_tutorials_as_their_terms_change() {
    let content = content();
    let (mut players, participant) = world();
    let mut session = content.session(participant, BTreeSet::new());
    // Fresh account: only the ungated tutorial, bound to the participant.
    let out = session.advance(&content, &mut players, 0.0, &[]).unwrap();
    assert_eq!(blueprints(&content, &players, &out), [0]);
    let ghost = out.records[0].id;
    let Some(Initial::Entity { fields, .. }) = players.objects().get(ghost).map(|o| &o.initial)
    else {
        panic!()
    };
    assert!(matches!(fields[0], entity::Initial::Root { .. }));
    assert!(
        session
            .advance(&content, &mut players, 0.1, &[])
            .unwrap()
            .records
            .is_empty()
    );
    // The club reminder needs both its launch and its active objective.
    session.launched(&content, CLUB, 1.0);
    assert!(
        session
            .advance(&content, &mut players, 1.0, &[])
            .unwrap()
            .records
            .is_empty()
    );
    session.objective(&content, CLUB, true);
    let out = session.advance(&content, &mut players, 1.1, &[]).unwrap();
    assert_eq!(blueprints(&content, &players, &out), [2]);
    let reminder = out.records[0].id;
    session.objective(&content, CLUB, false);
    let out = session.advance(&content, &mut players, 2.0, &[]).unwrap();
    assert_eq!(out.deleted, [reminder]);
    assert!(players.objects().get(reminder).is_none());
    // A second launch does not move the first rise; the intro finish waits
    // its 5 s from the completion.
    session.completed(&content, RETURN, 10.0);
    session.completed(&content, RETURN, 12.0);
    assert!(
        session
            .advance(&content, &mut players, 14.9, &[])
            .unwrap()
            .records
            .is_empty()
    );
    let out = session.advance(&content, &mut players, 15.0, &[]).unwrap();
    assert_eq!(blueprints(&content, &players, &out), [1]);
}

#[test]
fn context_rows_fire_on_changes_of_the_held_categories_only() {
    let content = content();
    let (mut players, participant) = world();
    let mut session = content.session(participant, BTreeSet::new());
    let ghost = session
        .advance(&content, &mut players, 0.0, &[])
        .unwrap()
        .records[0]
        .id;
    let fires = |out: &Output| -> Vec<(u16, u32, u32)> {
        out.calls
            .iter()
            .map(|c| match c {
                HostRpc::Event(f) => (f.target.ghost, f.target.entity, f.event),
                _ => panic!("a logic fire"),
            })
            .collect()
    };
    let other: BTreeSet<u32> = [7].into();
    let race: BTreeSet<u32> = [7, CATEGORY].into();
    // The reporter starts false: no fire until it changes.
    assert!(
        fires(
            &session
                .advance(&content, &mut players, 0.1, &[&other])
                .unwrap()
        )
        .is_empty()
    );
    let out = session
        .advance(&content, &mut players, 0.2, &[&race])
        .unwrap();
    assert_eq!(fires(&out), [(ghost, 1, 3)]);
    assert!(
        fires(
            &session
                .advance(&content, &mut players, 0.3, &[&race])
                .unwrap()
        )
        .is_empty()
    );
    let out = session
        .advance(&content, &mut players, 0.4, &[&other])
        .unwrap();
    assert_eq!(fires(&out), [(ghost, 1, 0)]);
}

#[test]
fn an_accept_commits_then_deletes_notifies_and_survives_a_restart() {
    let content = content();
    let (mut players, participant) = world();
    let mut session = content.session(participant, BTreeSet::new());
    let ghost = session
        .advance(&content, &mut players, 0.0, &[])
        .unwrap()
        .records[0]
        .id;
    let target = |entity| EntityRef { ghost, entity };
    // Wrong id, entity or player is refused; another ghost is not ours.
    assert!(session.accept(&content, 1004, target(1), 0).is_err());
    assert!(session.accept(&content, 1003, target(2), 0).is_err());
    assert!(session.accept(&content, 1003, target(1), 1).is_err());
    assert_eq!(
        session
            .accept(
                &content,
                1003,
                EntityRef {
                    ghost: ghost + 50,
                    entity: 1
                },
                0
            )
            .unwrap(),
        None
    );
    let key = session
        .accept(&content, 1003, target(1), 0)
        .unwrap()
        .unwrap();
    assert_eq!(key, crate::persistent::key("test/tutorial/a"));
    // A repeat while the commit is outstanding asks for nothing more, and
    // the logic stays until the commit.
    assert_eq!(session.accept(&content, 1003, target(1), 0).unwrap(), None);
    let out = session.advance(&content, &mut players, 0.1, &[]).unwrap();
    assert!(out.deleted.is_empty() && out.calls.is_empty());
    // A failed commit leaves it live for another accept.
    session.failed(&content, key);
    assert_eq!(
        session.accept(&content, 1003, target(1), 0).unwrap(),
        Some(key)
    );
    session.committed(&content, key);
    let out = session.advance(&content, &mut players, 0.2, &[]).unwrap();
    assert_eq!(out.deleted, [ghost]);
    let selector = content.endpoint(players.objects(), 9).unwrap().selector;
    assert_eq!(
        out.calls.iter().map(notified).collect::<Vec<_>>(),
        [(selector, true)]
    );
    assert!(!session.owns(ghost));
    // A repeated commit answer notifies nothing again.
    session.committed(&content, key);
    assert!(
        session
            .advance(&content, &mut players, 0.3, &[])
            .unwrap()
            .calls
            .is_empty()
    );
    // Restart: the stored seen set spawns nothing and the flush reports it.
    let (mut restarted, participant) = world();
    let seen = session.seen().clone();
    assert_eq!(
        content
            .notifications(restarted.objects(), &seen)
            .unwrap()
            .iter()
            .map(notified)
            .collect::<Vec<_>>(),
        [(selector, true)]
    );
    let mut again = content.session(participant, seen);
    assert!(
        again
            .advance(&content, &mut restarted, 0.0, &[])
            .unwrap()
            .records
            .is_empty()
    );
    // A fresh account's flush sends the unseen value too.
    assert_eq!(
        content
            .notifications(restarted.objects(), &BTreeSet::new())
            .unwrap()
            .iter()
            .map(notified)
            .collect::<Vec<_>>(),
        [(selector, false)]
    );
}

#[test]
fn participants_keep_separate_tutorials() {
    let content = content();
    let (mut players, first) = world();
    let player = players
        .create(
            HOST,
            PERSONA,
            Request {
                name: b"Second".to_vec(),
                flag: false,
                slot: 1,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let second = players.join(HOST, PERSONA, player).unwrap().unwrap().id;
    let mut a = content.session(first, BTreeSet::new());
    let mut b = content.session(second, BTreeSet::new());
    let ghost_a = a.advance(&content, &mut players, 0.0, &[]).unwrap().records[0].id;
    let ghost_b = b.advance(&content, &mut players, 0.0, &[]).unwrap().records[0].id;
    assert_ne!(ghost_a, ghost_b);
    assert_eq!(
        b.accept(
            &content,
            1003,
            EntityRef {
                ghost: ghost_a,
                entity: 1
            },
            0
        )
        .unwrap(),
        None
    );
    let key = a
        .accept(
            &content,
            1003,
            EntityRef {
                ghost: ghost_a,
                entity: 1,
            },
            0,
        )
        .unwrap()
        .unwrap();
    a.committed(&content, key);
    b.committed(&content, key);
    assert_eq!(
        a.advance(&content, &mut players, 0.1, &[]).unwrap().deleted,
        [ghost_a]
    );
    // The other participant never accepted: its answer for that key cannot
    // count, because its own logic is not accepting.
    assert!(b.owns(ghost_b));
}

#[test]
fn unsupported_triggers_and_mismatched_channels_are_rejected() {
    let a = tutorial("test/tutorial/a", 3, Gate::None);
    let spawn = Definition::new(layout(0, 3)).unwrap();
    let wrong = Definition::new(layout(0, 4)).unwrap();
    assert!(Content::new(SCENE_KEY, vec![(a.clone(), wrong)], vec![]).is_err());
    assert!(
        Content::new(
            SCENE_KEY,
            vec![(a.clone(), spawn.clone()), (a.clone(), spawn.clone())],
            vec![]
        )
        .is_err()
    );
    for term in [
        Term::Since {
            trigger: Trigger::PrefabEvent {
                prefab: RETURN.into(),
                event: 5,
            },
            seconds: 0.0,
        },
        Term::While(Trigger::LauncherStarted {
            prefab: CLUB.into(),
        }),
        Term::Since {
            trigger: Trigger::Objective {
                prefab: CLUB.into(),
                property: OBJECTIVE_ACTIVE,
            },
            seconds: 1.0,
        },
    ] {
        let rule = Rule {
            gate: REMINDER.into(),
            all: vec![term],
        };
        assert!(Content::new(SCENE_KEY, vec![(a.clone(), spawn.clone())], vec![rule]).is_err());
    }
}
