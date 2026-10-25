// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

std::thread_local! {
    static CLIENT_PADDING: std::cell::Cell<u64> = const { std::cell::Cell::new(0b0100110) };
}

pub(super) fn client_padding() -> u64 {
    CLIENT_PADDING.get()
}

/// Everything a scenario left for the world host must be sendable: the host
/// closes the session on a section or call it cannot encode (E1180), and on a
/// call naming an object the client was never given.
/// Queue lengths before a step, so only that step's own output is checked
/// (scenarios fill the queues with placeholders to simulate backpressure).
pub(super) fn queued(listener: &super::PlayerListener) -> [usize; 3] {
    [
        listener.pending.len(),
        listener.pending_rpcs.len(),
        listener.pending_entries.len(),
    ]
}

pub(super) fn assert_wire_ready(listener: &super::PlayerListener, before: [usize; 3]) {
    let entries = || listener.pending_entries.iter().skip(before[2]);
    let sections = listener
        .pending
        .iter()
        .skip(before[0])
        .chain(entries().map(|(section, _)| section));
    let mut deleted_now = std::collections::BTreeSet::new();
    for section in sections {
        if let Err(error) = section.encode() {
            panic!("unsendable section {error:?}: {section:?}");
        }
        deleted_now.extend(section.deleted.iter().copied());
    }
    let calls = listener
        .pending_rpcs
        .iter()
        .skip(before[1])
        .chain(entries().flat_map(|(_, calls)| calls));
    for &call in calls {
        if let Err(error) = call.encode() {
            panic!("unsendable call {error:?}: {call:?}");
        }
        // A call may name an object this step retires: its deletion is
        // queued after the call (departures before the deletion section).
        for object in call.objects() {
            assert!(
                listener.players.objects().get(object).is_some() || deleted_now.contains(&object),
                "call names object {object} the world does not hold: {call:?}"
            );
        }
    }
}

/// Exercise each complete scenario with zero and native nonzero alignment.
/// The setting is local to this test thread; each invocation gets fresh owners.
pub(super) fn with_native_padding(test: impl Fn()) {
    for padding in [0, 0b0100110, 0b1000000, 0b1111111] {
        CLIENT_PADDING.set(padding);
        test();
    }
    CLIENT_PADDING.set(0b0100110);
}
