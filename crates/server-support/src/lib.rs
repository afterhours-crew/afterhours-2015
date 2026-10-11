// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded loopback discovery and latency services. Run blocking socket work off async workers.
pub mod discovery;
pub mod qos_service;
pub mod qos_tls;
pub use discovery::Failure;
