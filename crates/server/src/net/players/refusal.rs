// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Refusal bookkeeping for owned world frames. A refused frame stays
//! unacknowledged, so the client resends its reliable messages in every later
//! frame and those frames are refused in turn. The log names the refusing site
//! and, for client RPCs, the decoded route, so a live run identifies the first
//! refusal instead of a count (E838: 6,592 refusals, route unproven).
use super::*;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

/// Every refusal is counted; the first few and then one in every `SAMPLE`
/// are logged, because one poisoned reliable message repeats in every frame.
const LOGGED_HEAD: usize = 16;
const SAMPLE: usize = 512;

pub(super) fn logged(count: usize) -> bool {
    count <= LOGGED_HEAD || count.is_multiple_of(SAMPLE)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RouteLabel {
    pub references: Vec<u16>,
    pub selector: u16,
    pub method: u32,
}

/// The route of a client RPC body for diagnostics only; `None` when the body
/// does not decode as one.
pub(super) fn route_label(body: BitSpan<'_>) -> Option<RouteLabel> {
    let envelope = Envelope::decode(
        body,
        Limits {
            max_input_bits: 8192,
            max_references: frame::MAX_RPC_REFERENCES,
            max_payload_bytes: 256,
        },
    )
    .ok()?;
    let route = envelope.route(RouteProfile::ClientSend).ok()?;
    Some(RouteLabel {
        references: envelope.references().to_vec(),
        selector: route.selector(),
        method: route.method_index(),
    })
}

impl PlayerListener {
    /// Refuse the current frame; `line` is the refusing site in `players.rs`.
    #[track_caller]
    pub(super) fn refuse(&mut self, error: replication::Error) -> bool {
        let site = std::panic::Location::caller();
        self.refused += 1;
        self.last_error = Some(error);
        if logged(self.refused) {
            tracing::warn!(
                ?error,
                line = site.line(),
                refused = self.refused,
                "owned world frame refused"
            );
        }
        false
    }
    /// Refuse the current frame at a client RPC, naming its route.
    #[track_caller]
    pub(super) fn refuse_rpc(&mut self, error: replication::Error, body: BitSpan<'_>) -> bool {
        let site = std::panic::Location::caller();
        self.refused += 1;
        self.last_error = Some(error);
        if logged(self.refused) {
            match route_label(body) {
                Some(route) => tracing::warn!(
                    ?error,
                    line = site.line(),
                    refused = self.refused,
                    references = ?route.references,
                    selector = route.selector,
                    method = route.method,
                    "owned world frame refused at client RPC"
                ),
                None => tracing::warn!(
                    ?error,
                    line = site.line(),
                    refused = self.refused,
                    "owned world frame refused at undecodable client RPC"
                ),
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world::bits::BitWriter;

    #[test]
    fn refusals_are_logged_in_full_at_first_and_then_sampled() {
        assert!((1..=LOGGED_HEAD).all(logged));
        assert!(!logged(LOGGED_HEAD + 1));
        assert!(!logged(SAMPLE - 1));
        assert!(logged(SAMPLE));
        assert!(!logged(SAMPLE + 1));
        assert!(logged(2 * SAMPLE));
    }

    #[test]
    fn route_label_names_references_selector_and_method_or_nothing() {
        let mut payload = BitWriter::new();
        payload
            .put(3, 9)
            .put(3, 32)
            .put(1.5f64.to_bits(), 64)
            .put(0, 32)
            .put(0, 7);
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(22, 13)
            .put(179, 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        assert_eq!(
            route_label(body.span()),
            Some(RouteLabel {
                references: vec![22, 179],
                selector: 3,
                method: 3,
            })
        );
        assert_eq!(route_label(body.span().slice(0, 40).unwrap()), None);
        let mut listener = PlayerListener::new(7);
        assert!(!listener.refuse_rpc(replication::Error::UnknownObject, body.span()));
        assert!(!listener.refuse(replication::Error::Bound));
        assert_eq!(
            (listener.refused, listener.last_error),
            (2, Some(replication::Error::Bound))
        );
    }
}
