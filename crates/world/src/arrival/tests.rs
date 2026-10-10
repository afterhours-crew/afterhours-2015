// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    logic::EntityRef,
    replication::{
        Initial,
        entity::{
            self,
            creation::{Asset, Catalog},
        },
        players::Request,
        sublevel,
    },
    sequences::Component,
};

fn setup() -> (Players, Arrivals) {
    let mut players = Players::default();
    let scenes = sublevel::Content::new(vec![
        (
            7,
            // Field 3 is the scene's third RPC: wire selector 2, as the
            // official Gameplay manager.
            sublevel::Profile::new(
                false,
                &[
                    sublevel::Kind::Noop,
                    sublevel::Kind::Rpc,
                    sublevel::Kind::Rpc,
                    sublevel::Kind::Rpc,
                ],
            )
            .unwrap(),
        ),
        (
            8,
            sublevel::Profile::new(false, &[sublevel::Kind::Noop, sublevel::Kind::Rpc]).unwrap(),
        ),
    ])
    .unwrap();
    let creations = [7, 8].map(|key| {
        sublevel::passive::creation((key - 6) as u16, key, Serial::new(9).unwrap(), &scenes)
            .unwrap()
    });
    players
        .create_scenes(&[1, 2], creations.into(), &scenes)
        .unwrap();
    let asset = Asset {
        bundle: 16,
        type_id: 3424,
        local_index: 9,
    };
    let content = Content::new(
        vec![Catalog::new(16, &[(3404, 1), (3424, 11)]).unwrap()],
        vec![(asset, profile())],
    )
    .unwrap();
    let definition = Definition {
        asset,
        manager: Component {
            scene_key: 7,
            index: 3,
        },
        embedded_bus: 2,
        channel: Component {
            scene_key: 8,
            index: 1,
        },
    };
    (players, Arrivals::new(definition, content).unwrap())
}

fn join(players: &mut Players) -> Owner {
    let player = players
        .create(
            4,
            400,
            Request {
                name: b"Local".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap()
        .id;
    Owner {
        connection: 4,
        persona: 400,
        participant: players.join(4, 400, player).unwrap().unwrap().id,
    }
}

fn reached(event: u32, ghost: u16, entity: u32) -> Message {
    Message::Reached {
        event,
        target: EntityRef { ghost, entity },
        player: 0,
    }
}

#[test]
fn an_arrive_sequence_starts_once_stops_on_the_event_and_completes_once() {
    let (mut players, mut arrivals) = setup();
    let owner = join(&mut players);
    let record = arrivals.start(&mut players, owner).unwrap().unwrap();
    assert_eq!(arrivals.start(&mut players, owner).unwrap(), None);
    let Some(Initial::Entity { prefix, fields }) = &record.initial else {
        panic!()
    };
    // Official layout: manager selector 2, sequence 1, the participant, bus 2.
    assert_eq!((prefix.sub_id, prefix.asset.local_index), (2, 9));
    let entity::Initial::SequenceRoot {
        manager_selector,
        sequence,
        participants,
        ..
    } = &fields[0]
    else {
        panic!()
    };
    assert_eq!(
        (*manager_selector, *sequence, participants.as_slice()),
        (2, 1, [owner.participant].as_slice())
    );
    let ghost = record.id;
    assert_eq!(arrivals.ghost(owner.participant), Some(ghost));
    assert!(arrivals.acknowledges(ghost, 2, 0));
    assert!(arrivals.acknowledges(ghost, 3, 0));
    assert!(!arrivals.acknowledges(ghost, 2, 1) && !arrivals.acknowledges(ghost + 1, 2, 0));
    assert!(arrivals.has_completion(ghost, 0) && !arrivals.has_completion(ghost, 1));
    let completed = Completed {
        sequence: ghost,
        participant: owner.participant,
    };
    // Completing before the stop is a protocol error.
    assert_eq!(
        arrivals.complete(&players, owner, completed),
        Err(Error::Shape)
    );
    for other in [
        reached(ARRIVED + 1, ghost, 1),
        reached(ARRIVED, ghost, 2),
        reached(ARRIVED, ghost + 1, 1),
    ] {
        assert_eq!(arrivals.arrived(&other).unwrap(), None);
    }
    let stop = arrivals
        .arrived(&reached(ARRIVED, ghost, 1))
        .unwrap()
        .unwrap();
    assert_eq!(stop.sequence, ghost);
    assert_eq!(arrivals.arrived(&reached(ARRIVED, ghost, 1)).unwrap(), None);
    let foreign = Owner {
        persona: 401,
        ..owner
    };
    assert!(arrivals.complete(&players, foreign, completed).is_err());
    assert_eq!(
        arrivals.complete(&players, owner, completed),
        Ok(Some(ghost))
    );
    assert_eq!(arrivals.complete(&players, owner, completed), Ok(None));
}

#[test]
fn a_silent_client_is_stopped_after_the_fallback() {
    let (mut players, mut arrivals) = setup();
    let owner = join(&mut players);
    arrivals.poll(1_000).unwrap();
    let ghost = arrivals.start(&mut players, owner).unwrap().unwrap().id;
    assert!(arrivals.poll(1_000 + FALLBACK_MS - 1).unwrap().is_empty());
    let stops = arrivals.poll(1_000 + FALLBACK_MS).unwrap();
    assert_eq!(
        stops.iter().map(|s| s.sequence).collect::<Vec<_>>(),
        vec![ghost]
    );
    assert!(arrivals.poll(1_000 + 2 * FALLBACK_MS).unwrap().is_empty());
    assert_eq!(arrivals.poll(0), Err(Error::Shape));
}

#[test]
fn foreign_participants_and_wrong_profiles_are_refused() {
    let (mut players, mut arrivals) = setup();
    let owner = join(&mut players);
    let stranger = Owner {
        persona: 999,
        ..owner
    };
    assert_eq!(
        arrivals.start(&mut players, stranger),
        Err(Error::UnknownObject)
    );
    let asset = Asset {
        bundle: 16,
        type_id: 3424,
        local_index: 9,
    };
    let wrong = Content::new(
        vec![Catalog::new(16, &[(3404, 1), (3424, 11)]).unwrap()],
        vec![(
            asset,
            entity::Profile::new(&[entity::Kind::SequenceRoot]).unwrap(),
        )],
    )
    .unwrap();
    let definition = Definition {
        asset,
        manager: Component {
            scene_key: 7,
            index: 3,
        },
        embedded_bus: 2,
        channel: Component {
            scene_key: 8,
            index: 1,
        },
    };
    assert!(Arrivals::new(definition, wrong).is_err());
    assert!(
        Arrivals::new(
            Definition {
                embedded_bus: 0,
                ..definition
            },
            setup().1.content.clone()
        )
        .is_err()
    );
}
