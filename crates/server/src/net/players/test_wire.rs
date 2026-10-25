// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

std::thread_local! {
    static CLIENT_PADDING: std::cell::Cell<u64> = const { std::cell::Cell::new(0b0100110) };
}

pub(super) fn client_padding() -> u64 {
    CLIENT_PADDING.get()
}

/// A real world session for queue tests: the flush and its admission run on
/// it exactly as in the world loop (synthetic key and MAC template).
pub(super) fn session_host() -> nfs_world::session::Host {
    let key = nfs_world::crypto::session_key(
        "11111111-2222-3333-4444-555555555555",
        "66666666-7777-8888-9999-aaaaaaaaaaaa",
    )
    .unwrap();
    let codec =
        nfs_world::transport::Codec::new(key, std::array::from_fn(|i| (i * 5) as u8)).unwrap();
    nfs_world::session::Host::new(
        codec,
        nfs_world::session::Ids {
            client: 0x0102_0304,
            host: 0x0a0b_0c0d,
            host_index: 0,
        },
        nfs_world::application::Policy::default(),
        Box::new(nfs_world::application::Fixed::new([1, 2, 3])),
    )
    .unwrap()
}

/// Flush the listener's owned output into `host` with the world loop's code.
pub(super) fn flush(
    listener: &mut super::PlayerListener,
    host: &mut nfs_world::session::Host,
) -> Result<(), (&'static str, Option<nfs_world::session::Error>)> {
    crate::net::flush_owned(listener, host, 1).map_err(|refused| (refused.queue, refused.error))
}

/// Everything a scenario left for the world host must be sendable: the host
/// closes the session on a section or call it cannot encode (E1180), on a
/// section or entry the session does not admit once stamped (E1186, E1188),
/// and on a call naming an object the client was never given.
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
    let admitted = listener
        .pending
        .iter()
        .skip(before[0])
        .map(|section| (section, &[][..]))
        .chain(entries().map(|(section, calls)| (section, calls.as_slice())));
    for (section, calls) in admitted {
        assert!(
            super::entry_admitted(section, calls),
            "the session refuses this {:?}-bit entry ({} records, {} calls)",
            nfs_world::session::queued_replication_bits(section, calls),
            section.records.len(),
            calls.len()
        );
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
