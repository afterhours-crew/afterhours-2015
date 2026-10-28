// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::event_logic::tests::{GAMEPLAY, world};
use nfs_content::messages::Message;
use nfs_services::messages::Row;

const REMINDER: u32 = 1_422_063_331;
const PREORDER: u32 = 985_423_157;

fn content() -> Content {
    let message = |name: &str, hash, flat| {
        (
            (name.to_string(), 0),
            Message {
                name: name.into(),
                hash,
                flat,
                persisted: true,
            },
        )
    };
    let catalog = Catalog::new(
        3,
        vec![
            message("reminder", REMINDER, 969),
            message("preorderclaimparts", PREORDER, 205),
        ],
    )
    .unwrap();
    let managers = Managers {
        scene_key: GAMEPLAY,
        normal: 3,
        underground: 4,
    };
    let licensed = vec![
        Content::licensed_from(
            &catalog,
            &managers,
            LicensedMessage {
                accept: 23_126_408,
                objective: 41,
                hash: PREORDER,
                manager: 4,
            },
        )
        .unwrap(),
    ];
    Content {
        catalog,
        managers,
        licensed,
    }
}

fn calls(rpcs: Vec<HostRpc>) -> Vec<(u16, u32, Option<u16>)> {
    rpcs.into_iter()
        .map(|rpc| {
            let HostRpc::GameMode(call) = rpc else {
                panic!("game mode call")
            };
            let flat = match call.arguments {
                Arguments::Message { flat, .. } => Some(flat),
                _ => None,
            };
            (call.selector, call.method, flat)
        })
        .collect()
}

#[test]
fn each_manager_restores_its_rows_before_its_initiation() {
    let content = content();
    let (players, _, _) = world();
    let world = players.objects();
    let (_, normal) = world.scene_endpoint(GAMEPLAY, 3).unwrap();
    let (_, underground) = world.scene_endpoint(GAMEPLAY, 4).unwrap();
    let row = |hash| Row {
        index: 0,
        hash,
        read: false,
    };
    // E912 32.336 s: s106 restores, s106 m1, s107 restore, s107 m1.
    assert_eq!(
        calls(
            content
                .session_start(world, &[row(REMINDER)], &[row(PREORDER)])
                .unwrap()
        ),
        [
            (normal, RESTORED, Some(969)),
            (normal, INITIATED, None),
            (underground, RESTORED, Some(205)),
            (underground, INITIATED, None),
        ]
    );
    // A fresh account (E1026 36.220 s): the two initiations only; a row of
    // a message that is not installed is skipped.
    assert_eq!(
        calls(content.session_start(world, &[row(7)], &[]).unwrap()),
        [(normal, INITIATED, None), (underground, INITIATED, None)]
    );
    // E1026 1247.925 s: the licensed message is announced on its manager.
    assert_eq!(
        calls(vec![
            content
                .added(world, Manager::Underground, PREORDER)
                .unwrap()
        ]),
        [(underground, ADDED, Some(205))]
    );
}

#[test]
fn a_licensed_message_names_an_installed_message_on_a_known_manager() {
    let content = content();
    assert_eq!(
        content.licensed(23_126_408),
        Some(Licensed {
            accept: 23_126_408,
            objective: 41,
            hash: PREORDER,
            manager: Manager::Underground,
        })
    );
    assert_eq!(content.licensed(23_126_409), None);
    let licensed = |hash, manager| {
        Content::licensed_from(
            &content.catalog,
            &content.managers,
            LicensedMessage {
                accept: 1,
                objective: 41,
                hash,
                manager,
            },
        )
    };
    assert_eq!(licensed(REMINDER, 3).unwrap().manager, Manager::Normal);
    assert!(licensed(PREORDER, 5).is_err());
    assert!(licensed(7, 4).is_err());
    assert_eq!(Manager::Underground.table(), "UgStaticMessages");
    assert_eq!(Manager::Normal.table(), "StaticMessage");
}
