// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Synthetic event content and world: layouts follow the shapes the installed event
//! resolves to (E996), with invented keys, indices and assets; no captured values.
use super::*;
use nfs_content::events::{GameMode, Goto, Rewards};
use nfs_world::{
    application::HOST_SELECTOR,
    replication::{
        Record,
        players::{Players, Request},
        sublevel::{Content as SceneContent, Kind as SceneKind, Profile, ordinary},
    },
};

pub(crate) const RACE: u32 = 900;
pub(crate) const GAMEPLAY: u32 = 901;
pub(crate) const EVENTS: u32 = 902;
pub(crate) const PERSONA: u64 = 77;

fn field(kind: Kind, class: &str, asset: &str, object: usize, binding: Option<Binding>) -> Field {
    Field {
        kind,
        class: class.into(),
        asset: asset.into(),
        object,
        binding,
        mirrors: None,
    }
}
fn category_property() -> Property {
    Property {
        channel: "gameplay/scoring/eventresultsdrivingstylescorechannel".into(),
        port: 2_758_283_283,
    }
}
fn scene(serializer: usize) -> Option<Binding> {
    Some(Binding::Scene {
        content_key: RACE,
        serializer,
    })
}
fn layout(name: &str, local_index: u16, sub_id: u32, fields: Vec<Field>) -> Layout {
    Layout {
        blueprint: name.into(),
        asset: Asset {
            bundle: 4,
            type_id: 10,
            local_index,
        },
        catalog: vec![(10, 40)],
        bus_count: 1,
        sub_id,
        occurrence: 0,
        fields,
    }
}
fn logic(name: &str, local_index: u16, sub_id: u32, extra: Vec<Field>) -> Layout {
    let mut fields = vec![
        field(Kind::Root, "ParticipantLogicEntityData", name, 0, None),
        field(
            Kind::RpcReference,
            "LinkedSchematicChannelReferenceEntityData",
            name,
            1,
            scene(9),
        ),
    ];
    fields.extend(extra);
    layout(name, local_index, sub_id, fields)
}

/// The game mode: root, a channel, every lifecycle state, both sequence
/// spawners, the route provider, the GUID listener and the outro provider.
fn game_mode() -> Layout {
    let mut fields = vec![
        field(
            Kind::GameModeRoot,
            "GameModeEntityData",
            "gameplay/gamemodes/racegamemode",
            4,
            None,
        ),
        field(
            Kind::RpcReference,
            "LinkedSchematicChannelReferenceEntityData",
            "gameplay/gamemodes/racegamemode",
            5,
            scene(8),
        ),
    ];
    for (state, asset, class) in STATES {
        let kind = match state {
            State::Countdown => Kind::CountdownState,
            State::Waypoints => Kind::WaypointsState,
            State::Dnf => Kind::DnfTransition,
            State::DnfTriggered | State::LocalPosition => Kind::Rpc,
            _ => Kind::RpcReferences,
        };
        fields.push(field(kind, class, asset, 1, None));
    }
    for asset in ["intro", "outro"] {
        fields.push(field(
            Kind::RpcReferences,
            "ParticipantSequenceSpawnerGameModeStateEntityData",
            asset,
            8,
            None,
        ));
    }
    fields.push(field(
        Kind::WaypointList,
        "ChannelWaypointListProviderEntityData",
        "gameplay/gamemodes/racegamemode",
        19,
        None,
    ));
    fields.push(field(
        Kind::RpcGuid,
        "DurangoEventListenerEntityData",
        "gameplay/gamemodes/racegamemode",
        22,
        None,
    ));
    fields.push(field(
        Kind::RpcSet64,
        "EventOutroPlayerProviderEntityData",
        "eventresultpresentationprefab",
        1,
        None,
    ));
    fields.push(field(
        Kind::RpcReferences,
        "WaitForAllAiToSpawnStateEntityData",
        "synthetic/ai-start",
        0,
        None,
    ));
    let mut l = layout("gameplay/gamemodes/racegamemode", 0, 2, fields);
    l.bus_count = 12;
    l
}
const INTRO_SPAWNER: usize = 13;
pub(crate) const GOTO: Goto = Goto {
    point: [10.5, 2.0, -30.25],
    radius: 150.,
    speed_limit: 22.35,
};
/// The in-event flags: five free serializers of the three scenes.
pub(crate) const FLAGS: [(u32, usize); 5] = [
    (EVENTS, 2),
    (GAMEPLAY, 5),
    (RACE, 12),
    (EVENTS, 1),
    (GAMEPLAY, 3),
];
const OUTRO_SPAWNER: usize = 14;

pub(crate) fn event() -> Event {
    let gm = game_mode();
    let names = [
        request::SOUND,
        request::INTRO,
        request::TELEMETRY,
        request::RESTART,
        request::LEAVE,
        request::RESULT_TRACKING,
        request::ONGOING_SCORE,
        request::ADD_SET,
        request::ADD_SET,
    ];
    let mut game_mode_logics: Vec<Layout> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut l = logic(name, 10 + i as u16, i as u32 + 2, vec![]);
            l.fields[1].binding = Some(Binding::GameMode { serializer: 1 });
            l
        })
        .collect();
    game_mode_logics[8].occurrence = 1;
    let mut driving = logic(
        request::DRIVING_STYLE,
        20,
        2,
        vec![
            field(
                Kind::RpcBool,
                "RaceVehicleCollisionQueryEntityData",
                "d",
                2,
                None,
            ),
            field(
                Kind::RpcBool,
                "RaceVehicleCollisionQueryEntityData",
                "d",
                3,
                None,
            ),
            field(Kind::FloatProperty, "SyncedFloatEntityData", "d", 4, None),
            field(
                Kind::RpcBool,
                "RaceVehicleCollisionQueryEntityData",
                "d",
                5,
                None,
            ),
        ],
    );
    driving.fields[1].binding = Some(Binding::GameMode { serializer: 1 });
    game_mode_logics.push(driving);
    game_mode_logics.push(logic(
        request::AWARDS,
        21,
        2,
        vec![field(
            Kind::I32Property,
            "SyncedIntEntityData",
            "a",
            2,
            None,
        )],
    ));
    game_mode_logics.push(logic(
        request::OUTRO,
        22,
        7,
        vec![
            Field {
                mirrors: Some(category_property()),
                ..field(
                    Kind::I32Property,
                    "SyncedIntEntityData",
                    "gameplay/scoring/eventresultscorelogic",
                    7,
                    None,
                )
            },
            field(
                Kind::I32Property,
                "SyncedIntEntityData",
                "gameplay/scoring/eventresultscorelogic",
                4,
                None,
            ),
            field(
                Kind::FloatProperty,
                "SyncedFloatEntityData",
                "gameplay/scoring/eventresultscorelogic",
                9,
                None,
            ),
            field(
                Kind::I32Property,
                "SyncedIntEntityData",
                "gameplay/scoring/raceeventoutrologic",
                9,
                None,
            ),
            field(
                Kind::I32Property,
                "SyncedIntEntityData",
                "gameplay/scoring/raceeventoutrologic",
                6,
                None,
            ),
            field(
                Kind::I32Property,
                "SyncedIntEntityData",
                "gameplay/scoring/speedlistcashrewardsprefab",
                2,
                None,
            ),
            field(
                Kind::I32Property,
                "SyncedIntEntityData",
                "gameplay/progression/groupplaybonuslogic",
                3,
                None,
            ),
            field(
                Kind::I32Property,
                "SyncedIntEntityData",
                "unlisted",
                1,
                None,
            ),
        ],
    ));
    let start_logics = vec![
        logic(
            request::START_AREA,
            1,
            12,
            vec![field(
                Kind::BoolProperty,
                "SyncedBoolEntityData",
                "s",
                2,
                None,
            )],
        ),
        logic(request::SHOW_START, 2, 12, vec![]),
        logic(
            request::SHOW_PARTICIPANTS,
            3,
            12,
            vec![field(
                Kind::I32Property,
                "SyncedIntEntityData",
                "s",
                2,
                None,
            )],
        ),
        logic(request::EXTERNAL_FILTER, 4, 12, vec![]),
        logic(request::STARTING, 5, 12, vec![]),
        logic(request::LOGIC_TELEMETRY, 6, 13, vec![]),
    ];
    let mut linked = logic(request::EXTERNAL_FILTER, 4, 9, vec![]);
    linked.fields[1].binding = scene(10);
    let sequence = |name: &str, local_index: u16, fields: Vec<Field>, spawner: Option<usize>| {
        nfs_content::events::Sequence {
            layout: layout(name, local_index, 2, fields),
            manager: (GAMEPLAY, 4),
            spawner,
            goto: (name == request::INTRO_SEQUENCE).then_some(GOTO),
            wander: None,
        }
    };
    let root = || {
        field(
            Kind::SequenceRoot,
            "ParticipantStateSequenceEntityData",
            "q",
            0,
            None,
        )
    };
    let sequences = vec![
        sequence(
            request::INTRO_SEQUENCE,
            30,
            vec![
                root(),
                field(
                    Kind::RpcOptionalReference,
                    "ParticipantProviderEntityData",
                    "q",
                    1,
                    None,
                ),
                field(
                    Kind::RpcReference,
                    "LinkedSchematicChannelReferenceEntityData",
                    "q",
                    2,
                    Some(Binding::GameMode { serializer: 1 }),
                ),
                field(
                    Kind::RpcReferences,
                    "ParticipantStateEntityData",
                    "q",
                    3,
                    None,
                ),
            ],
            Some(INTRO_SPAWNER),
        ),
        sequence(request::COUNTDOWN_SEQUENCE, 31, vec![root()], None),
        sequence(
            request::OUTRO_SEQUENCE,
            32,
            vec![
                root(),
                field(
                    Kind::RpcOptionalReference,
                    "ParticipantProviderEntityData",
                    "q",
                    1,
                    None,
                ),
                field(
                    Kind::RpcReference,
                    "LinkedSchematicChannelReferenceEntityData",
                    "q",
                    2,
                    scene(11),
                ),
                field(
                    Kind::RpcReferences,
                    "ParticipantStateEntityData",
                    "q",
                    3,
                    None,
                ),
            ],
            Some(OUTRO_SPAWNER),
        ),
    ];
    let scene_logic = |name: &str, local_index: u16| {
        let mut l = logic(name, local_index, 1, vec![]);
        l.asset.bundle = 5;
        l.catalog = vec![(10, 2)];
        l.fields[1].binding = Some(Binding::Scene {
            content_key: EVENTS,
            serializer: 1,
        });
        SceneLogic {
            scene_content_key: EVENTS,
            layout: l,
        }
    };
    Event {
        ai: Default::default(),
        invitations: nfs_content::events::Invitations {
            scene_content_key: EVENTS,
            serializer: 3,
            count_above: 1,
        },
        restart: None,
        scene: "race".into(),
        scene_content_key: RACE,
        game_mode_id: "gameplay/gamemodes/sprint_race_007".into(),
        launcher: 1,
        start_volume: 2,
        rewards: Rewards {
            cash: 2000,
            rep: 9000,
        },
        award_values: BTreeMap::new(),
        placement_rewards: Some([
            Rewards {
                cash: 2000,
                rep: 9000,
            },
            Rewards {
                cash: 1000,
                rep: 7000,
            },
            Rewards {
                cash: 750,
                rep: 5000,
            },
            Rewards {
                cash: 500,
                rep: 1500,
            },
        ]),
        game_mode: GameMode {
            layout: gm,
            root_bindings: [(GAMEPLAY, 1), (GAMEPLAY, 2), (GAMEPLAY, 3)],
            checkpoints: vec![(RACE, 6), (RACE, 7)],
            points: vec![[10., 0., 0.], [20., 0., 0.]],
            checkpoint_points: vec![[10., 0., 0.], [20., 0., 0.]],
            drift: Some(false),
            pacing: None,
            standings_interval: 0.3,
            laps: 1,
        },
        start_logics,
        game_mode_logics,
        sequences,
        scene_logics: vec![
            scene_logic(request::IN_GAME_MODE, 1),
            scene_logic(request::COOLDOWN, 0),
        ],
        linked_logics: vec![linked],
        area_notifications: vec![3, 4],
        scene_notifications: vec![5],
        game_mode_notifications: FLAGS.to_vec(),
        dnf_timeout: 80.,
    }
}

/// Constructed one-award graph, with an owned snapshot feeding its total,
/// synchronized presentation and Score node. No installed or captured data.
pub(crate) fn awards() -> nfs_content::events::awards::Content {
    use nfs_world::scoring::events::{Connection, Delivery, Endpoint};
    use nfs_world::scoring::{
        nodes,
        program::{Behavior, Identity, Keeper, Node, Program, Step},
        properties::{Definition, Kind as Type, Port},
        score, synced,
        value::Value,
    };
    let node = |i, behavior| Node {
        identity: Identity {
            asset: "test".into(),
            index: i,
            instances: vec![],
        },
        class: "Test".into(),
        created: true,
        events: true,
        properties: true,
        forward: None,
        defaults: vec![],
        behavior,
    };
    let port = |node, field| Port { node, field };
    let program = Program {
        nodes: vec![
            node(0, Behavior::Inert),
            node(
                1,
                Behavior::Snapshot(vec![(nodes::OUT, Value::Integer(1000))]),
            ),
            node(2, Behavior::Synced(synced::Kind::Integer)),
            node(3, Behavior::Inert),
            node(
                4,
                Behavior::ScoreContext {
                    participant_provider: 0,
                    keepers: vec![Keeper { node: 6, field: 7 }],
                },
            ),
            node(
                5,
                Behavior::Score {
                    default: 0,
                    context: 4,
                },
            ),
            node(6, Behavior::Inert),
        ],
        cells: vec![
            Definition {
                kind: Type::Integer,
                initial: None,
                ports: vec![
                    port(1, nodes::OUT),
                    port(2, synced::IN),
                    port(3, nodes::FLOAT_VALUE),
                    port(5, score::SCORE),
                ],
                listeners: vec![],
            },
            Definition {
                kind: Type::Integer,
                initial: None,
                ports: vec![port(2, synced::OUT)],
                listeners: vec![],
            },
        ],
        connections: vec![Connection {
            source: Endpoint {
                node: 0,
                event: 2_659_764_833,
            },
            target: Endpoint {
                node: 5,
                event: score::AWARD,
            },
            delivery: Delivery::Local,
        }],
        startup: vec![
            Step::Bind(1),
            Step::Bind(2),
            Step::Activate(2),
            Step::Activate(4),
        ],
        unmodeled: vec![],
    };
    nfs_content::events::awards::Content {
        program: std::sync::Arc::new(program),
        root: 0,
        total: 3,
        keeper: 7,
        reads: vec![],
        rankings: vec![],
        fields: vec![
            nfs_content::scoring::content::Field {
                node: 0,
                serializer: 0,
                kind: Kind::Root,
            },
            nfs_content::scoring::content::Field {
                node: 6,
                serializer: 1,
                kind: Kind::RpcReference,
            },
            nfs_content::scoring::content::Field {
                node: 2,
                serializer: 2,
                kind: Kind::I32Property,
            },
        ],
    }
}

/// Three scenes of RPC serializers after a no-op, and one owned participant.
pub(crate) fn add_scenes(players: &mut Players, first: u16) -> Vec<Record> {
    let kinds = |n| {
        let mut k = vec![SceneKind::Rpc; n];
        k[0] = SceneKind::Noop;
        k
    };
    let profiles = SceneContent::new(vec![
        (RACE, Profile::new(false, &kinds(13)).unwrap()),
        (GAMEPLAY, Profile::new(false, &kinds(6)).unwrap()),
        (
            EVENTS,
            Profile::new(false, &[kinds(3), vec![SceneKind::BoolProperty]].concat()).unwrap(),
        ),
    ])
    .unwrap();
    let creations = profiles
        .profiles()
        .enumerate()
        .map(|(i, (key, p))| {
            ordinary::creation(
                first + i as u16,
                key,
                None,
                Serial::new(0).unwrap(),
                &ordinary::unpopulated(p).unwrap(),
                &profiles,
            )
            .unwrap()
        })
        .collect();
    players
        .create_scenes(&[first, first + 1, first + 2], creations, &profiles)
        .unwrap()
}
pub(crate) fn world() -> (Players, Vec<Record>, u16) {
    let mut players = Players::default();
    let scenes = add_scenes(&mut players, 1);
    let player = players
        .create(
            HOST_SELECTOR as u8,
            PERSONA,
            Request {
                name: b"Driver".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    let participant = players
        .join(HOST_SELECTOR as u8, PERSONA, player)
        .unwrap()
        .unwrap()
        .id;
    (players, scenes, participant)
}

#[test]
fn invitation_value_is_defined_for_solo_changes_with_humans_and_is_world_local() {
    let content = content();
    let (mut first, _, _) = world();
    let (mut second, _, _) = world();
    let scene = first.objects().scene(EVENTS).unwrap();
    let state = |players: &Players| {
        let RecordInitial::SubLevel { fields, .. } = &players.objects().get(scene).unwrap().initial
        else {
            panic!("scene")
        };
        fields[3].clone()
    };
    assert_eq!(state(&first), sublevel::Initial::BoolProperty(None));
    let record = content.invitation_update(&mut first, 1).unwrap().unwrap();
    assert_eq!(record.id, scene);
    assert_eq!(state(&first), sublevel::Initial::BoolProperty(Some(false)));
    assert!(content.invitation_update(&mut first, 1).unwrap().is_none());
    assert_eq!(state(&second), sublevel::Initial::BoolProperty(None));
    content.invitation_update(&mut second, 2).unwrap().unwrap();
    assert_eq!(state(&second), sublevel::Initial::BoolProperty(Some(true)));
    assert_eq!(state(&first), sublevel::Initial::BoolProperty(Some(false)));
    content.invitation_update(&mut second, 1).unwrap().unwrap();
    assert_eq!(state(&second), sublevel::Initial::BoolProperty(Some(false)));
}
pub(crate) fn content() -> Content {
    Content::new(event(), "SpeedWall_SprintRace".into()).unwrap()
}

pub(crate) fn ai_content(ai: crate::event_ai::Content) -> Content {
    use nfs_world::{
        race_pacing::{AiOnlyMode, Config, Schedule},
        road_distance::*,
    };
    let mut event = event();
    event.ai = ai.roster().clone();
    event.game_mode.pacing = Some(
        Config::new(
            (1..=11)
                .map(|n| {
                    vec![
                        Schedule {
                            target: None,
                            skill: None
                        };
                        n
                    ]
                })
                .collect(),
            0.,
            1.,
            AiOnlyMode::Average,
        )
        .unwrap(),
    );
    event
        .sequences
        .iter_mut()
        .find(|s| s.layout.blueprint == request::OUTRO_SEQUENCE)
        .unwrap()
        .wander = Some(nfs_content::events::Wander {
        urgent: false,
        prefer_straight: true,
        speed_limit: 25.,
        timeout: 30.,
    });
    let mut grid = vec![vec![vec![]; GRID_SIZE]; GRID_SIZE];
    for cell in grid.iter_mut().take(102).skip(99) {
        for rows in cell.iter_mut().take(102).skip(99) {
            *rows = vec![0];
        }
    }
    let network = Network::new(
        2,
        vec![Lane {
            start: 0,
            end: 1,
            segments: vec![0],
            layer: 1,
        }],
        vec![Spline {
            controls: [[0.; 3], [0.; 3], [200., 0., 0.], [200., 0., 0.]],
            coefficients: [[0.; 3], [200., 0., 0.], [0.; 3], [0.; 3]],
            points: vec![[0.; 3], [200., 0., 0.]],
            end_times: vec![1.],
            end_distances: vec![200.],
        }],
        grid,
    )
    .unwrap();
    let mut content = Content::new(event, "SpeedWall_SprintRace".into())
        .unwrap()
        .with_navigation(std::sync::Arc::new(network))
        .unwrap();
    content.ai = Some(std::sync::Arc::new(ai));
    content
}
fn initial(creation: &Creation) -> &[Initial] {
    creation.body.initial.as_deref().unwrap()
}

#[test]
fn placement_payouts_use_finish_order_and_the_original_field_size() {
    let c = content();
    for (position, participants, cash, rep) in [
        (1, 6, 2000, 9000),
        (2, 6, 1000, 7000),
        (3, 6, 750, 5000),
        (4, 6, 500, 1500),
        (6, 6, 500, 1500),
        (12, 12, 500, 1500),
        (2, 2, 500, 1500),
        (3, 3, 500, 1500),
        (1, 1, 500, 1500),
        (2, 3, 1000, 7000),
    ] {
        let finish = Finish {
            race_ms: 50_000,
            position,
            participants,
        };
        assert_eq!(c.rewards_for_finish(finish).unwrap(), Rewards { cash, rep });
    }
    for (position, participants) in [(0, 6), (-1, 6), (7, 6), (1, 0), (1, 129)] {
        assert!(
            c.rewards_for_finish(Finish {
                race_ms: 50_000,
                position,
                participants,
            })
            .is_err()
        );
    }
    let mut bad = event();
    bad.placement_rewards.as_mut().unwrap()[1].cash = -1;
    assert!(Content::new(bad, "category".into()).is_err());
    let mut bad = event();
    bad.placement_rewards.as_mut().unwrap()[0].rep += 1;
    assert!(Content::new(bad, "category".into()).is_err());
}

#[test]
fn content_names_every_state_and_refuses_missing_or_repeated_ones() {
    let content = content();
    assert_eq!(content.record_id(), 1_761_118_161);
    assert_eq!(content.state(State::Root), 0);
    assert_eq!(content.state(State::Main), 2);
    assert_eq!(
        content.spawner(request::INTRO_SEQUENCE).unwrap(),
        INTRO_SPAWNER
    );
    let mut missing = event();
    missing.game_mode.layout.fields[3].class = "Other".into();
    assert!(Content::new(missing, "x".into()).is_err());
    let mut repeated = event();
    let copy = repeated.game_mode.layout.fields[4].clone();
    repeated.game_mode.layout.fields[5] = copy;
    assert!(Content::new(repeated, "x".into()).is_err());
    assert!(Content::new(event(), String::new()).is_err());
    let mut other_catalog = event();
    other_catalog.start_logics[0].catalog = vec![(10, 41)];
    assert!(Content::new(other_catalog, "x".into()).is_err());
}

#[test]
fn game_mode_binds_the_gameplay_endpoints_route_and_starting_participant() {
    let content = content();
    let (players, _, participant) = world();
    let world = players.objects();
    let creation = content.game_mode(world, participant, 4501.25).unwrap();
    assert_eq!(creation.prefix.blueprint, world.scene(GAMEPLAY).unwrap());
    assert_eq!(creation.prefix.sub_id, 2);
    let fields = initial(&creation);
    let Initial::GameModeRoot(root) = &fields[0] else {
        panic!("root")
    };
    assert_eq!(root.value, 12);
    assert_eq!(f64::from_bits(root.start), 4501.25);
    assert_eq!(root.age, 0);
    assert_eq!(root.participants, vec![participant]);
    for (slot, serializer) in [1, 2, 3].into_iter().enumerate() {
        assert_eq!(
            root.bindings[slot],
            world.scene_endpoint(GAMEPLAY, serializer).unwrap()
        );
    }
    // Consecutive selectors over the RPC fields; the intro spawner holds the
    // participant, the outro spawner and every state start empty.
    for (i, f) in fields.iter().enumerate() {
        assert_eq!(f.rpc().unwrap().selector as usize, i);
    }
    assert!(
        matches!(&fields[INTRO_SPAWNER], Initial::RpcReferences { references, .. } if references == &vec![participant])
    );
    assert!(
        matches!(&fields[OUTRO_SPAWNER], Initial::RpcReferences { references, .. } if references.is_empty())
    );
    let route: Vec<(u16, u16)> = [6, 7]
        .into_iter()
        .map(|s| world.scene_endpoint(RACE, s).unwrap())
        .collect();
    assert!(fields.iter().any(
        |f| matches!(f, Initial::WaypointList { laps: 1, waypoints, .. } if waypoints == &route)
    ));
    assert!(
        fields
            .iter()
            .any(|f| matches!(f, Initial::RpcGuid { guid: None, .. }))
    );
    assert_eq!(
        creation.body.updates.get(
            fields
                .iter()
                .position(|f| matches!(f, Initial::RpcSet64 { .. }))
                .unwrap()
        ),
        Some(&Some(Update::Set64(Some(sublevel::Set64 {
            entries: None
        }))))
    );
    assert!(content.game_mode(world, participant, f64::NAN).is_err());
}

#[test]
fn logics_follow_bindings_and_present_owned_values_only() {
    let content = content();
    let (mut players, _, participant) = world();
    let gm = content
        .game_mode(players.objects(), participant, 10.)
        .unwrap();
    let gm = players
        .spawn_entities(vec![gm], content.creation_content())
        .unwrap()
        .remove(0)
        .id;
    let world = players.objects();
    // Start-area logics are owned by the event scene; the start set counts one.
    let show = content
        .area_logic(
            content
                .logic(Owner::Scene, request::SHOW_PARTICIPANTS, 0)
                .unwrap(),
            world,
            participant,
        )
        .unwrap();
    assert_eq!(show.prefix.blueprint, world.scene(RACE).unwrap());
    assert_eq!(show.prefix.sub_id, 12);
    let fields = initial(&show);
    assert!(
        matches!(fields[0], Initial::Root { value: 1, reference, .. } if reference == participant)
    );
    assert!(
        matches!(fields[1], Initial::RpcReference { reference, target_selector, .. }
        if (reference, target_selector) == world.scene_endpoint(RACE, 9).unwrap())
    );
    assert_eq!(fields[2], Initial::I32Property(Some(1)));
    let start = content
        .area_logic(
            content.logic(Owner::Scene, request::START_AREA, 0).unwrap(),
            world,
            participant,
        )
        .unwrap();
    assert_eq!(initial(&start)[2], Initial::BoolProperty(None));
    // A game-mode channel names the game mode's own serializer.
    let sound = content
        .game_mode_logic(
            content.logic(Owner::GameMode, request::SOUND, 0).unwrap(),
            world,
            participant,
            gm,
            None,
        )
        .unwrap();
    assert_eq!(sound.prefix.blueprint, gm);
    assert!(
        matches!(initial(&sound)[1], Initial::RpcReference { reference, target_selector: 1, .. } if reference == gm)
    );
    // The driving-style logic's third Boolean RPC starts true.
    let driving = content
        .game_mode_logic(
            content
                .logic(Owner::GameMode, request::DRIVING_STYLE, 0)
                .unwrap(),
            world,
            participant,
            gm,
            None,
        )
        .unwrap();
    let bools: Vec<bool> = initial(&driving)
        .iter()
        .filter_map(|f| match f {
            Initial::RpcBool { value, .. } => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(bools, vec![false, false, true]);
    assert_eq!(initial(&driving)[4], Initial::FloatProperty(None));
    // The results logic presents the finish and the rewards; an unlisted value
    // stays absent; without a finish every value is absent.
    let outro_layout = content.logic(Owner::GameMode, request::OUTRO, 0).unwrap();
    let finish = Finish {
        race_ms: 148_129,
        position: 1,
        participants: 6,
    };
    let none = BTreeMap::new();
    let outro = content
        .game_mode_logic(
            outro_layout,
            world,
            participant,
            gm,
            Some(Results {
                finish,
                properties: &none,
                awards: None,
            }),
        )
        .unwrap();
    assert_eq!(
        initial(&outro)[2..].to_vec(),
        vec![
            Initial::I32Property(Some(0)),
            Initial::I32Property(Some(9000)),
            Initial::FloatProperty(Some(0f32.to_bits())),
            Initial::I32Property(Some(148_129)),
            Initial::I32Property(Some(1)),
            Initial::I32Property(Some(2000)),
            Initial::I32Property(Some(1)),
            Initial::I32Property(None),
        ]
    );
    // A written category total is presented and joins the total REP; other
    // properties do not.
    let written = BTreeMap::from([
        (category_property(), 1284),
        (
            Property {
                channel: "gameplay/eventawardscaptureddatachannel".into(),
                port: 3_919_538_161,
            },
            77,
        ),
    ]);
    let scored = content
        .game_mode_logic(
            outro_layout,
            world,
            participant,
            gm,
            Some(Results {
                finish,
                properties: &written,
                awards: None,
            }),
        )
        .unwrap();
    assert_eq!(
        initial(&scored)[2..4].to_vec(),
        vec![
            Initial::I32Property(Some(1284)),
            Initial::I32Property(Some(10_284)),
        ]
    );
    // Placement affects both the cash and total REP, without altering the
    // independently earned driving categories or the displayed finish position.
    for (position, participants, cash, total) in [
        (2, 6, 1000, 8284),
        (3, 6, 750, 6284),
        (4, 6, 500, 2784),
        (2, 2, 500, 2784),
        (3, 3, 500, 2784),
    ] {
        let created = content
            .game_mode_logic(
                outro_layout,
                world,
                participant,
                gm,
                Some(Results {
                    finish: Finish {
                        position,
                        participants,
                        ..finish
                    },
                    properties: &written,
                    awards: None,
                }),
            )
            .unwrap();
        assert_eq!(initial(&created)[2], Initial::I32Property(Some(1284)));
        assert_eq!(initial(&created)[3], Initial::I32Property(Some(total)));
        assert_eq!(initial(&created)[6], Initial::I32Property(Some(position)));
        assert_eq!(initial(&created)[7], Initial::I32Property(Some(cash)));
    }
    let empty = content
        .game_mode_logic(outro_layout, world, participant, gm, None)
        .unwrap();
    assert!(
        initial(&empty)[2..]
            .iter()
            .all(|f| matches!(f, Initial::I32Property(None) | Initial::FloatProperty(None)))
    );
    // Every update is present and carries no value.
    assert!(outro.body.updates.iter().all(|u| matches!(
        u,
        Some(Update::Noop | Update::I32Property(None) | Update::FloatProperty(None))
    )));
    // The second add-set occurrence is a separate layout of the same blueprint.
    assert_eq!(
        content
            .logic(Owner::GameMode, request::ADD_SET, 1)
            .unwrap()
            .occurrence,
        1
    );
    assert!(content.logic(Owner::GameMode, request::ADD_SET, 2).is_err());
    // Events-scene logics are owned by their own scene and bundle.
    let cooldown = content
        .scene_logic_creation(request::COOLDOWN, world, participant)
        .unwrap();
    assert_eq!(cooldown.prefix.blueprint, world.scene(EVENTS).unwrap());
    assert_eq!(cooldown.prefix.asset.bundle, 5);
}

#[test]
fn sequences_take_the_manager_ordinal_and_name_the_participant() {
    let content = content();
    let (mut players, _, participant) = world();
    let gm = content
        .game_mode(players.objects(), participant, 10.)
        .unwrap();
    let gm = players
        .spawn_entities(vec![gm], content.creation_content())
        .unwrap()
        .remove(0)
        .id;
    let intro = content
        .sequence_creation(request::INTRO_SEQUENCE, players.objects(), participant, gm)
        .unwrap();
    let manager = players.objects().scene_endpoint(GAMEPLAY, 4).unwrap();
    assert_eq!(intro.prefix.blueprint, manager.0);
    assert!(
        matches!(&initial(&intro)[0], Initial::SequenceRoot { sequence: 0, manager: m, manager_selector: s, participants, stopping: false, .. }
        if (*m, *s) == manager && participants == &vec![participant])
    );
    assert_eq!(
        initial(&intro)[1],
        Initial::RpcOptionalReference {
            rpc: rpc(1).unwrap(),
            reference: Some(participant)
        }
    );
    assert!(
        matches!(initial(&intro)[2], Initial::RpcReference { reference, target_selector: 1, .. } if reference == gm)
    );
    players
        .spawn_entities(vec![intro], content.creation_content())
        .unwrap();
    assert_eq!(live_sequences(players.objects(), manager), 1);
    let countdown = content
        .sequence_creation(
            request::COUNTDOWN_SEQUENCE,
            players.objects(),
            participant,
            gm,
        )
        .unwrap();
    assert!(matches!(
        &initial(&countdown)[0],
        Initial::SequenceRoot { sequence: 1, .. }
    ));
}

#[test]
fn calls_carry_live_serials_selectors_and_references() {
    let content = content();
    let (mut players, _, participant) = world();
    let gm = content
        .game_mode(players.objects(), participant, 10.)
        .unwrap();
    let gm = players
        .spawn_entities(vec![gm], content.creation_content())
        .unwrap()
        .remove(0)
        .id;
    let world = players.objects();
    let call = content
        .state_call(
            world,
            gm,
            State::Waypoints,
            3,
            Some(participant),
            Arguments::Word(762_201_586),
        )
        .unwrap();
    assert_eq!(
        (call.target, call.selector, call.method, call.reference),
        (
            gm,
            content.state(State::Waypoints) as u16,
            3,
            Some(participant)
        )
    );
    assert_eq!(call.serial, rpc(0).unwrap().serial);
    let scene = content.scene_call(world, 6, 0, None).unwrap();
    assert_eq!(
        (scene.target, scene.selector),
        world.scene_endpoint(RACE, 6).unwrap()
    );
    assert!(content.scene_call(world, 0, 0, None).is_err());
    assert!(event_logic_call_refuses(world));
    assert_eq!(
        car_id("Vehicles/player/Car_Subaru_BRZPremium_2014/Car_Subaru_BRZPremium_2014"),
        762_201_586
    );
}
fn event_logic_call_refuses(world: &World) -> bool {
    super::call(world, 8000, 0, 0, None, Arguments::None).is_err()
        && super::call(world, 1, 0, 0, Some(8000), Arguments::None).is_err()
}

#[test]
fn race_goal_uses_authored_destinations_laps_and_drift_and_rejects_invalid_routes() {
    let mut event = event();
    event.game_mode.laps = 2;
    event.game_mode.drift = Some(false);
    let c = Content::new(event.clone(), "race".into()).unwrap();
    let (initial, update) = c.race_goal().unwrap();
    let RecordInitial::GotoWaypointGoal(goal) = &initial else {
        panic!()
    };
    assert_eq!(
        goal.points,
        event
            .game_mode
            .points
            .iter()
            .map(|p| p.map(f32::to_bits))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        (goal.word, goal.flags, goal.progress_updates),
        (2, [false, false], true)
    );
    assert_eq!(goal.radius, 150f32.to_bits());
    assert_eq!(
        update,
        nfs_world::replication::Update::GotoWaypointGoal(goto_waypoint::Update::Progress(Some([
            0, 0
        ])))
    );
    Record {
        id: 100,
        initial: Some(initial),
        update,
    }
    .encode()
    .unwrap();
    for (points, drift) in [
        (vec![], Some(false)),
        (vec![[0.; 3]], Some(false)),
        (vec![[f32::NAN; 3]; 2], Some(false)),
        (event.game_mode.points.clone(), None),
        (event.game_mode.points.clone(), Some(true)),
    ] {
        let mut invalid = event.clone();
        invalid.game_mode.points = points;
        invalid.game_mode.drift = drift;
        assert!(
            Content::new(invalid, "race".into())
                .unwrap()
                .race_goal()
                .is_err()
        );
    }
}

#[test]
fn outro_commands_use_authored_speed_and_preserve_their_distinct_wander_flags() {
    let mut c = content();
    assert!(c.outro_goals().is_err());
    let seq = c
        .event
        .sequences
        .iter_mut()
        .find(|s| s.layout.blueprint == request::OUTRO_SEQUENCE)
        .unwrap();
    seq.wander = Some(nfs_content::events::Wander {
        urgent: false,
        prefer_straight: true,
        speed_limit: 17.25,
        timeout: 29.,
    });
    let goals = c.outro_goals().unwrap();
    assert!(matches!(&goals[0], RecordInitial::WanderGoal(w) if w.flags == [false, true, true]));
    assert!(matches!(&goals[1], RecordInitial::SpeedLimitGoal(s) if s.limit == 17.25f32.to_bits()));
    let seq = c
        .event
        .sequences
        .iter_mut()
        .find(|s| s.layout.blueprint == request::OUTRO_SEQUENCE)
        .unwrap();
    seq.wander.as_mut().unwrap().urgent = true;
    assert!(c.outro_goals().is_err());
}
