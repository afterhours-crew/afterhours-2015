// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

std::thread_local! {
    static CLIENT_PADDING: std::cell::Cell<u64> = const { std::cell::Cell::new(0b0100110) };
}

pub(super) fn client_padding() -> u64 {
    CLIENT_PADDING.get()
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
