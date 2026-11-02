// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

/// Requesting endpoint of a fresh transport connection. Owns only transport
/// admission, not application identity or any game readiness decision.
pub struct ClientHandshake {
    codec: Codec,
    host: u32,
    index: u16,
    token: u32,
    request: Vec<u8>,
    tx: u16,
    completed: Option<(Vec<u8>, Vec<u8>, Cursors)>,
}

impl ClientHandshake {
    /// IDs must be distinct and nonzero; the caller supplies its tunnel index
    /// and fresh connection token. Retries use the same encoded request.
    pub fn new(codec: Codec, client: u32, host: u32, index: u16, token: u32) -> Option<Self> {
        if client == 0 || host == 0 || client == host || index > transport::MAX_CURSOR {
            return None;
        }
        let request = codec
            .encode(
                Header::Initial {
                    sender: client,
                    index,
                    peer_known: false,
                    cursor: 0,
                },
                &[Part {
                    channel: 0,
                    bytes: Control::Request {
                        token,
                        sender: client,
                    }
                    .encode(),
                }],
            )
            .ok()?;
        let tx = transport::advance(0, request.len() - 11).ok()?;
        Some(Self {
            codec,
            host,
            index,
            token,
            request,
            tx,
            completed: None,
        })
    }

    pub fn request(&self) -> &[u8] {
        &self.request
    }

    pub fn is_established(&self) -> bool {
        self.completed.is_some()
    }

    /// Validate the host reply and return the confirmation to send. An exact
    /// duplicate reply receives the identical confirmation without cursor drift.
    /// Any different input after completion is left for the connection owner.
    pub fn receive(&mut self, raw: &[u8]) -> Step {
        if let Some((reply, confirmation, _)) = &self.completed {
            return if raw == reply {
                Step::Send(confirmation.clone())
            } else {
                Step::Ignored(Ignored::Unexpected)
            };
        }
        let decoded = match self.codec.decode(raw) {
            Ok(decoded) => decoded,
            Err(error) => return Step::Ignored(Ignored::Transport(error)),
        };
        let (
            Header::Initial {
                sender,
                index,
                peer_known: true,
                cursor,
            },
            [part],
        ) = (decoded.header, decoded.parts.as_slice())
        else {
            return Step::Ignored(Ignored::Unexpected);
        };
        if part.channel != 0 {
            return Step::Ignored(Ignored::Unexpected);
        }
        let Some(Control::Reply { token, sender: id }) = Control::decode(&part.bytes) else {
            return Step::Ignored(Ignored::Unexpected);
        };
        if sender != self.host || token != self.token || id != self.host {
            return Step::Ignored(Ignored::Identity);
        }
        if cursor > MAX_FORWARD_UNITS {
            return Step::Ignored(Ignored::Gap);
        }
        let confirmation = match self.codec.encode(
            Header::Ordinary {
                index,
                cursor: self.tx,
            },
            &[Part {
                channel: 0,
                bytes: Control::Confirm { token: self.token }.encode(),
            }],
        ) {
            Ok(wire) => wire,
            Err(error) => return Step::Ignored(Ignored::Transport(error)),
        };
        let (Ok(rx), Ok(tx)) = (
            decoded.next_cursor(),
            transport::advance(self.tx, confirmation.len() - 4),
        ) else {
            return Step::Ignored(Ignored::Gap);
        };
        // Link fields retain their original host-side names. Reverse the
        // indices for this endpoint; rx/tx always remain local directions.
        let cursors = Cursors {
            rx,
            tx,
            client_index: index,
            host_index: self.index,
            token: self.token,
        };
        self.completed = Some((raw.to_vec(), confirmation.clone(), cursors));
        Step::Send(confirmation)
    }

    /// Call after sending the returned confirmation. The next host packet is
    /// passed to `Link::respond`, with these endpoint-relative cursors.
    pub fn established(self) -> Option<(Codec, Cursors)> {
        Some((self.codec, self.completed?.2))
    }
}
