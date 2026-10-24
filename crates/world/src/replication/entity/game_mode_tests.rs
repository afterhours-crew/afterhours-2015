// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::replication::sublevel::Set64;
use creation::{Asset, Catalog, Content, Creation, Prefix};

fn rpc(selector: u16) -> Rpc {
    Rpc {
        selector,
        serial: Serial::new(1).unwrap(),
    }
}

fn kinds() -> Vec<Kind> {
    vec![
        Kind::GameModeRoot,
        Kind::RpcReference,
        Kind::CountdownState,
        Kind::WaypointsState,
        Kind::DnfTransition,
        Kind::WaypointList,
        Kind::RpcGuid,
        Kind::RpcSet64,
        Kind::RpcReferences,
    ]
}

fn fixture() -> (Profile, Body) {
    let initial = vec![
        Initial::GameModeRoot(Box::new(GameModeRoot {
            value: 12,
            rpc: rpc(0),
            start: 4488.25f64.to_bits(),
            age: 0.05f32.to_bits(),
            bindings: [(161, 5), (161, 4), (161, 0)],
            participants: vec![351],
            others: vec![],
            flag: false,
        })),
        Initial::RpcReference {
            rpc: rpc(1),
            reference: 313,
            target_selector: 115,
        },
        Initial::CountdownState {
            rpc: rpc(2),
            references: vec![],
            start: None,
        },
        Initial::WaypointsState {
            rpc: rpc(3),
            references: vec![],
            words: [0, 0, 0],
        },
        Initial::DnfTransition {
            rpc: rpc(4),
            values: vec![],
        },
        Initial::WaypointList {
            rpc: rpc(5),
            laps: 1,
            waypoints: vec![(313, 106), (313, 78), (313, 73)],
        },
        Initial::RpcGuid {
            rpc: rpc(6),
            guid: None,
        },
        Initial::RpcSet64(rpc(7)),
        Initial::RpcReferences {
            rpc: rpc(8),
            references: vec![351],
        },
    ];
    let mut updates = vec![Some(Update::Noop); initial.len()];
    updates[7] = Some(Update::Set64(Some(Set64 { entries: None })));
    (
        Profile::new(&kinds()).unwrap(),
        Body {
            initial: Some(initial),
            updates,
        },
    )
}

/// Field widths follow the client readers: root 16+19+64+32+3*22+(8+13)+8+1,
/// channel 41, countdown 19+16+1, waypoints 19+16+96+8, DNF 19+8,
/// waypoint list 19+4+8+3*22, GUID 19+1, set 19, reference list 19+16+13;
/// then nine presence bits and the set update (present set, no list: 2 bits).
#[test]
fn game_mode_fields_have_the_client_reader_widths_and_round_trip() {
    let (profile, body) = fixture();
    let wire = body.encode(&profile).unwrap();
    let initial = 227 + 41 + 36 + 139 + 27 + 97 + 20 + 19 + 48;
    assert_eq!(wire.len(), initial + 9 + 2);
    let decoded = Body::decode(wire.span(), &profile, true).unwrap();
    assert_eq!(decoded.body, body);
    assert_eq!(decoded.initial_bits, Some(initial));
    for end in 0..wire.len() {
        assert!(Body::decode(wire.span().slice(0, end).unwrap(), &profile, true).is_err());
    }
}

#[test]
fn optional_times_lists_and_guids_round_trip_with_exact_widths() {
    let (profile, mut body) = fixture();
    let values = body.initial.as_mut().unwrap();
    values[2] = Initial::CountdownState {
        rpc: rpc(2),
        references: vec![351, 1492],
        start: Some(4501.27f64.to_bits()),
    };
    values[3] = Initial::WaypointsState {
        rpc: rpc(3),
        references: vec![351],
        words: [1, u32::MAX, 7],
    };
    values[4] = Initial::DnfTransition {
        rpc: rpc(4),
        values: vec![(351, u64::MAX), (1492, 0), (8191, 4581.3f64.to_bits())],
    };
    values[6] = Initial::RpcGuid {
        rpc: rpc(6),
        guid: Some([0xa5; 16]),
    };
    body.updates[7] = Some(Update::Set64(Some(Set64 {
        entries: Some(vec![1, u64::MAX]),
    })));
    let wire = body.encode(&profile).unwrap();
    let decoded = Body::decode(wire.span(), &profile, true).unwrap();
    assert_eq!(decoded.body, body);
    let (_, plain) = fixture();
    // The set update grows from "present, no list" (2 bits) to a two-value list.
    let grown = 26 + 64 + 13 + 3 * (13 + 64) + 128 + (7 + 2 * 64);
    assert_eq!(wire.len(), plain.encode(&profile).unwrap().len() + grown);
}

#[test]
fn waypoint_records_lists_and_bounds_are_refused() {
    let (profile, body) = fixture();
    let mut wire = body.encode(&profile).unwrap();
    // The waypoints state's record count follows its three words.
    let count = 227 + 41 + 36 + 19 + 16 + 96;
    let mut bits: Vec<bool> = (0..wire.len())
        .map(|i| wire.span().read_u32(i, 1).unwrap() == 1)
        .collect();
    bits[count + 7] = true;
    wire = crate::bits::BitWriter::new();
    for bit in bits {
        wire.put_bool(bit);
    }
    assert_eq!(
        Body::decode(wire.span(), &profile, true),
        Err(Error::Unsupported)
    );
    let (_, mut long) = fixture();
    long.initial.as_mut().unwrap()[5] = Initial::WaypointList {
        rpc: rpc(5),
        laps: 1,
        waypoints: vec![(313, 106); MAX_WAYPOINTS + 1],
    };
    assert_eq!(long.encode(&profile), Err(Error::Bound));
    let (_, mut laps) = fixture();
    laps.initial.as_mut().unwrap()[5] = Initial::WaypointList {
        rpc: rpc(5),
        laps: 16,
        waypoints: vec![],
    };
    assert_eq!(laps.encode(&profile), Err(Error::Shape));
    let (_, mut set) = fixture();
    set.updates[7] = Some(Update::Set64(Some(Set64 {
        entries: Some(vec![0; MAX_SET64 + 1]),
    })));
    assert_eq!(set.encode(&profile), Err(Error::Bound));
    let (_, mut wrong) = fixture();
    wrong.updates[7] = Some(Update::Noop);
    assert_eq!(wrong.encode(&profile), Err(Error::TypeMismatch));
    assert_eq!(
        Profile::new(&[Kind::Root, Kind::GameModeRoot]),
        Err(Error::Unsupported)
    );
}

#[test]
fn state_snapshots_keep_the_last_set_and_check_every_reference() {
    let (profile, body) = fixture();
    let asset = Asset {
        bundle: 55,
        type_id: 3399,
        local_index: 2,
    };
    let content = Content::new(
        vec![Catalog::new(55, &[(3399, 3)]).unwrap()],
        vec![(asset, profile.clone())],
    )
    .unwrap();
    let creation = Creation {
        prefix: Prefix {
            parent: None,
            blueprint: 161,
            sub_id: 2,
            owner: None,
            asset,
        },
        body,
    };
    let known = |id: u16| [161, 313, 351].contains(&id);
    let mut state = state::State::new(creation.clone(), &content, known).unwrap();
    assert_eq!(state.snapshot(), creation);
    let mut updates = vec![None; profile.kinds().len()];
    let nine = Some(Set64 {
        entries: Some(vec![9]),
    });
    updates[7] = Some(Update::Set64(nine.clone()));
    state.apply(updates).unwrap();
    assert_eq!(state.snapshot().body.updates[7], Some(Update::Set64(nine)));
    for missing in [161, 313, 351] {
        assert_eq!(
            state::State::new(creation.clone(), &content, |id| known(id) && id != missing),
            Err(Error::UnknownObject)
        );
    }
}
