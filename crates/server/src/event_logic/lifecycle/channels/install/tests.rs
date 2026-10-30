// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use nfs_content::scoring::events::{Composition, Node};
use nfs_world::scoring::{
    program::{Identity as ProgramIdentity, Node as ProgramNode},
    properties::{Definition as Cell, Kind},
};
use std::sync::Arc;

fn fixture(reference: bool) -> (Composition, Program, BTreeMap<Identity, Definition>) {
    let identity = Identity {
        asset: "fixture".into(),
        index: 4,
        instances: vec![("scene".into(), 3)],
    };
    let class = if reference {
        "LinkedSchematicChannelReferenceEntityData"
    } else {
        "LinkedSchematicChannelEntityData"
    };
    let composition = Composition {
        nodes: vec![Node {
            identity: identity.clone(),
            class: class.into(),
            listeners: None,
            forward: None,
        }],
        buses: vec![],
        bus_entities: vec![],
        properties: vec![],
        connections: vec![],
        links: vec![],
    };
    let program = Program {
        nodes: vec![ProgramNode {
            identity: ProgramIdentity {
                asset: "fixture".into(),
                index: 4,
                instances: vec![("scene".into(), 3)],
            },
            class: class.into(),
            created: true,
            events: true,
            properties: true,
            forward: None,
            defaults: vec![],
            behavior: Behavior::Native,
        }],
        cells: [11, 13]
            .into_iter()
            .map(|field| Cell {
                kind: Kind::Integer,
                initial: None,
                ports: vec![Port { node: 0, field }],
                listeners: vec![],
            })
            .collect(),
        connections: vec![],
        startup: vec![],
        unmodeled: vec![],
    };
    let definitions = BTreeMap::from([(
        identity,
        Definition {
            asset: "schema".into(),
            reference,
            relay: false,
            links: vec![],
            properties: vec![11, 12, 11],
            reference_properties: vec![13],
        },
    )]);
    (composition, program, definitions)
}
fn object(instance: u64) -> Object {
    Object { instance, node: 0 }
}
fn location(instance: u64) -> Location {
    Location { instance, node: 0 }
}

#[test]
fn compiled_channel_handles_bind_by_occurrence_and_preserve_definition_order() {
    let (source, sp, sd) = fixture(false);
    let (reference, rp, rd) = fixture(true);
    let mut links = Registry::default();
    for (id, c, d) in [(1, &source, &sd), (2, &reference, &rd), (3, &source, &sd)] {
        links.install(id, Arc::new(c.clone()), d).unwrap();
    }
    links.bind(object(2), object(3)).unwrap();
    let mut router = Router::new(10);
    router
        .install(
            &[
                Graph {
                    instance: 2,
                    program: &rp,
                    channels: &rd,
                },
                Graph {
                    instance: 1,
                    program: &sp,
                    channels: &sd,
                },
                Graph {
                    instance: 3,
                    program: &sp,
                    channels: &sd,
                },
            ],
            &links,
        )
        .unwrap();
    assert_eq!(router.definitions[&location(1)].fields, [11, 11, 13]);
    assert_eq!(router.references[&location(2)].source, location(3));
    assert_eq!(
        router.references[&location(2)].fields,
        BTreeSet::from([11, 13])
    );
    assert!(!router.references[&location(2)].active);
    assert!(router.pending.is_empty());
}

#[test]
fn invalid_later_channel_or_program_occurrence_rolls_back_registration() {
    let (source, sp, sd) = fixture(false);
    let (reference, mut rp, rd) = fixture(true);
    let mut links = Registry::default();
    links.install(1, Arc::new(source), &sd).unwrap();
    links.install(2, Arc::new(reference), &rd).unwrap();
    let mut router = Router::new(10);
    let result = router.install(
        &[
            Graph {
                instance: 1,
                program: &sp,
                channels: &sd,
            },
            Graph {
                instance: 2,
                program: &rp,
                channels: &rd,
            },
        ],
        &links,
    );
    assert_eq!(result, Err(Error::MissingInput));
    assert!(router.definitions.is_empty());
    links.bind(object(2), object(1)).unwrap();
    rp.nodes[0].identity.instances[0].1 = 4;
    assert_eq!(
        router.install(
            &[
                Graph {
                    instance: 1,
                    program: &sp,
                    channels: &sd
                },
                Graph {
                    instance: 2,
                    program: &rp,
                    channels: &rd
                },
            ],
            &links
        ),
        Err(Error::InvalidGraph)
    );
    assert!(router.definitions.is_empty());
}
