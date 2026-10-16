// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::world::BitSpan;

/// Only numeric protocol identifiers. No body, player name or serial/token.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Route {
    Rpc {
        references: Vec<u16>,
        scene_content: Option<u32>,
        selector: u16,
        method: u32,
    },
    UndecodableRpc,
    LogicReached {
        ghost: u16,
        entity: u32,
        event: u32,
    },
    Logic {
        index: u8,
    },
    Message {
        index: u8,
    },
    DecoderIndex {
        index: u8,
    },
    ClientGhostRecords,
    Control {
        subtype: u8,
    },
}

impl PlayerListener {
    pub(in crate::net) fn with_connection(mut self, connection: u32) -> Self {
        self.unsupported = crate::diagnostics::Unsupported::new("world", connection);
        self
    }

    pub(in crate::net) fn unsupported_control(&mut self, subtype: u8) {
        self.unsupported.record(Route::Control { subtype });
    }

    pub(super) fn diagnose_decode(&mut self, error: frame::Error) {
        if let frame::Error::Unsupported(index) = error {
            self.unsupported.record(Route::DecoderIndex { index });
        } else if refusal::logged(self.stats.errors.saturating_add(1)) {
            tracing::warn!(
                connection = self.unsupported.connection(),
                ?error,
                "world handler frame rejected"
            );
        }
    }

    pub(super) fn diagnose_unconsumed(
        &mut self,
        parsed: &frame::Frame<'_>,
        decoded: &[frame::Message<'_>],
    ) {
        for message in decoded {
            if let frame::Message::Opaque { index, .. } = message {
                // RPC and reached-event dispatch happens below. Session::Observing
                // consumes level readiness and sublevel reports after our ACK.
                if ![47, 82, frame::EX_LEVEL_READY, frame::EX_SUBLEVEL_REPORT].contains(index) {
                    self.unsupported.record(Route::Message { index: *index });
                }
            }
        }
        if parsed
            .ghost
            .as_ref()
            .is_some_and(|ghost| ghost.prefix.record_count() != 0)
        {
            self.unsupported.record(Route::ClientGhostRecords);
        }
    }

    pub(super) fn diagnose_logic(&mut self, message: &nfs_world::logic::Message) {
        let route = match message {
            nfs_world::logic::Message::Reached { event, target, .. } => Route::LogicReached {
                ghost: target.ghost,
                entity: target.entity,
                event: *event,
            },
            other => Route::Logic {
                index: other.index(),
            },
        };
        self.unsupported.record(route);
    }

    pub(super) fn diagnose_rpc(&mut self, body: BitSpan<'_>, players: &Players) {
        let route = match refusal::route_label(body) {
            Some(label) => {
                let scene_content = label
                    .references
                    .first()
                    .and_then(|id| players.objects().get(*id))
                    .and_then(|object| match &object.initial {
                        replication::Initial::SubLevel { prefix, .. } => Some(prefix.content_key),
                        _ => None,
                    });
                Route::Rpc {
                    references: label.references,
                    scene_content,
                    selector: label.selector,
                    method: label.method,
                }
            }
            None => Route::UndecodableRpc,
        };
        self.unsupported.record(route);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::testing::capture;
    use nfs_world::{
        bits::BitWriter,
        logic::{EntityRef, Message as Logic},
    };

    fn message(listener: &mut PlayerListener, sequence: u8, value: frame::Message<'_>) -> bool {
        let wire = frame::Builder::new()
            .messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(sequence),
                    messages: vec![value],
                }],
            )
            .build()
            .unwrap();
        listener.frame(Queued {
            advertised: None,
            frame: u16::from(sequence),
            data: wire.span(),
        })
    }

    fn rpc(method: u32) -> BitWriter {
        let mut payload = BitWriter::new();
        payload.put(7, 9).put(method.into(), 32).put(0, 7);
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(1, 8)
            .put(22, 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body
    }

    #[test]
    fn actual_rpc_fallthrough_warns_once_per_route_and_deduplicates_delivery() {
        let output = capture(|| {
            let mut listener = PlayerListener::new(7).with_connection(41);
            let call = rpc(300);
            assert!(message(
                &mut listener,
                0,
                frame::Message::Opaque {
                    index: 47,
                    body: call.span()
                }
            ));
            assert!(message(
                &mut listener,
                0,
                frame::Message::Opaque {
                    index: 47,
                    body: call.span()
                }
            ));
            assert!(message(
                &mut listener,
                1,
                frame::Message::Opaque {
                    index: 47,
                    body: call.span()
                }
            ));
            let other = rpc(301);
            assert!(message(
                &mut listener,
                2,
                frame::Message::Opaque {
                    index: 47,
                    body: other.span()
                }
            ));
            assert_eq!(listener.unsupported_rpcs, 3);
            assert_eq!(listener.refused, 0);
            assert!(listener.pending_rpcs.is_empty());
        });
        assert_eq!(output.matches("WARN").count(), 2, "{output}");
        for value in [
            "connection=41",
            "references: [22]",
            "selector: 7",
            "method: 300",
            "method: 301",
            "unsupported_total=3",
            "suppressed_total=1",
        ] {
            assert!(output.contains(value), "missing {value}: {output}");
        }
    }

    #[test]
    fn unhandled_logic_and_opaque_messages_log_identifiers_without_names_or_bodies() {
        let output = capture(|| {
            let mut listener = PlayerListener::new(7).with_connection(42);
            let body = Logic::Reached {
                event: 123,
                target: EntityRef {
                    ghost: 22,
                    entity: 456,
                },
                player: 0,
            }
            .encode()
            .unwrap();
            assert!(message(
                &mut listener,
                0,
                frame::Message::Opaque {
                    index: 82,
                    body: body.span()
                }
            ));
            assert_eq!(listener.unsupported_logic, 1);
            let body = Logic::PlayerConnected {
                reason: 0,
                name: b"do-not-log-player-secret".to_vec(),
            }
            .encode()
            .unwrap();
            assert!(message(
                &mut listener,
                1,
                frame::Message::Opaque {
                    index: 35,
                    body: body.span()
                }
            ));
        });
        for value in [
            "LogicReached",
            "ghost: 22",
            "entity: 456",
            "event: 123",
            "Message { index: 35 }",
        ] {
            assert!(output.contains(value), "{output}");
        }
        assert!(!output.contains("do-not-log-player-secret"), "{output}");
    }

    #[test]
    fn session_owned_reports_state_and_timing_do_not_warn() {
        let output = capture(|| {
            let mut listener = PlayerListener::new(7).with_connection(43);
            assert!(message(&mut listener, 0, frame::Message::State(1)));
            let mut level = BitWriter::new();
            level.put(1, 32);
            assert!(message(
                &mut listener,
                1,
                frame::Message::Opaque {
                    index: frame::EX_LEVEL_READY,
                    body: level.span()
                }
            ));
            let mut report = BitWriter::new();
            report.put(0, 32).put(0, 32);
            assert!(message(
                &mut listener,
                2,
                frame::Message::Opaque {
                    index: frame::EX_SUBLEVEL_REPORT,
                    body: report.span()
                }
            ));
            assert!(message(
                &mut listener,
                3,
                frame::Message::Time {
                    flag: false,
                    tick: 0,
                    time: 0.0
                }
            ));
            assert!(message(&mut listener, 4, frame::Message::Sentinel(-1)));
        });
        assert!(output.is_empty(), "{output}");
    }

    #[test]
    fn unknown_decoder_index_is_visible_and_still_refused() {
        let output = capture(|| {
            let mut listener = PlayerListener::new(7).with_connection(44);
            let empty = BitWriter::new();
            assert!(!message(
                &mut listener,
                0,
                frame::Message::Opaque {
                    index: 106,
                    body: empty.span()
                }
            ));
            assert_eq!((listener.stats.accepted, listener.stats.errors), (0, 1));
        });
        assert!(output.contains("DecoderIndex { index: 106 }"), "{output}");
    }

    #[test]
    fn malformed_input_is_rejected_without_claiming_an_unsupported_operation() {
        let output = capture(|| {
            let mut listener = PlayerListener::new(7).with_connection(45);
            let empty = BitWriter::new();
            assert!(!listener.frame(Queued {
                advertised: None,
                frame: 0,
                data: empty.span()
            }));
        });
        assert!(output.contains("world handler frame rejected"), "{output}");
        assert!(!output.contains("unsupported operation"), "{output}");
    }

    #[test]
    fn unknown_control_uses_the_world_connection_budget() {
        let output = capture(|| {
            let mut listener = PlayerListener::new(7).with_connection(46);
            listener.unsupported_control(63);
            listener.unsupported_control(63);
        });
        assert_eq!(output.matches("WARN").count(), 1, "{output}");
        assert!(output.contains("Control { subtype: 63 }"), "{output}");
        assert!(output.contains("unsupported_total=2"), "{output}");
    }

    #[test]
    fn unconsumed_client_ghost_records_are_visible_but_empty_sections_are_quiet() {
        let output = capture(|| {
            let mut listener = PlayerListener::new(7).with_connection(47);
            for count in [0, 1] {
                let mut wire = BitWriter::new();
                wire.put(1 << frame::GHOST, 6)
                    .put(0, 1)
                    .put(0, 1)
                    .put(count, 13);
                if count != 0 {
                    wire.put(1, 1).put(23, 13);
                }
                wire.align();
                assert!(listener.frame(Queued {
                    advertised: None,
                    frame: 0,
                    data: wire.span()
                }));
            }
        });
        assert_eq!(output.matches("WARN").count(), 1, "{output}");
        assert!(output.contains("ClientGhostRecords"), "{output}");
        assert!(output.contains("unsupported_total=1"), "{output}");
    }
}
