// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::super::tests::{drain, native_completion, send};
use super::*;
use crate::sequence_content::execution::{AWARD, AWARDED};
use nfs_content::prologue::sequences::{
    Relay, ServiceLink,
    program::{Definition, Service},
};
use nfs_storage::{InventoryRepository, Timestamp};
use nfs_world::replication::entity;
use nfs_world::scoring::{
    events::{Connection, Delivery, Endpoint as EventEndpoint},
    interpreter,
    program::{Behavior, Step},
};

pub(in crate::net::players::prologue) fn program(listener: &PlayerListener) -> Arc<Definition> {
    let mut result = crate::sequence_content::execution::tests::fixture(false);
    let d = Arc::make_mut(&mut result);
    let vehicles = listener.vehicles.as_ref().unwrap();
    let slot = vehicles.layout.slot(0).unwrap();
    let preview = listener
        .prologue
        .as_ref()
        .unwrap()
        .previews
        .as_ref()
        .unwrap();
    let guid = preview.items[0].items[&1].definition;
    let item = Channel::Scene {
        content_key: vehicles.layout.item_scene_key,
        serializer: slot.item_component_index,
    };
    let primary = Channel::Scene {
        content_key: vehicles.layout.scene_key,
        serializer: slot.component_index,
    };
    let link = |class: &str, target, definition| ServiceLink {
        class: class.into(),
        target,
        definition,
        port: 0,
        relay: None,
        identity: nfs_content::scoring::events::Identity {
            asset: "constructed".into(),
            index: 0,
            instances: vec![],
        },
    };
    d.services
        .insert(5, Service::AwardItem { definition: guid });
    d.links.insert(
        5,
        BTreeMap::from([(
            2_612_322_229,
            link("ItemSystemEntityData", Some(item.clone()), None),
        )]),
    );
    d.links.insert(
        7,
        BTreeMap::from([
            (
                3_339_738_264,
                link("ItemEntityData", Some(item.clone()), Some(guid)),
            ),
            (2_356_970_361, link("ItemEntityData", Some(item), None)),
        ]),
    );
    let mut relay = link("ParticipantEventRelayEntityData", None, None);
    relay.relay = Some(Relay {
        objective: 20,
        selected: 3_135_534_132,
        watched: 1_055_657_837,
        primary_slot: primary,
    });
    d.services.insert(8, Service::ParticipantEvent);
    d.links.insert(8, BTreeMap::from([(3_253_352_650, relay)]));
    let edge = |from, event, to, input| Connection {
        source: EventEndpoint { node: from, event },
        target: EventEndpoint {
            node: to,
            event: input,
        },
        delivery: Delivery::Local,
    };
    d.program
        .connections
        .retain(|e| !(e.source.node == 9 && e.target.event == AWARD));
    d.program
        .connections
        .push(edge(5, AWARDED, 8, 3_135_534_132));
    for (i, slot) in preview.garage.slots.iter().enumerate() {
        let id = 10 + i as u16;
        let mut node = d.program.nodes[8].clone();
        node.identity.index = u32::from(id);
        node.behavior = Behavior::Native;
        d.program.nodes.push(node);
        d.program
            .startup
            .extend([Step::Bind(id), Step::Activate(id)]);
        d.program
            .connections
            .push(edge(9, nfs_world::scoring::nodes::OUT, id, 2_968_292_408));
        d.services.insert(id, Service::VehicleSlotLoader);
        d.links.insert(
            id,
            BTreeMap::from([(
                926_267_033,
                link(
                    "VehicleSlotEntityData",
                    Some(Channel::Scene {
                        content_key: preview.garage.scene_key,
                        serializer: slot.component,
                    }),
                    None,
                ),
            )]),
        );
    }
    // Validate the synthetic compiled graph independently of the listener.
    interpreter::Session::new(Arc::new(d.program.clone())).unwrap();
    result
}

#[test]
fn choice_waits_for_commit_then_unloads_previews_and_finishes_once() {
    crate::net::players::test_wire::with_native_padding(|| {
        selected(false);
    });
}

pub(in crate::net::players::prologue) fn selected(handoff: bool) -> (PlayerListener, u16) {
    let (mut listener, participant, items) = super::super::tests::ready_for_selection(true);
    if handoff {
        let story = Arc::make_mut(listener.prologue.as_mut().unwrap());
        story.first_race = Some(crate::prologue::Race {
            destination: nfs_content::prologue::Destination {
                objective: 21,
                opening_blueprint: "test/arrival".into(),
                opening_guid: [0; 16],
                teleport: nfs_content::prologue::Pose {
                    position: [100., 20., 300.],
                    basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                },
                reference: nfs_content::scoring::events::Identity {
                    asset: "test/story".into(),
                    index: 1,
                    instances: vec![],
                },
            },
            arrival: story.opening.clone(),
            mode: story.driving.as_ref().unwrap().0.clone(),
        });
    }
    let ghost = listener.selection.ghost(participant).unwrap();
    let reached = |event, player| {
        nfs_world::logic::Message::Reached {
            event,
            player,
            target: nfs_world::logic::EntityRef { ghost, entity: 1 },
        }
        .encode()
        .unwrap()
    };
    let (repo, account, first) = crate::progression::tests::local();
    let content = listener.progression.clone().unwrap();
    let policy = content.catalog.story_policy(content.settings);
    let nfs_services::story::Prepared::Mutation(batch) = policy
        .complete(
            &first,
            first.generation,
            policy.next_operation(&first).unwrap(),
            10,
            vec![],
        )
        .unwrap()
    else {
        panic!()
    };
    repo.apply(account, &batch, Timestamp(3)).unwrap();
    let saved = repo.snapshot(account).unwrap();
    let tables = crate::progression::tests::tables();
    let before = Arc::new(tables.view(&saved).unwrap());
    let restored = content.catalog.restore(&before, content.settings).unwrap();
    let scene = listener.players.objects().scene(909).unwrap();
    for branch in [
        crate::progression::Branch::Intro,
        crate::progression::Branch::Main,
    ] {
        let creation = content
            .construction
            .build(
                branch,
                restored.branch(branch),
                participant,
                scene,
                |_, _| {
                    Some(crate::progression::entities::SceneEndpoint {
                        ghost: scene,
                        selector: 1,
                    })
                },
            )
            .unwrap();
        listener
            .players
            .spawn_entities(vec![creation], content.construction.content())
            .unwrap();
    }
    listener.persistent = Some(before.clone());
    listener.inventory = Some(Inventory {
        slots: nfs_world::garage::Slots::new([None; 5]).unwrap(),
        items: items.clone(),
    });
    listener.story_pending.clear();
    assert!(send(&mut listener, 20, 82, reached(41, 1).span()));
    assert!(listener.story_pending.is_empty());
    // A full account-work queue must leave the graph's award latch unchanged.
    let filler = Request {
        participant,
        player_index: 0,
        progression: content.clone(),
        action: Action::Complete { objective: 10 },
    };
    listener.story_pending.resize(MAX_PENDING, filler);
    assert!(!send(&mut listener, 21, 82, reached(41, 0).span()));
    listener.story_pending.clear();
    assert!(send(&mut listener, 21, 82, reached(41, 0).span()));
    assert!(listener.pending_rpcs.is_empty());
    let request = listener.take_story().unwrap();
    let Action::Choose {
        node: 5,
        objective: 20,
        definition,
        ..
    } = request.action
    else {
        panic!()
    };
    assert!(send(&mut listener, 22, 82, reached(41, 0).span()));
    assert!(listener.story_pending.is_empty());
    let complete = native_completion(Completed {
        sequence: ghost,
        participant,
    });
    assert!(!send(&mut listener, 23, 47, complete.span()));
    let nfs_services::story::Prepared::Mutation(mut batch) = policy
        .complete(
            &saved,
            saved.generation,
            policy.next_operation(&saved).unwrap(),
            20,
            vec![],
        )
        .unwrap()
    else {
        panic!()
    };
    let item = nfs_storage::ItemId::new(1).unwrap();
    batch.ops.extend([
        nfs_storage::Op::Insert(nfs_storage::ItemRecord {
            id: item,
            definition: nfs_storage::DefinitionGuid(definition),
            owner: None,
            state: nfs_world::items::OWNED,
            buy_price: 0,
            sell_price: 0,
            default_items: vec![],
            sub_items: vec![],
            derived: vec![],
        }),
        nfs_storage::Op::SetGarage(nfs_storage::GarageSlots([
            Some(item),
            None,
            None,
            None,
            None,
        ])),
    ]);
    repo.apply(account, &batch, Timestamp(4)).unwrap();
    let committed = crate::net::items::story::Committed {
        loaded: Arc::new(tables.view(&repo.snapshot(account).unwrap()).unwrap()),
        items: items.clone(),
        notifications: vec![],
        reward: Some(Receipt { definition, item }),
    };
    listener
        .pending
        .resize(MAX_PENDING, super::super::super::returning::empty_section());
    let world = listener.players.objects().snapshot();
    assert_eq!(
        listener.story_committed(&request, committed.clone()),
        Err(replication::Error::Bound)
    );
    assert_eq!(listener.players.objects().snapshot(), world);
    assert_eq!(
        listener.persistent.as_ref().unwrap().generation(),
        before.generation()
    );
    drain(&mut listener);
    listener
        .story_committed(&request, committed.clone())
        .unwrap();
    assert_eq!(
        listener.inventory.as_ref().unwrap().slots.values()[0],
        Some(1)
    );
    assert_eq!(
        listener
            .pending_rpcs
            .iter()
            .filter(|r| matches!(r, HostRpc::Event(f) if f.event == 11))
            .count(),
        1
    );
    let owned = listener.players.objects().snapshot();
    drain(&mut listener);
    listener.story_committed(&request, committed).unwrap();
    assert_eq!(listener.players.objects().snapshot(), owned);
    assert!(listener.pending_rpcs.is_empty());
    assert!(send(&mut listener, 23, 82, reached(43, 0).span()));
    assert_eq!(listener.pending.iter().flat_map(|s| &s.deleted).count(), 3);
    // Each preview slot unloads for the participant (method 2), once.
    let unloads = |listener: &PlayerListener| {
        listener
            .pending_rpcs
            .iter()
            .filter(|r| {
                matches!(r, HostRpc::GameMode(c)
                    if c.method == 2 && c.reference == Some(participant)
                        && c.arguments == nfs_world::game_mode::Arguments::None)
            })
            .count()
    };
    assert_eq!(unloads(&listener), 3);
    drain(&mut listener);
    assert!(send(&mut listener, 24, 82, reached(43, 0).span()));
    assert!(listener.pending.is_empty());
    assert_eq!(unloads(&listener), 0);
    assert!(send(&mut listener, 25, 82, reached(42, 0).span()));
    assert!(
        listener
            .pending_rpcs
            .iter()
            .any(|r| matches!(r, HostRpc::SequenceStop(s) if s.sequence == ghost))
    );
    drain(&mut listener);
    if handoff {
        listener
            .pending
            .resize(MAX_PENDING, super::super::super::returning::empty_section());
        let before = listener.players.objects().snapshot();
        assert!(!send(&mut listener, 26, 47, complete.span()));
        assert_eq!(listener.players.objects().snapshot(), before);
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroCarSelection)
        );
        drain(&mut listener);
    }
    assert!(send(&mut listener, 26, 47, complete.span()));
    assert!(listener.players.objects().get(ghost).is_none());
    if handoff {
        assert_eq!(
            listener.participants.intro_chapter(participant),
            Some(Chapter::FirstRace)
        );
        assert_eq!(
            listener.participants.stage(participant),
            Some(Stage::IntroSpawning)
        );
        assert!(listener.opening.ghost(participant).is_none());
        let asset = listener
            .prologue
            .as_ref()
            .unwrap()
            .loading
            .as_ref()
            .unwrap()
            .asset();
        let (loading, object) = listener.players.objects().iter().find(|(_, object)|
            matches!(&object.initial, replication::Initial::Entity { prefix, .. } if prefix.asset == asset)
        ).unwrap();
        assert!(
            matches!(&object.initial, replication::Initial::Entity { fields, .. }
            if fields[1] == entity::Initial::BoolProperty(Some(false)))
        );
        assert!(
            listener
                .pending
                .iter()
                .chain(listener.pending_entries.iter().map(|(s, _)| s))
                .flat_map(|s| &s.records)
                .any(|r| r.id == loading
                    && matches!(&r.update, replication::Update::Entity { fields, .. }
                    if fields[1] == Some(entity::Update::BoolProperty(Some(false))))),
            "selection exit must publish the garage streaming release"
        );
    }
    drain(&mut listener);
    assert!(send(&mut listener, 27, 47, complete.span()));
    assert!(listener.pending.is_empty());
    (listener, participant)
}
