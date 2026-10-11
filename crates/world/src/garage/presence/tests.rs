// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::world::rpc::Serial;

fn endpoint() -> Endpoint {
    Endpoint {
        scene: 17,
        selector: 23,
        serial: Serial::new(6).unwrap(),
    }
}

#[test]
fn first_entry_last_leave_repeats_and_world_isolation() {
    let ep = endpoint();
    let mut state = Presence::default();
    let mut other = Presence::default();
    let owns = |id| [3, 4].contains(&id);
    assert!(state.set(ep, 3, false, owns).unwrap().is_none());
    assert_eq!(
        state.set(ep, 3, true, owns).unwrap(),
        Some(Notification {
            endpoint: ep,
            enabled: true
        })
    );
    assert!(state.set(ep, 3, true, owns).unwrap().is_none());
    assert!(state.set(ep, 4, true, owns).unwrap().is_none());
    assert!(other.set(ep, 3, true, owns).unwrap().is_some());
    assert!(state.set(ep, 3, false, owns).unwrap().is_none());
    assert_eq!(
        state.set(ep, 4, false, owns).unwrap(),
        Some(Notification {
            endpoint: ep,
            enabled: false
        })
    );
    assert!(state.set(ep, 4, false, owns).unwrap().is_none());
    assert!(state.set(ep, 4, true, owns).unwrap().is_some());
    assert_eq!(other.occupants.len(), 1);
}

#[test]
fn foreign_participants_stale_endpoints_and_capacity_do_not_mutate() {
    let ep = endpoint();
    let mut state = Presence::default();
    for participant in 1..=128 {
        state.set(ep, participant, true, |_| true).unwrap();
    }
    let before = state.clone();
    assert_eq!(state.set(ep, 129, true, |_| true), Err(Error::Bound));
    assert_eq!(
        state.set(ep, 1, false, |_| false),
        Err(Error::UnknownObject)
    );
    for altered in [
        Endpoint { scene: 19, ..ep },
        Endpoint {
            serial: Serial::new(7).unwrap(),
            ..ep
        },
    ] {
        assert_eq!(
            state.set(altered, 1, false, |_| true),
            Err(Error::UnknownObject)
        );
    }
    assert_eq!(state, before);
    assert!(state.set(ep, 1, true, |_| true).unwrap().is_none());
}
