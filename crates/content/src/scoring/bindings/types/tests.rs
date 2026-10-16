// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::scoring::bindings::{Cell, Plan};
use nfs_world::scoring::properties::Store;

fn port(node: u16) -> Port {
    Port { node, field: 10 }
}
fn fixture() -> Layout {
    Layout {
        plan: Plan {
            buses: vec![],
            excluded_rows: 0,
        },
        cells: vec![
            Cell { producers: vec![] },
            Cell { producers: vec![] },
            Cell {
                producers: vec![0, 1],
            },
            Cell { producers: vec![] },
        ],
        outputs: [(port(0), 0), (port(1), 1), (port(5), 0), (port(3), 3)].into(),
        inputs: [(port(2), 2), (port(4), 2)].into(),
    }
}
fn constraint(endpoint: Endpoint, kind: Kind) -> Constraint {
    Constraint { endpoint, kind }
}

#[test]
fn aliases_and_merges_share_type_but_not_storage_or_readiness() {
    for kind in [
        Kind::Boolean,
        Kind::Integer,
        Kind::Float,
        Kind::Vec3,
        Kind::Vec4,
        Kind::Transform,
    ] {
        let layout = fixture();
        let mut constraints = vec![constraint(Endpoint::Input(port(4)), kind)];
        assert_eq!(
            layout.property_types(&constraints).unwrap(),
            [Some(kind), Some(kind), Some(kind), None]
        );
        assert!(layout.unready_definitions(&constraints).is_err());
        constraints.push(constraint(Endpoint::Output(port(3)), Kind::Boolean));
        let definitions = layout.unready_definitions(&constraints).unwrap();
        assert_eq!(definitions.len(), 4);
        assert_eq!(definitions[0].ports, [port(0), port(5)]);
        assert_eq!(definitions[2].ports, [port(2), port(4)]);
        assert!(
            definitions
                .iter()
                .all(|d| d.initial.is_none() && d.listeners.is_empty())
        );
        let store = Store::new(6, definitions).unwrap();
        for node in 0..6 {
            assert_eq!(store.read(port(node)).unwrap(), None);
        }
        // Compilation has no effect on the source topology or merge order.
        assert_eq!(layout.cells[2].producers, [0, 1]);
    }
}

#[test]
fn conflicts_propagate_from_either_merge_producer_and_alias() {
    let layout = fixture();
    for other in [
        Endpoint::Output(port(1)),
        Endpoint::Output(port(5)),
        Endpoint::Input(port(2)),
    ] {
        for reverse in [false, true] {
            let mut constraints = [
                constraint(Endpoint::Output(port(0)), Kind::Float),
                constraint(other, Kind::Integer),
            ];
            if reverse {
                constraints.reverse();
            }
            assert!(layout.property_types(&constraints).is_err());
        }
    }
    let c = constraint(Endpoint::Output(port(0)), Kind::Float);
    assert_eq!(
        layout.property_types(&[c, c]).unwrap()[2],
        Some(Kind::Float)
    );
    let mut repeated = layout;
    repeated.cells[2].producers = vec![0, 0];
    assert_eq!(
        repeated.property_types(&[c]).unwrap(),
        [Some(Kind::Float), None, Some(Kind::Float), None]
    );
}

#[test]
fn malformed_and_excessive_public_layouts_fail_without_panics() {
    for change in 0..8 {
        let mut layout = fixture();
        match change {
            0 => {
                layout.inputs.insert(port(4), 999);
            }
            1 => layout.cells[2].producers[0] = 999,
            2 => layout.cells[2].producers[0] = 2,
            3 => layout.cells[2].producers.truncate(1),
            4 => layout.cells[2].producers.push(0),
            5 => layout.cells.push(Cell { producers: vec![] }),
            6 => layout
                .cells
                .resize(MAX_PROPERTIES + 1, Cell { producers: vec![] }),
            7 => layout
                .outputs
                .extend((0..MAX_PORTS as u32).map(|field| (Port { node: 8, field }, 0))),
            _ => unreachable!(),
        }
        assert!(layout.property_types(&[]).is_err(), "{change}");
    }
    let layout = fixture();
    assert!(
        layout
            .property_types(&[constraint(Endpoint::Input(port(0)), Kind::Float)])
            .is_err()
    );
    let c = constraint(Endpoint::Output(port(0)), Kind::Float);
    assert!(layout.property_types(&vec![c; MAX_PORTS + 1]).is_err());
}

#[test]
fn store_namespace_collision_cannot_silently_alias_separate_cells() {
    let mut layout = fixture();
    layout.inputs.insert(port(0), 2);
    let constraints = [
        constraint(Endpoint::Output(port(0)), Kind::Float),
        constraint(Endpoint::Output(port(3)), Kind::Float),
    ];
    assert!(layout.property_types(&constraints).is_ok());
    assert!(layout.unready_definitions(&constraints).is_err());
    layout.inputs.insert(port(0), 0);
    let definitions = layout.unready_definitions(&constraints).unwrap();
    assert_eq!(
        definitions[0]
            .ports
            .iter()
            .filter(|&&p| p == port(0))
            .count(),
        1
    );
}
