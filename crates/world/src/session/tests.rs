// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{
    application::{Event as App, Fixed, HOST_SELECTOR, Retain},
    crypto::session_key,
    transport::{self, Header, Part},
};
use nfs_protocol::world::{
    admission::{Decoded as Admission, Message, type_zero::Client5},
    challenge::{OpaqueAnswer16, Request13},
    timing::Decoded as Timing,
};

const CLIENT: u32 = 0x0102_0304;
const HOST: u32 = 0x0a0b_0c0d;
const CLIENT_INDEX: u16 = 9;
const HOST_INDEX: u16 = 0;
const TOKEN: u32 = 0x5151_0001;

struct AcceptSignals;

fn entry_setup() -> (Host, Client, AcceptSignals, u64, Output) {
    use crate::replication::players::{Players, Request};
    let (mut host, client, listener, now) = established_with_content();
    let mut players = Players::default();
    let mut section = owned_section(0);
    section.records.push(
        players
            .create(
                HOST_SELECTOR as u8,
                123,
                Request {
                    name: b"Current owner".to_vec(),
                    flag: false,
                    slot: 3,
                },
            )
            .unwrap()
            .unwrap(),
    );
    host.queue_replication(section).unwrap();
    let now = now + REPORT_TIMEOUT_MS;
    loading::after_exchange(&mut host, now);
    let now = now + 2 * STATE_WAIT_MS;
    let sent = host.poll(now);
    assert!(matches!(
        sent.events[..],
        [Event::Replication { records: 1, .. }]
    ));
    (host, client, listener, now, sent)
}

fn acknowledge_entry(
    host: &mut Host,
    client: &mut Client,
    listener: &mut AcceptSignals,
    sent: &Output,
    now: u64,
    accepted: bool,
) {
    let last = sent
        .send
        .iter()
        .filter_map(|w| client.open(w))
        .filter_map(|b| {
            nfs_protocol::world::envelope::ZeroTail::new(1600)
                .unwrap()
                .decode(&b)
                .unwrap()
                .sequence()
                .map(|s| s.number().value())
        })
        .next_back()
        .unwrap();
    let mut control = crate::bits::BitWriter::new();
    control.put(1, 1).put(1, 4);
    let body = application::sequenced(
        HOST_SELECTOR,
        3,
        last,
        if accepted { u32::MAX } else { u32::MAX - 1 },
        &control,
    );
    host.receive(&client.app(&body), now, listener);
}

#[test]
fn first_player_entry_needs_linked_client_and_accepted_owned_creation_then_sends_once() {
    let (mut host, mut client, mut listener, now, sent) = entry_setup();
    let Stage::Running { world, .. } = &host.stage else {
        panic!()
    };
    let entry::Entry::Pending { ticket, .. } = world.player_entry else {
        panic!()
    };
    assert!(
        !host
            .poll(now + REPLICATION_INTERVAL_MS)
            .events
            .iter()
            .any(|e| matches!(e, Event::FirstPlayerEntered { .. }))
    );
    let now = now + REPLICATION_INTERVAL_MS;
    acknowledge_entry(&mut host, &mut client, &mut listener, &sent, now + 2, true);
    let t = now + REPLICATION_INTERVAL_MS + 1;
    let out = host.poll(t);
    assert_eq!(out.events, vec![Event::FirstPlayerEntered { player: 1 }]);
    assert_eq!(host.application().unwrap().delivery_status(ticket), None);
    let decoded = host_frames(&client, &out.send)
        .into_iter()
        .filter(|(bytes, bits)| {
            frame::parse(
                BitSpan::new(bytes, 0, *bits).unwrap(),
                frame::Direction::FromHost,
            )
            .unwrap()
            .messages
            .is_some()
        })
        .collect::<Vec<_>>();
    assert_eq!(decoded.len(), 1);
    let frame = frame::parse(
        BitSpan::new(&decoded[0].0, 0, decoded[0].1).unwrap(),
        frame::Direction::FromHost,
    )
    .unwrap();
    assert_eq!(
        frame.messages.unwrap().groups[0].messages,
        vec![frame::Message::Empty]
    );
    host.receive(
        &client.app(&loading::state_response(4, 1, 1)),
        t + 4,
        &mut listener,
    );
    assert!(
        !host
            .poll(t + REPLICATION_INTERVAL_MS + 3)
            .events
            .iter()
            .any(|e| matches!(e, Event::FirstPlayerEntered { .. }))
    );
    let (fresh, _, _, _, _) = entry_setup();
    let Stage::Running { world, .. } = &fresh.stage else {
        panic!()
    };
    assert!(matches!(world.player_entry, entry::Entry::Pending { .. }));
}

#[test]
fn rejected_creation_and_other_players_do_not_enter_and_disconnect_releases_receipt() {
    let (mut host, mut client, mut listener, now, sent) = entry_setup();
    acknowledge_entry(&mut host, &mut client, &mut listener, &sent, now + 1, false);
    assert!(
        !host
            .poll(now + REPLICATION_INTERVAL_MS + 3)
            .events
            .iter()
            .any(|e| matches!(e, Event::FirstPlayerEntered { .. }))
    );
    let Stage::Running { world, .. } = &host.stage else {
        panic!()
    };
    assert!(matches!(world.player_entry, entry::Entry::Rejected));
    let mut other = owned_section(1);
    let Some(crate::replication::Initial::Player(player)) = &mut other.records[0].initial else {
        panic!()
    };
    player.value8 = HOST_SELECTOR as u8 + 1;
    assert_eq!(entry::Entry::Waiting.first_creation(&other), None);
    let (mut host, mut client, mut listener, now, _) = entry_setup();
    let Stage::Running { world, .. } = &host.stage else {
        panic!()
    };
    let entry::Entry::Pending { ticket, .. } = world.player_entry else {
        panic!()
    };
    host.receive(&client.close(), now + 1, &mut listener);
    assert_eq!(host.stage(), StageName::Closed);
    assert_eq!(host.application().unwrap().delivery_status(ticket), None);
}

#[test]
fn first_player_entry_preserves_ready_state_when_application_window_is_full() {
    let (mut host, mut client, mut listener, now, sent) = entry_setup();
    acknowledge_entry(&mut host, &mut client, &mut listener, &sent, now + 1, true);
    let Stage::Running { application, .. } = &mut host.stage else {
        panic!()
    };
    let empty = frame::Builder::new().build().unwrap();
    let mut latest = None;
    for _ in 0..64 {
        match application.send_frame(empty.span()) {
            Ok(body) => {
                latest = nfs_protocol::world::envelope::ZeroTail::new(1600)
                    .unwrap()
                    .decode(&body)
                    .unwrap()
                    .sequence()
                    .map(|s| s.number().value())
            }
            Err(application::Error::Window) => break,
            other => panic!("{other:?}"),
        }
    }
    let out = host.poll(now + REPLICATION_INTERVAL_MS + 3);
    assert!(
        !out.events
            .iter()
            .any(|e| matches!(e, Event::FirstPlayerEntered { .. }))
    );
    let Stage::Running { world, .. } = &host.stage else {
        panic!()
    };
    assert!(matches!(
        world.player_entry,
        entry::Entry::Ready { player: 1 }
    ));
    let mut control = crate::bits::BitWriter::new();
    control.put(1, 1).put(1, 4);
    let body = application::sequenced(HOST_SELECTOR, 6, latest.unwrap(), u32::MAX, &control);
    host.receive(
        &client.app(&body),
        now + REPLICATION_INTERVAL_MS + 4,
        &mut listener,
    );
    assert_eq!(
        host.poll(now + REPLICATION_INTERVAL_MS + 5).events,
        vec![Event::FirstPlayerEntered { player: 1 }]
    );
}

#[test]
fn dynamic_content_backpressure_preserves_handles_and_rejects_after_close() {
    let mut host = host();
    for _ in 0..MAX_CONTENT_CHUNKS / 64 {
        assert_eq!(
            host.queue_content(85, &[0; frame::CHUNK_BYTES * 64]),
            Ok(64)
        );
    }
    let progress = host.registration_progress();
    assert_eq!(
        host.queue_content(frame::EX_COLLECTION, &collection_bytes(&[7])),
        Err(Error::Application(application::Error::Window))
    );
    assert_eq!(host.registration_progress(), progress);
    assert_eq!(host.content.len(), MAX_CONTENT_CHUNKS);
    host.content.clear();
    assert!(
        host.queue_content(frame::EX_COLLECTION, &[0xff; 20])
            .is_err()
    );
    assert!(host.content.is_empty());
    assert_eq!(host.registration_progress(), progress);
    host.queue_content(frame::EX_COLLECTION, &collection_bytes(&[7]))
        .unwrap();
    assert_eq!(host.registration_progress().0, progress.0 + 1);
    host.stage = Stage::Closed(None);
    assert_eq!(host.queue_content(85, &[1]), Err(Error::Stage));
}

#[test]
fn file_deadline_releases_receipt_and_other_connection_is_unchanged() {
    use crate::files::{Completed, sender};
    let (mut a, _, _, now) = established_with_content();
    let (mut b, _, _, _) = established_with_content();
    let data = Completed {
        name: b"Items".to_vec(),
        bytes: vec![1, 2, 3],
    };
    a.queue_file(4, &data, now).unwrap();
    b.queue_file(5, &data, now).unwrap();
    a.poll(now + 1);
    b.poll(now + 1);
    let ta = a.files[0].as_ref().unwrap().receipt.unwrap();
    let tb = b.files[1].as_ref().unwrap().receipt.unwrap();
    let out = a.poll(now + sender::Policy::default().deadline_ms);
    assert!(out.events.contains(&Event::FileError {
        handler: 4,
        error: sender::Error::Deadline
    }));
    assert_eq!(a.application().unwrap().delivery_status(ta), None);
    assert_eq!(
        b.application().unwrap().delivery_status(tb),
        Some(application::delivery::Status::Pending)
    );
    assert!(a.files[0].is_none());
    assert!(b.files[1].is_some());
    assert_eq!(a.failures().file, 1);
    assert_eq!(b.failures(), Failures::default());
    a.poll(now + sender::Policy::default().deadline_ms + 1);
    assert_eq!(a.failures().file, 1);
}

#[test]
fn session_failures_count_errors_before_delivery_and_poll_early_returns_once() {
    let (mut host, mut client, mut listener, now) = established_with_content();
    assert_eq!(host.failures(), Failures::default());
    let before = host.application().unwrap().stats().frames_complete;
    let mut control = crate::bits::BitWriter::new();
    control.put(1, 1).put(1, 4);
    let invalid = application::sequenced(HOST_SELECTOR + 1, 3, 0, 0, &control);
    let out = host.receive(&client.app(&invalid), now + 1, &mut listener);
    assert_eq!(
        out.events,
        vec![Event::ApplicationError(application::Error::Selector)]
    );
    assert_eq!(host.application().unwrap().stats().frames_complete, before);
    assert_eq!(
        host.failures(),
        Failures {
            application: 1,
            ..Default::default()
        }
    );
    let valid = application::sequenced(HOST_SELECTOR, 3, 0, 0, &control);
    let out = host.receive(&client.app(&valid), now + 2, &mut listener);
    assert!(out.events.contains(&Event::Application(App::EmptyControl)));
    let out = host.receive(&client.app(&valid), now + 3, &mut listener);
    assert!(out.events.contains(&Event::Application(App::Duplicate)));
    let outside = application::sequenced(HOST_SELECTOR, 40, 0, 0, &control);
    let out = host.receive(&client.app(&outside), now + 4, &mut listener);
    assert!(out.events.contains(&Event::Application(App::OutOfWindow)));
    assert_eq!(
        host.failures(),
        Failures {
            application: 1,
            ..Default::default()
        }
    );
    let out = host.poll(now + 3);
    assert_eq!(
        out.events
            .iter()
            .filter(|e| matches!(e, Event::ApplicationError(_)))
            .count(),
        1
    );
    assert!(
        out.events
            .contains(&Event::ApplicationError(application::Error::Clock))
    );
    assert_eq!(host.failures().application, 2);
    host.poll(now + 5);
    assert_eq!(host.failures().application, 2);
    let out = host.receive(&[0], now + 6, &mut listener);
    assert_eq!(
        out.events
            .iter()
            .filter(|e| matches!(e, Event::Link(_)))
            .count(),
        1
    );
    assert_eq!(
        host.failures(),
        Failures {
            application: 2,
            link: 1,
            file: 0
        }
    );
    host.receive(&client.close(), now + 7, &mut listener);
    assert_eq!(host.stage(), StageName::Closed);
    host.poll(now + 8);
    host.receive(&[0], now + 9, &mut listener);
    assert_eq!(
        host.failures(),
        Failures {
            application: 2,
            link: 1,
            file: 0
        }
    );
    assert_eq!(entry_setup().0.failures(), Failures::default());
}

#[test]
fn file_queue_uses_current_receipts_retries_and_cleans_up_on_close() {
    use crate::files::{Completed, Receiver};
    let (mut host, mut client, mut listener, now) = established_with_content();
    let data = Completed {
        name: b"Items".to_vec(),
        bytes: (0..5000).map(|i| i as u8).collect(),
    };
    assert!(host.queue_file(3, &data, now).is_err());
    host.queue_file(4, &data, now).unwrap();
    assert_eq!(host.files_pending(), 1);
    assert!(host.queue_file(4, &data, now).is_err());
    let mut receiver = Receiver::default();
    let mut rejected_start = false;
    let mut rejected_block = false;
    let mut complete = false;
    for (number, step) in (3..).zip(0..20) {
        let t = now + step * 2 + 1;
        let out = host.poll(t);
        if out.events.contains(&Event::FileComplete { handler: 4 }) {
            assert_eq!(host.files_pending(), 0);
            complete = true;
            break;
        }
        assert_eq!(host.files_pending(), 1);
        assert!(
            !out.events
                .iter()
                .any(|e| matches!(e, Event::FileError { .. } | Event::ApplicationError(_)))
        );
        let reject = if out.events.contains(&Event::FileRecord {
            handler: 4,
            kind: 1,
        }) && !rejected_start
        {
            rejected_start = true;
            true
        } else if out.events.contains(&Event::FileRecord {
            handler: 4,
            kind: 2,
        }) && !rejected_block
        {
            rejected_block = true;
            true
        } else {
            false
        };
        if !reject {
            for (bytes, bits) in host_frames(&client, &out.send) {
                let parsed = frame::parse_with_files(
                    BitSpan::new(&bytes, 0, bits).unwrap(),
                    frame::Direction::FromHost,
                    [receiver.size(), None],
                )
                .unwrap();
                for (handler, record) in parsed.files {
                    assert_eq!(handler, 4);
                    receiver.receive(&record).unwrap();
                }
            }
        }
        let sequences = out
            .send
            .iter()
            .filter_map(|w| client.open(w))
            .filter_map(|b| {
                nfs_protocol::world::envelope::ZeroTail::new(1600)
                    .unwrap()
                    .decode(&b)
                    .unwrap()
                    .sequence()
                    .map(|s| s.number().value())
            })
            .collect::<Vec<_>>();
        let latest = *sequences.last().unwrap();
        let history = if reject {
            !(1 << (latest.wrapping_sub(sequences[0]) & 1023))
        } else {
            u32::MAX
        };
        let mut control = crate::bits::BitWriter::new();
        control.put(1, 1).put(1, 4);
        let body = application::sequenced(HOST_SELECTOR, number, latest, history, &control);
        host.receive(&client.app(&body), t + 1, &mut listener);
    }
    assert!(complete && rejected_start && rejected_block);
    assert_eq!(receiver.take_completed(), Some(data.clone()));
    assert!(!host.cancel_file(4));
    let t = now + 100;
    host.queue_file(4, &data, t).unwrap();
    host.queue_file(5, &data, t).unwrap();
    host.poll(t);
    let tickets = host
        .files
        .each_ref()
        .map(|f| f.as_ref().unwrap().receipt.unwrap());
    assert!(host.cancel_file(4));
    assert_eq!(
        host.application().unwrap().delivery_status(tickets[0]),
        None
    );
    host.receive(&client.close(), t + 1, &mut listener);
    assert!(host.files.iter().all(Option::is_none));
    assert_eq!(
        host.application().unwrap().delivery_status(tickets[1]),
        None
    );
}
impl Listener for AcceptSignals {
    fn frame(&mut self, _: application::Queued<'_>) -> bool {
        true
    }
}

fn owned_section(count: usize) -> Section {
    use crate::replication::players::{Players, Request};
    let mut players = Players::default();
    Section {
        float_bits: None,
        flag: false,
        deleted: vec![],
        setup: Some(nfs_protocol::world::ghost::Setup::Packed {
            tag: None,
            width: None,
            axes: [None; 3],
        }),
        records: (0..count)
            .map(|i| {
                players
                    .create(
                        (i / 8 + 1) as u8,
                        100 + i as u64,
                        Request {
                            name: b"LocalPlayer".to_vec(),
                            flag: false,
                            slot: (i % 8) as u8,
                        },
                    )
                    .unwrap()
                    .unwrap()
            })
            .collect(),
    }
}

#[test]
fn typed_scene_queue_tracks_lifecycle_and_preserves_failed_queue_state() {
    use crate::replication::{
        Record,
        sublevel::{Body, Content, Creation, Initial, Kind, Prefix, Profile, Update},
    };
    let (mut host, _, _, _) = established_with_content();
    let content = Content::new(vec![(7, Profile::new(true, &[Kind::Noop]).unwrap())]).unwrap();
    let record = Record::from_scene(
        1,
        Creation {
            prefix: Prefix {
                level_id: 0,
                content_key: 7,
                blueprint: None,
                word_a: 0,
                word_b: 0,
                constructor_word: None,
            },
            body: Body {
                initial: Some(vec![Initial::Empty]),
                updates: vec![Some(Update::Noop)],
            },
        },
        &content,
    )
    .unwrap();
    let mut section = owned_section(1);
    section.records = vec![record];
    host.queue_replication(section.clone()).unwrap();
    assert!(host.queue_replication(section.clone()).is_err());
    assert_eq!(host.replication_progress(), (0, 1));
    section.records[0].initial = None;
    host.queue_replication(section).unwrap();
    assert_eq!(host.replication_progress(), (0, 2));
}

#[test]
fn typed_sections_wait_for_state_time_then_fragment_without_template_inputs() {
    let (mut host, mut client, mut listener, now) = established_with_content();
    let section = owned_section(16);
    assert!(section.frame().unwrap().len() > DEFAULT_FRAGMENT_BITS);
    host.queue_replication(section.clone()).unwrap();
    assert_eq!(host.replication_progress(), (0, 1));
    assert!(host.poll(now + REPORT_TIMEOUT_MS - 1).events.is_empty());
    let t = loading::exchange(
        &mut host,
        &mut client,
        &mut listener,
        now + REPORT_TIMEOUT_MS,
    );
    let mut expected_section = section;
    expected_section.float_bits =
        Some(((t - host.application().unwrap().origin_ms().unwrap()) as f32 / 1000.0).to_bits());
    expected_section.flag = true;
    let expected = expected_section.frame().unwrap();
    let out = host.poll(t);
    assert_eq!(
        out.events,
        vec![Event::Replication {
            records: 16,
            fragments: 2,
            remaining: 0
        }]
    );
    assert_eq!(
        host_frames(&client, &out.send),
        vec![(expected.bytes().to_vec(), expected.len())]
    );
    assert_eq!(host.replication_progress(), (1, 0));
}

#[test]
fn launcher_notifications_follow_creations_and_advance_reliable_sequence_in_bounded_batches() {
    use crate::{
        launchers::Enabled,
        replication::{
            Record,
            sublevel::{Rpc, root},
        },
    };
    use nfs_protocol::world::rpc::{Envelope, Limits, RouteProfile, Serial};
    let (mut host, client, _, now) = established_with_content();
    let reply = Enabled {
        scene: 1,
        selector: 0,
        serial: Serial::new(0).unwrap(),
    };
    assert!(host.queue_launcher_enabled(reply).is_err());
    let content = root::content().unwrap();
    let creation = root::creation(
        Rpc {
            selector: 0,
            serial: reply.serial,
        },
        [None; root::REFERENCES],
    )
    .unwrap();
    let mut section = owned_section(1);
    section.records = vec![Record::from_scene(1, creation, &content).unwrap()];
    host.queue_replication(section).unwrap();
    for _ in 0..MAX_RPC_QUEUE {
        host.queue_launcher_enabled(reply).unwrap();
    }
    assert_eq!(
        host.queue_launcher_enabled(reply),
        Err(Error::Application(application::Error::Window))
    );
    let t = now + REPORT_TIMEOUT_MS;
    loading::after_exchange(&mut host, t);
    assert!(matches!(
        host.poll(t + 2 * STATE_WAIT_MS).events[..],
        [Event::Replication { .. }]
    ));
    assert_eq!(host.rpc_queue.len(), MAX_RPC_QUEUE);
    assert_eq!(host.rpc_progress(), (0, MAX_RPC_QUEUE));
    let mut previous = None;
    for step in 1..=2 {
        let out = host.poll(t + 2 * STATE_WAIT_MS + step * REPLICATION_INTERVAL_MS);
        assert_eq!(
            host.rpc_progress(),
            (step as usize * 15, MAX_RPC_QUEUE - step as usize * 15)
        );
        assert_eq!(
            out.events,
            vec![Event::RpcNotifications {
                count: 15,
                remaining: MAX_RPC_QUEUE - step as usize * 15
            }]
        );
        let frames = host_frames(&client, &out.send);
        let parsed = frame::parse(
            BitSpan::new(&frames[0].0, 0, frames[0].1).unwrap(),
            frame::Direction::FromHost,
        )
        .unwrap();
        let group = &parsed.messages.as_ref().unwrap().groups[0];
        if let Some(seq) = previous {
            assert_eq!(group.sequence, Some((seq + 15) & 127));
        }
        previous = group.sequence;
        for message in &group.messages {
            let frame::Message::Opaque { index: 47, body } = message else {
                panic!()
            };
            let envelope = Envelope::decode(
                *body,
                Limits {
                    max_input_bits: 200,
                    max_references: 1,
                    max_payload_bytes: 7,
                },
            )
            .unwrap();
            assert_eq!(envelope.references(), &[1]);
            assert_eq!(
                envelope
                    .route(RouteProfile::ClientReceive)
                    .unwrap()
                    .method_index(),
                0
            );
        }
    }
    host.stage = Stage::Closed(None);
    assert_eq!(host.queue_launcher_enabled(reply), Err(Error::Stage));
}

#[test]
fn scene_dependencies_wait_for_exact_observer_reports_without_timeout_bypass() {
    use crate::replication::{Record, sublevel::root};
    use nfs_protocol::world::rpc::Serial;
    let (mut host, mut client, mut listener, now) = established_with_content();
    let (mut other, _, _, _) = established_with_content();
    let content = root::traffic_content(&crate::test_data::TRAFFIC_KEYS).unwrap();
    let mut links = [None; root::REFERENCES];
    links[0] = Some(175);
    let root = root::creation(
        crate::replication::sublevel::Rpc {
            selector: 0,
            serial: Serial::new(0).unwrap(),
        },
        links,
    )
    .unwrap();
    let mut section = owned_section(1);
    section.records = vec![
        Record::from_scene(1, root, &content).unwrap(),
        Record::from_scene(
            2,
            root::traffic_creation(&crate::test_data::TRAFFIC_KEYS, 0, 176).unwrap(),
            &content,
        )
        .unwrap(),
    ];
    host.queue_replication(section.clone()).unwrap();
    other.queue_replication(section.clone()).unwrap();
    let mut player = owned_section(1);
    player.records[0].id = 3;
    host.queue_replication(player).unwrap();
    let t = now + REPORT_TIMEOUT_MS;
    for observer in [&mut host, &mut other] {
        loading::after_exchange(observer, t);
        assert_eq!(
            observer.poll(t + 2 * STATE_WAIT_MS).events,
            vec![Event::ReplicationWaiting { level: 175 }]
        );
    }
    let later = t + 2 * STATE_WAIT_MS + REPORT_TIMEOUT_MS;
    for (i, level) in [174, 174, 177].into_iter().enumerate() {
        host.receive(
            &client.app(&report_frame(3 + i as u16, level)),
            later + i as u64,
            &mut listener,
        );
    }
    assert_eq!(host.registration_progress().1, 2);
    assert!(host.poll(later + 5).events.is_empty());
    assert_eq!(host.replication_progress(), (0, 2));
    host.receive(&client.app(&report_frame(6, 175)), later + 6, &mut listener);
    assert_eq!(
        host.poll(later + 7).events,
        vec![Event::ReplicationWaiting { level: 176 }]
    );
    host.receive(&client.app(&report_frame(7, 176)), later + 8, &mut listener);
    section.float_bits = Some(
        ((later + 9 - host.application().unwrap().origin_ms().unwrap()) as f32 / 1000.0).to_bits(),
    );
    section.flag = true;
    let expected = section.frame().unwrap();
    let out = host.poll(later + 9);
    assert_eq!(
        out.events,
        vec![Event::Replication {
            records: 2,
            fragments: 1,
            remaining: 1
        }]
    );
    assert_eq!(
        host_frames(&client, &out.send),
        vec![(expected.bytes().to_vec(), expected.len())]
    );
    assert!(other.poll(later + 9).events.is_empty());
    assert_eq!(other.replication_progress(), (0, 1));
    assert_eq!(
        host.poll(later + 9 + REPLICATION_INTERVAL_MS).events,
        vec![Event::Replication {
            records: 1,
            fragments: 1,
            remaining: 0
        }]
    );
}

#[test]
fn root_absent_and_clear_links_need_no_ordinary_loading_report() {
    use crate::replication::{
        Record,
        sublevel::{Rpc, root},
    };
    use nfs_protocol::world::rpc::Serial;
    let mut links = [None; root::REFERENCES];
    links[0] = Some(u16::MAX);
    let creation = root::creation(
        Rpc {
            selector: 0,
            serial: Serial::new(0).unwrap(),
        },
        links,
    )
    .unwrap();
    let mut section = owned_section(1);
    section.records = vec![Record::from_scene(1, creation, &root::content().unwrap()).unwrap()];
    assert_eq!(missing_scene_level(&section, &Default::default()), None);
    section.records[0].initial = None;
    assert_eq!(missing_scene_level(&section, &Default::default()), None);
}

#[test]
fn native_outbound_buffer_budget_is_smaller_than_local_receive_limit() {
    let mut host = host();
    let mut section = owned_section(1);
    let baseline = section.records[0].clone();
    while section.frame().unwrap().len() <= application::OUTBOUND_FRAME_BITS {
        let mut record = baseline.clone();
        record.id = section.records.len() as u16 + 1;
        section.records.push(record);
    }
    assert!(section.frame().unwrap().len() < Policy::default().max_frame_bits);
    assert_eq!(
        host.queue_replication(section.clone()),
        Err(Error::Application(application::Error::Bound))
    );
    assert!(host.replication_bindings.is_empty());
    assert_eq!(host.replication_progress(), (0, 0));
    section.records.pop();
    host.queue_replication(section).unwrap();
}

#[test]
fn actor_rpc_requires_all_three_typed_ghosts_in_the_same_observer_queue() {
    use crate::{
        participants::HostRpc,
        replication::{
            players::{Players, Request},
            sublevel::{self, gameplay, ordinary},
        },
    };
    use nfs_protocol::world::rpc::Serial;
    let content = sublevel::Content::new(vec![(
        crate::test_data::GAMEPLAY_KEY,
        gameplay::profile().unwrap(),
    )])
    .unwrap();
    let creation = ordinary::creation(
        1,
        crate::test_data::GAMEPLAY_KEY,
        None,
        Serial::new(0).unwrap(),
        &ordinary::unpopulated(content.profile(crate::test_data::GAMEPLAY_KEY).unwrap()).unwrap(),
        &content,
    )
    .unwrap();
    let mut players = Players::default();
    let mut records = players
        .create_scenes(&[1], vec![creation], &content)
        .unwrap();
    let p = players
        .create(
            1,
            101,
            Request {
                name: b"Local".to_vec(),
                flag: false,
                slot: 0,
            },
        )
        .unwrap()
        .unwrap();
    let participant = players.join(1, 101, p.id).unwrap().unwrap();
    let actor = players
        .create_actor(1, 101, participant.id)
        .unwrap()
        .unwrap();
    let binding = players
        .bind_actor(crate::test_data::GAMEPLAY_KEY, 1, 101, participant.id)
        .unwrap()
        .unwrap();
    records.extend([p, participant]);
    let mut host = host();
    let mut section = owned_section(1);
    section.records = records;
    host.queue_replication(section.clone()).unwrap();
    assert!(host.queue_rpc(HostRpc::Actor(binding)).is_err());
    assert!(host.rpc_queue.is_empty());
    section.records = vec![actor];
    host.queue_replication(section).unwrap();
    assert!(
        host.queue_rpc(HostRpc::Actor(crate::actors::Binding {
            actor: binding.participant,
            ..binding
        }))
        .is_err()
    );
    host.queue_rpc(HostRpc::Actor(binding)).unwrap();
    assert_eq!(host.rpc_queue.len(), 1);
}

#[test]
fn sequence_stop_accepts_any_sequence_root_entity_and_refuses_other_entities() {
    use crate::{
        participants::HostRpc,
        replication::entity::{
            Kind, Profile,
            creation::{Asset, Catalog, Content},
        },
        sequences::Stop,
    };
    use nfs_protocol::world::rpc::Serial;
    let asset = |local_index| Asset {
        bundle: 3,
        type_id: 4,
        local_index,
    };
    // An event intro's layout (E995): sequence root, optional reference, five
    // channel references and a reference list; and a plain logic layout.
    let intro = Profile::new(&[
        Kind::SequenceRoot,
        Kind::RpcOptionalReference,
        Kind::RpcReference,
        Kind::RpcReference,
        Kind::RpcReference,
        Kind::RpcReference,
        Kind::RpcReference,
        Kind::RpcReferences,
    ])
    .unwrap();
    let logic = Profile::new(&[Kind::Root, Kind::Rpc]).unwrap();
    let content = Content::new(
        vec![Catalog::new(3, &[(4, 2)]).unwrap()],
        vec![(asset(0), intro), (asset(1), logic)],
    )
    .unwrap();
    let mut host = host();
    host.replication_bindings = Bindings::default().with_entities(content);
    host.replication_bindings.bind_entity(7, asset(0)).unwrap();
    host.replication_bindings.bind_entity(8, asset(1)).unwrap();
    let stop = |sequence| {
        HostRpc::SequenceStop(Stop {
            sequence,
            serial: Serial::new(1).unwrap(),
        })
    };
    host.queue_rpc(stop(7)).unwrap();
    assert_eq!(host.rpc_queue.len(), 1);
    assert!(host.queue_rpc(stop(8)).is_err());
    assert!(host.queue_rpc(stop(9)).is_err());
    assert_eq!(host.rpc_queue.len(), 1);
}

#[test]
fn typed_queue_limits_and_rejections_preserve_observer_bindings() {
    let mut host = host();
    let section = owned_section(1);
    let mut oversized = section.clone();
    oversized.records = vec![section.records[0].clone(); 100];
    assert_eq!(
        host.queue_replication(oversized),
        Err(Error::Application(application::Error::Bound))
    );
    assert!(host.replication_bindings.is_empty());
    for id in 1..=MAX_REPLICATION_QUEUE {
        let mut next = section.clone();
        next.records[0].id = id as u16;
        host.queue_replication(next).unwrap();
    }
    let mut next = section.clone();
    next.records[0].id = 33;
    assert_eq!(
        host.queue_replication(next.clone()),
        Err(Error::Application(application::Error::Window))
    );
    assert_eq!(host.replication_bindings.len(), 32);
    let (_, bits, _) = host.replication_queue.pop_front().unwrap();
    host.replication_bits -= bits;
    host.queue_replication(next).unwrap();
    assert_eq!(host.replication_bindings.len(), 33);
    host.stage = Stage::Closed(None);
    assert_eq!(host.queue_replication(section), Err(Error::Stage));
}

#[test]
fn entry_messages_precede_deletion_in_one_frame_with_atomic_backpressure() {
    use crate::{
        participants::HostRpc,
        replication::{
            Record,
            sublevel::{Rpc, root},
        },
    };
    use nfs_protocol::world::rpc::Serial;
    let (mut host, client, _, now) = established_with_content();
    let content = root::content().unwrap();
    let creation = root::creation(
        Rpc {
            selector: 0,
            serial: Serial::new(0).unwrap(),
        },
        [None; root::REFERENCES],
    )
    .unwrap();
    let mut initial = owned_section(1);
    initial.records = vec![Record::from_scene(1, creation, &content).unwrap()];
    host.queue_replication(initial).unwrap();
    let enabled = crate::launchers::Enabled {
        scene: 1,
        selector: 0,
        serial: Serial::new(0).unwrap(),
    };
    host.queue_launcher_enabled(enabled).unwrap();
    let section = Section {
        float_bits: None,
        flag: false,
        deleted: vec![1],
        setup: None,
        records: vec![],
    };
    let event = HostRpc::Event(crate::logic::Fire {
        event: 3,
        player: None,
        target: crate::logic::EntityRef {
            ghost: 1,
            entity: 1,
        },
    });
    let before = vec![event; 15];
    let window = Err(Error::Application(application::Error::Window));
    assert_eq!(
        host.queue_replication_after(section.clone(), before.clone()),
        window
    );
    assert!(host.replication_bindings.get(1).is_some());
    let t = now + REPORT_TIMEOUT_MS;
    loading::after_exchange(&mut host, t);
    let t = t + 2 * STATE_WAIT_MS;
    host.poll(t);
    host.poll(t + REPLICATION_INTERVAL_MS);
    assert!(host.rpc_queue.is_empty());
    let unchanged = host.replication_bindings.clone();
    assert!(
        host.queue_replication_after(section.clone(), vec![event; 16])
            .is_err()
    );
    assert_eq!(host.replication_bindings, unchanged);
    let missing = HostRpc::Event(crate::logic::Fire {
        target: crate::logic::EntityRef {
            ghost: 2,
            entity: 1,
        },
        event: 3,
        player: None,
    });
    assert!(
        host.queue_replication_after(section.clone(), vec![missing])
            .is_err()
    );
    assert_eq!(host.replication_bindings, unchanged);
    let Stage::Running { world, .. } = &host.stage else {
        panic!()
    };
    let sequence = world.next_ex_sequence;
    let mut expected_section = section.clone();
    expected_section.float_bits = Some(
        ((t + 2 * REPLICATION_INTERVAL_MS - host.application().unwrap().origin_ms().unwrap())
            as f32
            / 1000.0)
            .to_bits(),
    );
    expected_section.flag = true;
    let expected = replication_frame(&expected_section, &before, sequence).unwrap();
    let parsed = frame::parse(expected.span(), frame::Direction::FromHost).unwrap();
    assert_eq!(parsed.mask, (1 << frame::MESSAGES) | (1 << frame::GHOST));
    let messages = &parsed.messages.as_ref().unwrap().groups[0];
    assert_eq!(messages.sequence, Some(sequence));
    assert_eq!(messages.messages.len(), 15);
    assert!(
        messages
            .messages
            .iter()
            .all(|m| matches!(m, frame::Message::Opaque { index: 86, .. }))
    );
    assert_eq!(parsed.ghost.as_ref().unwrap().prefix.deleted(), &[1]);
    host.queue_replication_after(section, before).unwrap();
    assert!(host.replication_bindings.get(1).is_none());
    let out = host.poll(t + 2 * REPLICATION_INTERVAL_MS);
    assert_eq!(
        host_frames(&client, &out.send),
        vec![(expected.bytes().to_vec(), expected.len())]
    );
    assert!(out.events.contains(&Event::RpcNotifications {
        count: 15,
        remaining: 0
    }));
    let Stage::Running { world, .. } = &host.stage else {
        panic!()
    };
    assert_eq!(world.next_ex_sequence, (sequence + 15) & 127);
    assert_eq!(host.replication_progress(), (2, 0));
    assert!(
        host.poll(t + 3 * REPLICATION_INTERVAL_MS)
            .events
            .iter()
            .all(|e| !matches!(
                e,
                Event::Replication { .. } | Event::RpcNotifications { .. }
            ))
    );
}

#[test]
fn refused_frames_do_not_commit_world_pacing_signals() {
    let wire = frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(0),
                messages: vec![frame::Message::State(7)],
            }],
        )
        .build()
        .unwrap();
    let mut retain = Retain::default();
    let mut observing = Observing {
        inner: &mut retain,
        seen: Seen::default(),
        waiting_level: None,
        messages: Default::default(),
        now_ms: 0,
        invalid: false,
    };
    assert!(!observing.frame(application::Queued {
        advertised: None,
        frame: 1,
        data: wire.span()
    }));
    assert_eq!(observing.seen.state_reports, 0);
    let mut accept = AcceptSignals;
    observing.inner = &mut accept;
    assert!(observing.frame(application::Queued {
        advertised: None,
        frame: 1,
        data: wire.span()
    }));
    assert_eq!(observing.seen.state_reports, 1);
}

#[test]
fn a_blocked_scene_can_learn_its_load_confirmation_without_accepting_gameplay() {
    use crate::replication::{Record, sublevel::root};
    for level in [175, 206, 511] {
        let (mut host, mut client, _, now) = established_with_content();
        let content = root::traffic_content(&crate::test_data::TRAFFIC_KEYS).unwrap();
        let mut section = owned_section(1);
        section.records = vec![
            Record::from_scene(
                1,
                root::traffic_creation(&crate::test_data::TRAFFIC_KEYS, 0, level).unwrap(),
                &content,
            )
            .unwrap(),
        ];
        host.queue_replication(section).unwrap();
        let now = now + REPORT_TIMEOUT_MS;
        loading::after_exchange(&mut host, now);
        let now = now + 2 * STATE_WAIT_MS;
        assert_eq!(
            host.poll(now).events,
            vec![Event::ReplicationWaiting { level }]
        );

        // A load confirmation shares the reliable frame with gameplay which
        // cannot commit while its resulting output queue is full. Learning
        // unrelated load IDs or pacing signals must still wait for acceptance.
        let mut report = crate::bits::BitWriter::new();
        report
            .put(1, 32)
            .put(2, 32)
            .put(level.into(), 16)
            .put(600, 16);
        let frame = frame::Builder::new()
            .messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(1),
                    messages: vec![
                        frame::Message::State(7),
                        frame::Message::Opaque {
                            index: frame::EX_SUBLEVEL_REPORT,
                            body: report.span(),
                        },
                    ],
                }],
            )
            .build()
            .unwrap();
        let input = |number| {
            let mut queued = crate::bits::BitWriter::new();
            queued
                .put(0, 1)
                .put(0, 1)
                .put(number, 16)
                .put(0, 6)
                .put(frame.len() as u64, 15)
                .put(1, 1)
                .put_span(frame.span());
            application::sequenced(HOST_SELECTOR, number as u16, 0, 0, &queued)
        };
        let accepted = host.application().unwrap().stats().frames_accepted;
        let state_reports = match &host.stage {
            Stage::Running { world, .. } => world.state_reports,
            _ => panic!(),
        };
        let mut backpressure = Retain::default();
        host.receive(&client.app(&input(3)), now + 1, &mut backpressure);
        assert_eq!(backpressure.frames, 1);
        assert_eq!(
            host.application().unwrap().stats().frames_accepted,
            accepted,
            "readiness must not acknowledge rejected gameplay"
        );
        let Stage::Running { world, .. } = &host.stage else {
            panic!()
        };
        assert!(
            world.reported.contains(&level),
            "refusing the confirmation deadlocks the queue that needs it"
        );
        assert!(!world.reported.contains(&600));
        assert_eq!(world.state_reports, state_reports);
        assert!(host.poll(now + 2).events.iter().any(|e| matches!(
            e,
            Event::Replication {
                records: 1,
                remaining: 0,
                ..
            }
        )));

        // The rejected gameplay remains available in a later handler frame's
        // retry, with the same MessageEX sequence, after the output drains.
        let mut accept = AcceptSignals;
        host.receive(&client.app(&input(4)), now + 3, &mut accept);
        assert_eq!(
            host.application().unwrap().stats().frames_accepted,
            accepted + 1
        );
        let Stage::Running { world, .. } = &host.stage else {
            panic!()
        };
        assert!(world.reported.contains(&600));
        assert_eq!(world.state_reports, state_reports + 1);
    }
}

#[test]
fn learning_a_blocked_load_does_not_skip_reliable_gaps_or_repeat_gameplay() {
    #[derive(Default)]
    struct Ordered {
        receiver: frame::receive::Receiver,
        allowed: bool,
        committed: usize,
    }
    impl Listener for Ordered {
        fn frame(&mut self, queued: application::Queued<'_>) -> bool {
            let parsed = frame::parse_prefix(queued.data, frame::Direction::FromClient).unwrap();
            let Ok((next, messages)) = self.receiver.prepare(parsed.messages.as_ref().unwrap(), 1)
            else {
                return false;
            };
            if !self.allowed {
                return false;
            }
            self.committed += messages.len();
            self.receiver = next;
            true
        }
    }
    let group = |sequence, messages| frame::Group {
        channel: 0,
        sequence: Some(sequence),
        messages,
    };
    let mut gameplay = Ordered::default();
    let (receiver, _) = gameplay
        .receiver
        .prepare(
            &frame::Messages {
                state: None,
                groups: vec![group(57, vec![frame::Message::State(1)])],
            },
            0,
        )
        .unwrap();
    gameplay.receiver = receiver;
    let mut load = crate::bits::BitWriter::new();
    load.put(1, 32).put(1, 32).put(206, 16);
    let future = frame::Builder::new()
        .messages(
            None,
            vec![group(
                73,
                vec![
                    frame::Message::Opaque {
                        index: frame::EX_SUBLEVEL_REPORT,
                        body: load.span(),
                    },
                    frame::Message::State(7),
                ],
            )],
        )
        .build()
        .unwrap();
    let missing = frame::Builder::new()
        .messages(None, vec![group(58, vec![frame::Message::State(2); 15])])
        .build()
        .unwrap();
    let mut observing = Observing {
        inner: &mut gameplay,
        seen: Seen::default(),
        waiting_level: Some(206),
        messages: Default::default(),
        now_ms: 0,
        invalid: false,
    };
    assert!(!observing.frame(application::Queued {
        advertised: None,
        frame: 1,
        data: future.span(),
    }));
    assert_eq!(observing.seen.reported, [206]);
    assert_eq!(observing.seen.state_reports, 0);
    assert_eq!(gameplay.committed, 0);

    // Resource readiness releases output capacity. Fill the original gap first,
    // then accept its successor, and suppress an old reliable-message retry.
    gameplay.allowed = true;
    for (number, data) in [missing.span(), future.span(), missing.span()]
        .into_iter()
        .enumerate()
    {
        assert!(gameplay.frame(application::Queued {
            advertised: None,
            frame: number as u16 + 2,
            data,
        }));
    }
    assert_eq!(gameplay.committed, 17);
}

#[test]
fn typed_dispatch_retains_the_section_under_application_backpressure() {
    let (mut host, mut client, mut listener, now) = established_with_content();
    let t = now + REPORT_TIMEOUT_MS;
    loading::after_exchange(&mut host, t);
    host.queue_replication(owned_section(1)).unwrap();
    let empty = frame::Builder::new().build().unwrap();
    let mut last = 0;
    for _ in 0..32 {
        match host.send_frame(empty.span(), t + STATE_WAIT_MS) {
            Ok(wire) => {
                let body = client.open(&wire).unwrap();
                let e = nfs_protocol::world::envelope::ZeroTail::new(1600)
                    .unwrap()
                    .decode(&body)
                    .unwrap();
                last = e.sequence().unwrap().number().value();
            }
            Err(Error::Application(application::Error::Window)) => break,
            other => panic!("{other:?}"),
        }
    }
    let out = host.poll(t + 2 * STATE_WAIT_MS);
    assert!(
        !out.events
            .iter()
            .any(|e| matches!(e, Event::Replication { .. }))
    );
    assert_eq!(host.replication_progress(), (0, 1));
    let mut payload = crate::bits::BitWriter::new();
    payload.put(1, 1).put(1, 4);
    let body = application::sequenced(HOST_SELECTOR, 3, last, u32::MAX, &payload);
    host.receive(&client.app(&body), t + 2 * STATE_WAIT_MS + 1, &mut listener);
    let out = host.poll(t + 2 * STATE_WAIT_MS + 50);
    assert!(
        out.events
            .iter()
            .any(|e| matches!(e, Event::Replication { records: 1, .. })),
        "{:?}",
        out.events
    );
    assert_eq!(host.replication_progress(), (1, 0));
}

fn codec() -> Codec {
    let key = session_key(
        "11111111-2222-3333-4444-555555555555",
        "66666666-7777-8888-9999-aaaaaaaaaaaa",
    )
    .unwrap();
    Codec::new(key, std::array::from_fn(|i| (i * 5) as u8)).unwrap()
}

fn host() -> Host {
    Host::new(
        codec(),
        Ids {
            client: CLIENT,
            host: HOST,
            host_index: HOST_INDEX,
        },
        Policy::default(),
        Box::new(Fixed::new([0x1111_1111, 0x2222_2222, 0x3333_3333])),
    )
    .unwrap()
}

fn motion_fixture() -> (Host, impl Fn(u16, u32) -> Section) {
    use crate::replication::{
        Record, Update,
        entity::creation::{Asset, Catalog, Content},
        section::Setup,
        vehicle,
    };
    use std::sync::Arc;
    let asset = Asset {
        bundle: 3,
        type_id: 4,
        local_index: 0,
    };
    let content = Content::new(vec![Catalog::new(3, &[(4, 1)]).unwrap()], vec![])
        .unwrap()
        .with_vehicles(vec![(
            asset,
            vehicle::Profile::new(vec![
                vehicle::Kind::Root {
                    property_owner: false,
                },
                vehicle::Kind::Chassis,
            ])
            .unwrap(),
        )])
        .unwrap();
    let motion = vehicle::chassis::Motion {
        identity: 1f64.to_bits(),
        revision: 30,
        position: [0.; 3],
        quaternion: [0., 0., 0., 1.],
        velocity: [1., 0., 0.],
        angular_velocity: [0.; 3],
    }
    .encode([0.; 3], &vehicle::chassis::Controls::new(4).unwrap())
    .unwrap();
    let binding = Arc::new(content.binding(asset).unwrap());
    let section = move |id, revision| Section {
        float_bits: Some(0),
        flag: true,
        deleted: vec![],
        setup: Some(Setup::RawEscape([0; 3])),
        records: vec![Record {
            id,
            initial: None,
            update: Update::Vehicle {
                binding: binding.clone(),
                fields: vec![
                    None,
                    Some(vehicle::Update::Chassis(Box::new(vehicle::ChassisUpdate {
                        physics: Some(vehicle::Physics {
                            identity_words: [
                                motion.identity_words[0],
                                motion.identity_words[1],
                                revision,
                            ],
                            ..motion.clone()
                        }),
                        flag: None,
                        custom: None,
                        pair: None,
                    }))),
                ],
            },
        }],
    };
    let mut host = host();
    host.replication_bindings = Bindings::default().with_entities(content.clone());
    for id in 1..=3 {
        host.replication_bindings.bind_entity(id, asset).unwrap();
    }
    (host, section)
}

#[test]
fn adjacent_car_motion_shares_one_bounded_frame_and_preserves_barriers() {
    use crate::replication::{Update, vehicle};
    let (mut host, snapshot) = motion_fixture();
    let section = |id| snapshot(id, 30);
    host.queue_replication(section(1)).unwrap();
    host.queue_replication(section(2)).unwrap();
    assert_eq!(host.replication_progress(), (0, 1));
    assert_eq!(
        host.replication_queue[0]
            .0
            .records
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        host.replication_bits,
        replication_frame(&host.replication_queue[0].0, &[], 0)
            .unwrap()
            .len()
    );
    // Unknown identities must still fail atomically on the co-publication path.
    let before = host.replication_queue.clone();
    assert!(host.queue_replication(section(4)).is_err());
    assert_eq!(host.replication_queue, before);
    // Equal/regressing revisions cannot replace a newer pending sample.
    host.queue_replication(section(1)).unwrap();
    assert_eq!(host.replication_progress(), (0, 2));
    // Frame limits keep two valid individual publications separate.
    host.max_replication_frame_bits = replication_frame(&section(3), &[], 0).unwrap().len();
    host.queue_replication(section(3)).unwrap();
    assert_eq!(host.replication_progress(), (0, 3));
    let mut deletion = owned_section(0);
    deletion.setup = None;
    deletion.deleted.push(2);
    host.queue_replication(deletion).unwrap();
    host.queue_replication(section(1)).unwrap();
    assert_eq!(host.replication_progress(), (0, 5));
    assert!(host.queue_replication(section(2)).is_err());
    // Payload changes other than chassis motion never coalesce.
    let mut changed = section(3);
    let Update::Vehicle { fields, .. } = &mut changed.records[0].update else {
        panic!()
    };
    let Some(vehicle::Update::Chassis(c)) = &mut fields[1] else {
        panic!()
    };
    c.flag = Some(true);
    assert!(super::motion::combine(&section(1), &changed).is_none());
}

#[test]
fn waiting_rpc_preserves_its_motion_boundary_without_disabling_later_coalescing() {
    use crate::{
        logic::{EntityRef, Fire},
        participants::HostRpc,
    };
    let (mut host, snapshot) = motion_fixture();
    let callback = HostRpc::Event(Fire {
        target: EntityRef {
            ghost: 1,
            entity: 0,
        },
        event: 7,
        player: None,
    });
    host.queue_replication(snapshot(1, 1)).unwrap();
    host.queue_rpc(callback).unwrap();
    // The first snapshot after a callback cannot replace one queued before it.
    host.queue_replication(snapshot(1, 2)).unwrap();
    assert_eq!(host.replication_progress(), (0, 2));
    // The callback may await an unsent creation or a blocked transport. Later
    // absolute snapshots can still supersede each other on this side of it.
    for revision in 3..=120 {
        for id in 1..=3 {
            host.queue_replication(snapshot(id, revision)).unwrap();
        }
        assert_eq!(host.replication_progress(), (0, 2));
    }
    assert_eq!(host.replication_queue[0].0.records, snapshot(1, 1).records);
    for id in 1..=3 {
        assert_eq!(
            host.replication_queue[1]
                .0
                .records
                .iter()
                .find(|r| r.id == id),
            snapshot(id, 120).records.first()
        );
    }
    host.queue_rpc(callback).unwrap();
    host.queue_replication(snapshot(1, 121)).unwrap();
    host.queue_replication(snapshot(1, 122)).unwrap();
    assert_eq!(host.replication_progress(), (0, 3));
    assert_eq!(host.rpc_queue, [callback, callback]);
    assert_eq!(
        host.replication_queue[2].0.records,
        snapshot(1, 122).records
    );
    assert_eq!(
        host.replication_bits,
        host.replication_queue.iter().map(|(_, bits, _)| bits).sum()
    );
}

#[test]
fn pending_notifications_fence_motion_once_without_disabling_batching() {
    use crate::{participants::HostRpc, replication::sublevel};
    let (mut host, snapshot) = motion_fixture();
    let content = sublevel::Content::new(vec![(
        7,
        sublevel::Profile::new(false, &[sublevel::Kind::Noop]).unwrap(),
    )])
    .unwrap();
    let creation = sublevel::ordinary::creation(
        1,
        7,
        None,
        nfs_protocol::world::rpc::Serial::new(0).unwrap(),
        &sublevel::ordinary::unpopulated(content.profile(7).unwrap()).unwrap(),
        &content,
    )
    .unwrap();
    let mut scene = owned_section(0);
    scene
        .records
        .push(crate::replication::Record::from_scene(20, creation, &content).unwrap());
    host.replication_bindings.apply(&scene).unwrap();
    let event = HostRpc::Event(crate::logic::Fire {
        target: crate::logic::EntityRef {
            ghost: 20,
            entity: 1,
        },
        event: 3,
        player: None,
    });
    host.queue_replication(snapshot(1, 1)).unwrap();
    host.queue_rpc(event).unwrap();
    for revision in 2..=120 {
        for id in 1..=3 {
            host.queue_replication(snapshot(id, revision)).unwrap();
        }
        // Keep the sample before the notification separate, but all reports
        // after it share one current frame even while the notification waits.
        assert_eq!(host.replication_progress(), (0, 2));
    }
    assert_eq!(host.replication_queue[0].0.records, snapshot(1, 1).records);
    let retained = host.replication_queue[1].0.clone();
    assert_eq!(retained.records.len(), 3);
    assert_eq!(retained.records[0], snapshot(1, 120).records[0]);
    host.queue_rpc(event).unwrap();
    host.queue_replication(snapshot(1, 121)).unwrap();
    host.queue_replication(snapshot(2, 121)).unwrap();
    assert_eq!(host.replication_progress(), (0, 3));
    assert_eq!(host.replication_queue[1].0, retained);
    assert_eq!(host.rpc_queue.len(), 2);
    assert_eq!(
        host.replication_bits,
        host.replication_queue
            .iter()
            .map(|(_, bits, _)| bits)
            .sum::<usize>()
    );
}

#[test]
fn unsent_car_and_minimap_snapshots_stay_current_during_backpressure() {
    use crate::replication::{Initial, Record, Update, location_map, section::Setup, vehicle};
    let (mut host, section) = motion_fixture();
    let sample = section(1, 0);
    let Update::Vehicle { binding, .. } = &sample.records[0].update else {
        panic!()
    };
    for id in 4..=9 {
        host.replication_bindings
            .bind_entity(id, binding.asset())
            .unwrap();
    }
    let map = |x| Record {
        id: 10,
        initial: None,
        update: Update::ParticipantLocationMap(location_map::Update {
            groups: Some(vec![location_map::Group {
                value: 1,
                entries: vec![location_map::Entry {
                    participant: 1,
                    x: location_map::Coordinate::from_world(x).unwrap(),
                    z: location_map::Coordinate::from_world(0.).unwrap(),
                }],
            }]),
        }),
    };
    let mut creation = section(1, 0);
    creation.records = vec![Record {
        initial: Some(Initial::ParticipantLocationMap),
        ..map(0.)
    }];
    host.replication_bindings.apply(&creation).unwrap();
    // The transport can be blocked for seconds. Retain one current snapshot,
    // not a sequence of obsolete corrections to replay when it recovers.
    for revision in 1..=120 {
        let mut player = section(1, revision);
        player.records.push(map(revision as f32 * 5.));
        host.queue_replication(player).unwrap();
        for id in 2..=9 {
            host.queue_replication(section(id, revision)).unwrap();
        }
        assert_eq!(host.replication_progress(), (0, 1));
    }
    let retained = &host.replication_queue[0].0;
    assert_eq!(retained.records.len(), 10);
    assert_eq!(retained.records[1], map(600.));
    for record in retained.records.iter().filter(|r| r.id != 10) {
        let Update::Vehicle { fields, .. } = &record.update else {
            panic!()
        };
        let Some(vehicle::Update::Chassis(c)) = &fields[1] else {
            panic!()
        };
        assert_eq!(c.physics.as_ref().unwrap().identity_words[2], 120);
    }
    assert_eq!(
        host.replication_bits,
        replication_frame(retained, &[], 0).unwrap().len()
    );
    // Leaving the world (map membership changes) is a lifecycle boundary.
    let mut leave = Section {
        records: vec![Record {
            id: 10,
            initial: None,
            update: Update::ParticipantLocationMap(location_map::Update {
                groups: Some(vec![]),
            }),
        }],
        ..section(1, 121)
    };
    host.queue_replication(leave.clone()).unwrap();
    assert_eq!(host.replication_progress(), (0, 2));
    leave.setup = Some(Setup::Packed {
        tag: None,
        width: None,
        axes: [None; 3],
    });
    assert!(super::motion::combine(&section(1, 121), &leave).is_none());
}

struct Client {
    codec: Codec,
    tx: u16,
    unreliable: u32,
}

impl Client {
    fn new() -> Self {
        Self {
            codec: codec(),
            tx: 0,
            unreliable: 128,
        }
    }
    fn send(&mut self, header: Header, bytes: Vec<u8>) -> Vec<u8> {
        let wire = self
            .codec
            .encode(header, &[Part { channel: 0, bytes }])
            .unwrap();
        let header_len = match header {
            Header::Initial { .. } => 11,
            Header::Ordinary { .. } => 4,
        };
        self.tx = transport::advance(self.tx, wire.len() - header_len).unwrap();
        wire
    }
    fn request(&mut self) -> Vec<u8> {
        let header = Header::Initial {
            sender: CLIENT,
            index: CLIENT_INDEX,
            peer_known: false,
            cursor: self.tx,
        };
        self.send(
            header,
            [1, TOKEN, CLIENT]
                .iter()
                .flat_map(|w| w.to_be_bytes())
                .collect(),
        )
    }
    fn ordinary(&self) -> Header {
        Header::Ordinary {
            index: HOST_INDEX,
            cursor: self.tx,
        }
    }
    fn confirm(&mut self) -> Vec<u8> {
        let header = self.ordinary();
        self.send(
            header,
            [2, TOKEN].iter().flat_map(|w| w.to_be_bytes()).collect(),
        )
    }
    fn close(&mut self) -> Vec<u8> {
        let header = self.ordinary();
        self.send(
            header,
            [3, TOKEN].iter().flat_map(|w| w.to_be_bytes()).collect(),
        )
    }
    fn sync(&mut self, now: u32) -> Vec<u8> {
        let mut inner = 256u32.to_be_bytes().to_vec();
        inner.extend_from_slice(&256u32.to_be_bytes());
        inner.extend_from_slice(&now.to_be_bytes());
        inner.extend_from_slice(&(now + 3).to_be_bytes());
        inner.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 16, 0x40]);
        let header = self.ordinary();
        self.send(header, inner)
    }
    fn app(&mut self, body: &[u8]) -> Vec<u8> {
        let mut inner = self.unreliable.to_be_bytes().to_vec();
        self.unreliable += 1;
        inner.extend_from_slice(&256u32.to_be_bytes());
        inner.extend_from_slice(body);
        inner.push(6);
        let header = self.ordinary();
        self.send(header, inner)
    }
    fn open(&self, wire: &[u8]) -> Option<Vec<u8>> {
        let inner = self.codec.decode(wire).unwrap().parts[0].bytes.clone();
        let parsed = crate::link::parse(&inner).unwrap();
        parsed
            .envelopes
            .iter()
            .find(|e| e.kind == 6)
            .map(|e| e.body.to_vec())
    }
}

fn client5(client_fresh: u32) -> Vec<u8> {
    Client5 {
        peer_echo: 0x4444_4444,
        client_fresh,
        client_selector: 7,
        callback: vec![],
    }
    .new_body(HOST_SELECTOR)
    .unwrap()
}

#[test]
fn a_full_session_runs_from_handshake_to_the_timing_exchange() {
    let mut host = host();
    let mut client = Client::new();
    let mut listener = Retain::default();
    assert_eq!(host.stage(), StageName::Handshake);

    let out = host.receive(&client.request(), 1_000, &mut listener);
    assert_eq!(out.events, vec![Event::HandshakeReply]);
    assert_eq!(out.send[0].len(), 37);
    let out = host.receive(&client.confirm(), 1_010, &mut listener);
    assert_eq!(out.events, vec![Event::Established]);
    assert_eq!(out.send[0].len(), 43, "initial host sync");
    assert_eq!(host.stage(), StageName::Running);

    let out = host.receive(&client.sync(1_010), 1_020, &mut listener);
    assert_eq!(out.send.len(), 1);
    assert_eq!(out.send[0].len(), 26, "empty acknowledgement");
    assert!(host.link().unwrap().running());

    let out = host.receive(
        &client.app(&Request13::new([1, 2]).encode()),
        1_100,
        &mut listener,
    );
    assert_eq!(
        out.events,
        vec![Event::Application(App::ChallengeRequest {
            repeated: false
        })]
    );
    assert_eq!(out.send[0].len(), 35, "observed 35-byte challenge wire");
    let out = host.receive(
        &client.app(&OpaqueAnswer16::new([1; 20], &[]).unwrap().encode()),
        1_110,
        &mut listener,
    );
    assert_eq!(out.send[0].len(), 35, "observed 35-byte Host1 wire");
    let out = host.receive(
        &client.app(
            &Admission::new(Message::Client2 {
                peer_token: 0x4444_4444,
                engine_echo: 0x2222_2222,
                opaque: vec![1; 45],
            })
            .unwrap()
            .encode()
            .unwrap(),
        ),
        1_120,
        &mut listener,
    );
    assert_eq!(
        (out.send[0].len(), out.send[1].len()),
        (31, 39),
        "observed Host9 and Host4 wires"
    );
    let out = host.receive(&client.app(&client5(0x5555_5555)), 1_130, &mut listener);
    assert_eq!(
        out.events,
        vec![Event::Application(App::Client5 { repeated: false })]
    );
    assert_eq!(
        out.send[0].len(),
        40,
        "observed 40-byte Host6 wire without sync"
    );
    assert_eq!(
        host.application().unwrap().selectors(),
        Some((HOST_SELECTOR, 7))
    );

    let frame = [0x5a; 29];
    let mut queued = crate::bits::BitWriter::new();
    queued
        .put(0, 1)
        .put(0, 1)
        .put(0, 16)
        .put(0, 6)
        .put(232, 15)
        .put(1, 1);
    queued.put_span(nfs_protocol::world::BitSpan::new(&frame, 0, 232).unwrap());
    let body_with =
        |number, ack| crate::application::sequenced(HOST_SELECTOR, number, ack, 0, &queued);
    let body = body_with(1, 0);
    let out = host.receive(&client.app(&body), 1_140, &mut listener);
    assert_eq!(
        out.events,
        vec![
            Event::Application(App::Queued {
                bits: 232,
                complete: true,
                accepted: false
            }),
            Event::Initialized
        ]
    );
    assert_eq!(
        out.send[0].len(),
        48,
        "observed 48-byte timing request wire"
    );
    let request = Timing::decode(&client.open(&out.send[0]).unwrap()).unwrap();
    assert_eq!((request.number(), request.acknowledgement()), (1, 1));
    assert_eq!(listener.frames, 1);
    assert_eq!(out.send[1].len(), 73, "46-byte body without sync");
    let body = client.open(&out.send[1]).unwrap();
    let e = nfs_protocol::world::envelope::ZeroTail::new(64)
        .unwrap()
        .decode(&body)
        .unwrap();
    assert_eq!(e.sequence().unwrap().number().value(), 2);
    let nfs_protocol::world::payload::Route::Queued { body: q, .. } =
        nfs_protocol::world::payload::Route::decode(e.payload()).unwrap()
    else {
        panic!()
    };
    let fragment = nfs_protocol::world::fragment::Fragment::decode(q).unwrap();
    let parsed = crate::frame::parse(fragment.data(), crate::frame::Direction::FromHost).unwrap();
    assert_eq!(
        parsed.time_sync,
        Some(crate::frame::TimeSync::Init(
            crate::frame::TimeSyncInit::DEFAULT
        ))
    );
    let m = parsed.messages.unwrap();
    assert_eq!(
        (m.groups[0].sequence, m.groups[0].messages.len()),
        (Some(0), 2)
    );
    assert!(matches!(
        m.groups[0].messages[0],
        crate::frame::Message::Time {
            flag: true,
            tick: 0,
            ..
        }
    ));
    let out = host.receive(&client.app(&body_with(2, 1)), 1_145, &mut listener);
    assert_eq!(out.send.len(), 0);
    assert!(!out.events.contains(&Event::Initialized));

    let sent = host
        .send_frame(
            nfs_protocol::world::BitSpan::new(&frame, 0, 232).unwrap(),
            1_150,
        )
        .unwrap();
    assert_eq!(sent.len(), 46 + 27, "absent-advertisement form on the wire");
    assert_eq!(client.open(&sent).unwrap().len(), 46);

    let out = host.poll(1_700);
    assert_eq!(
        out.send.len(),
        2,
        "idle sync plus an empty frame for the new input"
    );
    assert!(out.send.iter().any(|w| w.len() == 43));
    let out = host.poll(3_200);
    assert!(out.send.iter().any(|w| client.open(w).is_some()));

    let out = host.receive(&client.close(), 3_300, &mut listener);
    assert_eq!(out.events, vec![Event::Closed]);
    assert_eq!(host.stage(), StageName::Closed);
    assert!(host.application().is_some(), "stats survive the close");
    assert!(
        host.receive(&client.app(&[0; 4]), 3_400, &mut listener)
            .events
            .is_empty()
    );
    assert!(host.poll(5_000).send.is_empty());
}

#[test]
fn queued_content_follows_the_client_signals_and_inputs_get_empty_frames() {
    let mut host = host();
    let mut client = Client::new();
    let mut listener = AcceptSignals;
    assert_eq!(host.queue_content(85, &[0x11; 86]).unwrap(), 3);
    assert_eq!(host.queue_content(28, &[0x22; 252]).unwrap(), 8);
    assert_eq!(host.queue_content(28, &[0x33; 651]).unwrap(), 21);
    assert_eq!(host.content_progress(), (0, 32));
    host.receive(&client.request(), 1_000, &mut listener);
    host.receive(&client.confirm(), 1_010, &mut listener);
    host.receive(&client.sync(1_010), 1_020, &mut listener);
    host.receive(
        &client.app(&Request13::new([1, 2]).encode()),
        1_100,
        &mut listener,
    );
    host.receive(
        &client.app(&OpaqueAnswer16::new([1; 20], &[]).unwrap().encode()),
        1_110,
        &mut listener,
    );
    host.receive(
        &client.app(
            &Admission::new(Message::Client2 {
                peer_token: 0x4444_4444,
                engine_echo: 0x2222_2222,
                opaque: vec![1; 45],
            })
            .unwrap()
            .encode()
            .unwrap(),
        ),
        1_120,
        &mut listener,
    );
    host.receive(&client.app(&client5(0x5555_5555)), 1_130, &mut listener);
    assert!(host.poll(1_140).send.is_empty());
    let message_frame = |number: u16, index: u8, width: usize, value: u64| {
        let mut w = crate::bits::BitWriter::new();
        w.put(0b10, 6)
            .put(1, 4)
            .put(0, 1)
            .put(1, 1)
            .put(0, 3)
            .put(u64::from(number.saturating_sub(2) & 127), 7);
        w.put(u64::from(index), 7).put(value, width).put(0, 1);
        w.align();
        let mut queued = crate::bits::BitWriter::new();
        queued
            .put(0, 1)
            .put(0, 1)
            .put(u64::from(number), 16)
            .put(0, 6)
            .put(w.len() as u64, 15)
            .put(1, 1)
            .put_span(w.span());
        crate::application::sequenced(HOST_SELECTOR, number, 0, 0, &queued)
    };
    let frame = [0x5a; 29];
    let mut queued = crate::bits::BitWriter::new();
    queued
        .put(0, 1)
        .put(0, 1)
        .put(0, 16)
        .put(0, 6)
        .put(232, 15)
        .put(1, 1);
    queued.put_span(nfs_protocol::world::BitSpan::new(&frame, 0, 232).unwrap());
    let body = crate::application::sequenced(HOST_SELECTOR, 1, 0, 0, &queued);
    let out = host.receive(&client.app(&body), 1_150, &mut listener);
    assert!(out.events.contains(&Event::Initialized));
    let out = host.poll(2_200);
    assert!(
        !out.events
            .iter()
            .any(|e| matches!(e, Event::Content { .. }))
    );
    host.receive(
        &client.app(&message_frame(2, crate::frame::EX_CLIENT_STATE, 4, 3)),
        2_250,
        &mut listener,
    );
    let out = host.poll(2_300);
    assert_eq!(
        out.events,
        vec![Event::Content {
            chunks: 3,
            remaining: 29
        }]
    );
    let body = out.send.iter().find_map(|w| client.open(w)).unwrap();
    let e = nfs_protocol::world::envelope::ZeroTail::new(1600)
        .unwrap()
        .decode(&body)
        .unwrap();
    let nfs_protocol::world::payload::Route::Queued { body: q, .. } =
        nfs_protocol::world::payload::Route::decode(e.payload()).unwrap()
    else {
        panic!()
    };
    let fragment = nfs_protocol::world::fragment::Fragment::decode(q).unwrap();
    let parsed = crate::frame::parse(fragment.data(), crate::frame::Direction::FromHost).unwrap();
    let messages = parsed.messages.unwrap();
    assert_eq!(
        messages.state,
        Some(0),
        "the descriptor frame advertises state 0"
    );
    let group = &messages.groups[0];
    assert_eq!((group.channel, group.sequence), (0, Some(2)));
    assert_eq!(group.messages.len(), 3);
    assert!(matches!(
        group.messages[0],
        crate::frame::Message::Chunk {
            ordinal: 0,
            target: Some(85),
            ..
        }
    ));
    let out = host.poll(4_000);
    assert!(
        !out.events
            .iter()
            .any(|e| matches!(e, Event::Content { .. }))
    );
    host.receive(
        &client.app(&message_frame(3, crate::frame::EX_LEVEL_READY, 32, 7)),
        4_050,
        &mut listener,
    );
    let out = host.poll(4_100);
    assert_eq!(
        out.events,
        vec![Event::Content {
            chunks: 15,
            remaining: 14
        }]
    );
    let body = out.send.iter().find_map(|w| client.open(w)).unwrap();
    let e = nfs_protocol::world::envelope::ZeroTail::new(1600)
        .unwrap()
        .decode(&body)
        .unwrap();
    let nfs_protocol::world::payload::Route::Queued { body: q, .. } =
        nfs_protocol::world::payload::Route::decode(e.payload()).unwrap()
    else {
        panic!()
    };
    let fragment = nfs_protocol::world::fragment::Fragment::decode(q).unwrap();
    let burst = crate::frame::parse(fragment.data(), crate::frame::Direction::FromHost).unwrap();
    let burst = burst.messages.unwrap();
    assert_eq!(burst.state, None, "burst frames carry no state");
    assert_eq!(burst.groups[0].sequence, Some(5));
    let out = host.poll(4_150);
    assert_eq!(
        out.events,
        vec![Event::Content {
            chunks: 14,
            remaining: 0
        }]
    );
    assert_eq!(host.content_progress(), (32, 0));
    assert!(
        host.poll(4_200).send.is_empty(),
        "queue drained, no new inputs"
    );
    let mut sample = crate::bits::BitWriter::new();
    sample.put(0b1, 6).put(97_228, 38);
    sample.align();
    let mut queued = crate::bits::BitWriter::new();
    queued
        .put(0, 1)
        .put(0, 1)
        .put(4, 16)
        .put(0, 6)
        .put(sample.len() as u64, 15)
        .put(1, 1)
        .put_span(sample.span());
    let body = crate::application::sequenced(HOST_SELECTOR, 4, 0, 0, &queued);
    host.receive(&client.app(&body), 4_250, &mut listener);
    let out = host.poll(4_300);
    assert!(out.events.contains(&Event::TimeSynced));
    let body = out.send.iter().find_map(|w| client.open(w)).unwrap();
    let e = nfs_protocol::world::envelope::ZeroTail::new(64)
        .unwrap()
        .decode(&body)
        .unwrap();
    let nfs_protocol::world::payload::Route::Queued { body: q, .. } =
        nfs_protocol::world::payload::Route::decode(e.payload()).unwrap()
    else {
        panic!()
    };
    let fragment = nfs_protocol::world::fragment::Fragment::decode(q).unwrap();
    let times = crate::frame::parse(fragment.data(), crate::frame::Direction::FromHost).unwrap();
    let Some(crate::frame::TimeSync::Times([echo, received, sent])) = times.time_sync else {
        panic!("Times expected")
    };
    assert_eq!(echo, 97_228);
    assert_eq!((received, sent), (3_120 * 1024 / 1000, 3_170 * 1024 / 1000));
    let body = crate::application::sequenced(HOST_SELECTOR, 5, 1, 0, &queued);
    host.receive(&client.app(&body), 4_400, &mut listener);
    let out = host.poll(4_450);
    let empty = out.send.iter().find_map(|w| client.open(w)).unwrap();
    let e = nfs_protocol::world::envelope::ZeroTail::new(64)
        .unwrap()
        .decode(&empty)
        .unwrap();
    let nfs_protocol::world::payload::Route::Queued { body: q, .. } =
        nfs_protocol::world::payload::Route::decode(e.payload()).unwrap()
    else {
        panic!()
    };
    assert_eq!(
        nfs_protocol::world::fragment::Fragment::decode(q)
            .unwrap()
            .data()
            .len(),
        8,
        "8-bit empty frames"
    );
    assert!(host.poll(4_500).send.is_empty());
}

#[test]
fn bad_inputs_are_reported_without_ending_the_session() {
    let mut host = host();
    let mut client = Client::new();
    let mut listener = Retain::default();
    let out = host.receive(&[0; 30], 0, &mut listener);
    assert!(matches!(
        out.events.as_slice(),
        [Event::Ignored(Ignored::Transport(_))]
    ));
    host.receive(&client.request(), 1, &mut listener);
    host.receive(&client.confirm(), 2, &mut listener);
    host.receive(&client.sync(2), 3, &mut listener);
    let out = host.receive(&[0; 30], 4, &mut listener);
    assert!(matches!(out.events.as_slice(), [Event::Link(_)]));
    let out = host.receive(&client.app(&[0xff; 12]), 5, &mut listener);
    assert_eq!(
        out.events,
        vec![Event::ApplicationError(application::Error::Shape)]
    );
    assert_eq!(host.stage(), StageName::Running);
    let out = host.receive(
        &client.app(&Request13::new([1, 2]).encode()),
        6,
        &mut listener,
    );
    assert_eq!(out.send.len(), 1);
    assert_eq!(
        host.send_frame(nfs_protocol::world::BitSpan::new(&[0; 4], 0, 8).unwrap(), 7),
        Err(Error::Application(application::Error::Phase))
    );
}

fn established_with_content() -> (Host, Client, AcceptSignals, u64) {
    let mut host = host();
    let mut client = Client::new();
    let mut listener = AcceptSignals;
    assert_eq!(host.queue_content(85, &[0x11; 86]).unwrap(), 3);
    host.receive(&client.request(), 1_000, &mut listener);
    host.receive(&client.confirm(), 1_010, &mut listener);
    host.receive(&client.sync(1_010), 1_020, &mut listener);
    host.receive(
        &client.app(&Request13::new([1, 2]).encode()),
        1_100,
        &mut listener,
    );
    host.receive(
        &client.app(&OpaqueAnswer16::new([1; 20], &[]).unwrap().encode()),
        1_110,
        &mut listener,
    );
    host.receive(
        &client.app(
            &Admission::new(Message::Client2 {
                peer_token: 0x4444_4444,
                engine_echo: 0x2222_2222,
                opaque: vec![1; 45],
            })
            .unwrap()
            .encode()
            .unwrap(),
        ),
        1_120,
        &mut listener,
    );
    host.receive(&client.app(&client5(0x5555_5555)), 1_130, &mut listener);
    let frame = [0x5a; 29];
    let mut queued = crate::bits::BitWriter::new();
    queued
        .put(0, 1)
        .put(0, 1)
        .put(0, 16)
        .put(0, 6)
        .put(232, 15)
        .put(1, 1);
    queued.put_span(nfs_protocol::world::BitSpan::new(&frame, 0, 232).unwrap());
    let body = crate::application::sequenced(HOST_SELECTOR, 1, 0, 0, &queued);
    let out = host.receive(&client.app(&body), 1_150, &mut listener);
    assert!(out.events.contains(&Event::Initialized));
    host.receive(
        &client.app(&message_frame(2, crate::frame::EX_STATE, 4, 0)),
        2_250,
        &mut listener,
    );
    let out = host.poll(2_300);
    assert_eq!(
        out.events,
        vec![Event::Content {
            chunks: 3,
            remaining: 0
        }]
    );
    assert_eq!(host.content_progress(), (3, 0));
    (host, client, listener, 2_300)
}

fn message_frame(number: u16, index: u8, width: usize, value: u64) -> Vec<u8> {
    let mut w = crate::bits::BitWriter::new();
    w.put(0b10, 6)
        .put(1, 4)
        .put(0, 1)
        .put(1, 1)
        .put(0, 3)
        .put(u64::from(number.saturating_sub(2) & 127), 7);
    w.put(u64::from(index), 7).put(value, width).put(0, 1);
    w.align();
    let mut queued = crate::bits::BitWriter::new();
    queued
        .put(0, 1)
        .put(0, 1)
        .put(u64::from(number), 16)
        .put(0, 6)
        .put(w.len() as u64, 15)
        .put(1, 1)
        .put_span(w.span());
    crate::application::sequenced(HOST_SELECTOR, number, 0, 0, &queued)
}

fn host_frames(client: &Client, wires: &[Vec<u8>]) -> Vec<(Vec<u8>, usize)> {
    let mut assembler = nfs_protocol::world::fragment::Assembler::new(1 << 20).unwrap();
    let mut frames = Vec::new();
    for wire in wires {
        let Some(body) = client.open(wire) else {
            continue;
        };
        let e = nfs_protocol::world::envelope::ZeroTail::new(1600)
            .unwrap()
            .decode(&body)
            .unwrap();
        let Ok(nfs_protocol::world::payload::Route::Queued { body: q, .. }) =
            nfs_protocol::world::payload::Route::decode(e.payload())
        else {
            continue;
        };
        let fragment = nfs_protocol::world::fragment::Fragment::decode(q).unwrap();
        if let Ok(nfs_protocol::world::fragment::Outcome::Complete(f)) = assembler.push(fragment) {
            frames.push((f.data().bytes().to_vec(), f.data().len()));
        }
    }
    frames
}

#[test]
fn state_exchange_does_not_require_queued_replication() {
    let (mut host, mut client, mut listener, now) = established_with_content();
    assert!(host.poll(now + REPORT_TIMEOUT_MS - 1).events.is_empty());
    let t = loading::exchange(
        &mut host,
        &mut client,
        &mut listener,
        now + REPORT_TIMEOUT_MS,
    );
    assert!(host.poll(t).events.is_empty());
}

fn collection_bytes(handles: &[u16]) -> Vec<u8> {
    let mut w = crate::bits::BitWriter::new();
    w.put(6, 34).put(handles.len() as u64, 32);
    for h in handles {
        w.put(u64::from(*h), 16)
            .put(u64::from(*h) + 1, 16)
            .put(0, 16)
            .put(u64::from(*h) + 4, 32)
            .put(254, 8)
            .put(0, 1)
            .put(3, 3)
            .put(0, 32)
            .put(1, 1)
            .put(0, 2)
            .put(0, 32)
            .put(0, 10);
    }
    w.align();
    w.into_bytes()
}

fn report_frame(number: u16, id: u16) -> Vec<u8> {
    let mut w = crate::bits::BitWriter::new();
    w.put(0b10, 6)
        .put(1, 4)
        .put(0, 1)
        .put(1, 1)
        .put(0, 3)
        .put(u64::from(number.saturating_sub(2) & 127), 7);
    w.put(u64::from(crate::frame::EX_SUBLEVEL_REPORT), 7)
        .put(1, 32)
        .put(1, 32)
        .put(u64::from(id), 16)
        .put(0, 1);
    w.align();
    let mut queued = crate::bits::BitWriter::new();
    queued
        .put(0, 1)
        .put(0, 1)
        .put(u64::from(number), 16)
        .put(0, 6)
        .put(w.len() as u64, 15)
        .put(1, 1)
        .put_span(w.span());
    crate::application::sequenced(HOST_SELECTOR, number, 0, 0, &queued)
}

#[test]
fn replication_starts_when_the_reports_cover_the_registered_handles() {
    let mut host = host();
    assert_eq!(
        host.queue_content(
            crate::frame::EX_COLLECTION,
            &collection_bytes(&[0, 1, 2, 3])
        )
        .unwrap(),
        3,
        "93 bytes: three chunks"
    );
    assert_eq!(host.registration_progress(), (4, 0));
    assert!(
        host.queue_content(crate::frame::EX_COLLECTION, &[0xff; 20])
            .is_err(),
        "a collection that does not parse is rejected"
    );
    let mut client = Client::new();
    let mut listener = AcceptSignals;
    host.receive(&client.request(), 1_000, &mut listener);
    host.receive(&client.confirm(), 1_010, &mut listener);
    host.receive(&client.sync(1_010), 1_020, &mut listener);
    host.receive(
        &client.app(&Request13::new([1, 2]).encode()),
        1_100,
        &mut listener,
    );
    host.receive(
        &client.app(&OpaqueAnswer16::new([1; 20], &[]).unwrap().encode()),
        1_110,
        &mut listener,
    );
    host.receive(
        &client.app(
            &Admission::new(Message::Client2 {
                peer_token: 0x4444_4444,
                engine_echo: 0x2222_2222,
                opaque: vec![1; 45],
            })
            .unwrap()
            .encode()
            .unwrap(),
        ),
        1_120,
        &mut listener,
    );
    host.receive(&client.app(&client5(0x5555_5555)), 1_130, &mut listener);
    let frame = [0x5a; 29];
    let mut queued = crate::bits::BitWriter::new();
    queued
        .put(0, 1)
        .put(0, 1)
        .put(0, 16)
        .put(0, 6)
        .put(232, 15)
        .put(1, 1);
    queued.put_span(nfs_protocol::world::BitSpan::new(&frame, 0, 232).unwrap());
    let body = crate::application::sequenced(HOST_SELECTOR, 1, 0, 0, &queued);
    host.receive(&client.app(&body), 1_150, &mut listener);
    host.receive(
        &client.app(&message_frame(2, crate::frame::EX_STATE, 4, 0)),
        2_250,
        &mut listener,
    );
    assert!(
        host.poll(2_300)
            .events
            .iter()
            .any(|e| matches!(e, Event::Content { .. })),
        "the collection goes out"
    );
    for (i, id) in [1u16, 2, 3].iter().enumerate() {
        host.receive(
            &client.app(&report_frame(3 + i as u16, *id)),
            2_400 + 100 * i as u64,
            &mut listener,
        );
    }
    assert_eq!(host.registration_progress(), (4, 3));
    assert!(
        !host
            .poll(2_700)
            .events
            .iter()
            .any(|e| matches!(e, Event::StateSent { .. }))
    );
    host.receive(&client.app(&report_frame(6, 4)), 2_800, &mut listener);
    assert!(
        host.poll(2_850).events.is_empty(),
        "coverage alone is insufficient"
    );
    host.receive(
        &client.app(&loading::acknowledge_level(7, 5)),
        2_851,
        &mut listener,
    );
    let out = host.poll(2_852);
    assert_eq!(
        out.events,
        vec![Event::StateSent {
            registered: 4,
            reported: 4
        }],
        "coverage and accepted level acknowledgement permit state 7"
    );
    assert_eq!(host.registration_progress(), (4, 4));
}

fn movement_grants(client: &Client, wires: &[Vec<u8>]) -> Vec<Vec<(u16, u32)>> {
    host_frames(client, wires)
        .into_iter()
        .filter_map(|(bytes, len)| {
            let span = nfs_protocol::world::BitSpan::new(&bytes, 0, len).unwrap();
            let parsed = crate::frame::parse(span, crate::frame::Direction::FromHost).unwrap();
            parsed.movement.map(|m| {
                m.records
                    .iter()
                    .map(|r| (r.id, r.data.read_u32(0, r.data.len() as u8).unwrap()))
                    .collect()
            })
        })
        .collect()
}

#[test]
fn movement_grants_repeat_at_the_official_cadence_for_sent_objects() {
    let (mut host, client, _listener, now, sent) = entry_setup();
    assert!(movement_grants(&client, &sent.send).is_empty());
    // Object 1 (the Player) was created on the wire; 99 never was.
    host.set_movement([99, 1].into_iter().collect()).unwrap();
    let first = host.poll(now + 10);
    assert_eq!(movement_grants(&client, &first.send), vec![vec![(1, 0)]]);
    assert!(
        movement_grants(
            &client,
            &host.poll(now + 10 + MOVEMENT_INTERVAL_MS - 1).send
        )
        .is_empty()
    );
    assert_eq!(
        movement_grants(&client, &host.poll(now + 10 + MOVEMENT_INTERVAL_MS).send),
        vec![vec![(1, 0)]]
    );
    // Only unsent objects remain: nothing is listed.
    host.set_movement([99].into_iter().collect()).unwrap();
    assert!(
        movement_grants(
            &client,
            &host.poll(now + 10 + 2 * MOVEMENT_INTERVAL_MS).send
        )
        .is_empty()
    );
    host.set_movement(Default::default()).unwrap();
    assert!(
        movement_grants(
            &client,
            &host.poll(now + 10 + 3 * MOVEMENT_INTERVAL_MS).send
        )
        .is_empty()
    );
    assert_eq!(
        host.set_movement((0..=crate::frame::MAX_MOVEMENT_RECORDS as u16).collect()),
        Err(Error::Application(application::Error::Bound))
    );
}

#[test]
fn replication_clock_advances_without_object_changes_and_is_session_owned() {
    let (mut host, client, _, now, initial) = entry_setup();
    let (mut other, mut other_client, mut other_listener, _, _) = entry_setup();
    let origin = host.application().unwrap().origin_ms().unwrap();
    let clock = |out: &Output| {
        host_frames(&client, &out.send)
            .into_iter()
            .filter_map(|(bytes, bits)| {
                let parsed = frame::parse(
                    BitSpan::new(&bytes, 0, bits).unwrap(),
                    frame::Direction::FromHost,
                )
                .unwrap();
                parsed.ghost.map(|g| {
                    (
                        f32::from_bits(g.prefix.float_bits().unwrap()),
                        g.prefix.remaining_records(),
                        g.prefix.flag(),
                    )
                })
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        clock(&initial),
        vec![((now - origin) as f32 / 1000.0, 1, true)]
    );
    assert!(clock(&host.poll(now + 99)).is_empty());
    let heartbeat = host.poll(now + 100);
    assert_eq!(
        clock(&heartbeat),
        vec![((now + 100 - origin) as f32 / 1000.0, 0, true)]
    );
    assert_eq!(host.replication_progress(), (1, 0));
    assert_eq!(other.replication_clock_ms, Some(now));
    assert!(clock(&host.poll(now + 101)).is_empty());
    assert_eq!(
        clock(&host.poll(now + 200)),
        vec![((now + 200 - origin) as f32 / 1000.0, 0, true)]
    );
    // Caller timestamps cannot leak another session's clock or bypass bounds.
    let mut delta = owned_section(1);
    delta.records[0].initial = None;
    delta.float_bits = Some(1_000_000.0f32.to_bits());
    host.queue_replication(delta).unwrap();
    assert_eq!(
        clock(&host.poll(now + 250)),
        vec![((now + 250 - origin) as f32 / 1000.0, 1, true)]
    );
    other.receive(&other_client.close(), now + 300, &mut other_listener);
    assert!(other.poll(now + 300).send.is_empty());
}

#[test]
fn clock_header_reservation_and_backpressure_leave_state_unchanged() {
    let (mut host, _, _, _) = established_with_content();
    let section = owned_section(1);
    host.max_replication_frame_bits = section.frame().unwrap().len();
    assert_eq!(
        host.queue_replication(section),
        Err(Error::Application(application::Error::Bound))
    );
    assert_eq!(host.replication_progress(), (0, 0));
    assert!(host.replication_bindings.is_empty());
    let (mut host, _, _, now, _) = entry_setup();
    let Stage::Running { application, .. } = &mut host.stage else {
        panic!()
    };
    let empty = frame::Builder::new().build().unwrap();
    for _ in 0..64 {
        if application.send_frame(empty.span()) == Err(application::Error::Window) {
            break;
        }
    }
    let mut out = Output::default();
    host.replication_clock(now + 100, &mut out);
    assert!(out.send.is_empty());
    assert!(out.events.is_empty());
    assert_eq!(host.replication_clock_ms, Some(now));
}

#[test]
fn time_sample_survives_window_pressure_and_sends_once_after_real_ack() {
    let (mut host, mut client, mut listener, now, _) = entry_setup();
    let sample = 97_228;
    let mut frame = crate::bits::BitWriter::new();
    frame.put(1, 6).put(sample, 38).align();
    let mut queued = crate::bits::BitWriter::new();
    queued
        .put(0, 1)
        .put(0, 1)
        .put(4, 16)
        .put(0, 6)
        .put(frame.len() as u64, 15)
        .put(1, 1)
        .put_span(frame.span());
    let body = application::sequenced(HOST_SELECTOR, 3, 0, 0, &queued);
    let received_at = now + 1;
    host.receive(&client.app(&body), received_at, &mut listener);
    let Stage::Running {
        application, world, ..
    } = &mut host.stage
    else {
        panic!()
    };
    assert_eq!(world.time_sample, Some((sample, received_at)));
    let origin = application.origin_ms().unwrap();
    let empty = crate::frame::Builder::new().build().unwrap();
    let mut latest = None;
    loop {
        match application.send_frame(empty.span()) {
            Ok(body) => {
                latest = nfs_protocol::world::envelope::ZeroTail::new(1600)
                    .unwrap()
                    .decode(&body)
                    .unwrap()
                    .sequence()
                    .map(|s| s.number().value())
            }
            Err(application::Error::Window) => break,
            other => panic!("{other:?}"),
        }
    }
    for offset in 2..=4 {
        let out = host.poll(now + offset);
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::TimeSynced | Event::ApplicationError(_)))
        );
        let Stage::Running { world, .. } = &host.stage else {
            panic!()
        };
        assert_eq!(world.time_sample, Some((sample, received_at)));
    }
    let mut control = crate::bits::BitWriter::new();
    control.put(1, 1).put(1, 4);
    let body = application::sequenced(HOST_SELECTOR, 4, latest.unwrap(), u32::MAX, &control);
    host.receive(&client.app(&body), now + 5, &mut listener);
    let sent_at = now + 6;
    let out = host.poll(sent_at);
    assert_eq!(
        out.events
            .iter()
            .filter(|e| **e == Event::TimeSynced)
            .count(),
        1
    );
    let times = host_frames(&client, &out.send)
        .into_iter()
        .filter_map(|(bytes, bits)| {
            crate::frame::parse(
                BitSpan::new(&bytes, 0, bits).unwrap(),
                crate::frame::Direction::FromHost,
            )
            .unwrap()
            .time_sync
        })
        .collect::<Vec<_>>();
    assert_eq!(
        times,
        vec![crate::frame::TimeSync::Times([
            sample,
            (received_at - origin) * 1024 / 1000,
            (sent_at - origin) * 1024 / 1000,
        ])]
    );
    let Stage::Running { world, .. } = &host.stage else {
        panic!()
    };
    assert_eq!(world.time_sample, None);
    assert!(!host.poll(now + 7).events.contains(&Event::TimeSynced));
    assert_eq!(host.failures(), Failures::default());
}

#[test]
fn deferred_rpc_window_pressure_retains_order_and_sends_once_after_real_ack() {
    use crate::replication::{
        Record,
        sublevel::{Rpc, root},
    };
    use nfs_protocol::world::rpc::Serial;
    let (mut host, mut client, mut listener, now) = established_with_content();
    let content = root::content().unwrap();
    let creation = root::creation(
        Rpc {
            selector: 0,
            serial: Serial::new(0).unwrap(),
        },
        [None; root::REFERENCES],
    )
    .unwrap();
    let mut section = owned_section(1);
    section.records = vec![Record::from_scene(1, creation, &content).unwrap()];
    host.queue_replication(section).unwrap();
    let now = now + REPORT_TIMEOUT_MS;
    loading::after_exchange(&mut host, now);
    let now = now + 2 * STATE_WAIT_MS;
    assert!(
        host.poll(now)
            .events
            .iter()
            .any(|event| matches!(event, Event::Replication { records: 1, .. }))
    );
    let rpc = crate::participants::HostRpc::Event(crate::logic::Fire {
        target: crate::logic::EntityRef {
            ghost: 1,
            entity: 1,
        },
        event: 123,
        player: None,
    });
    host.queue_rpc(rpc).unwrap();
    let Stage::Running {
        application, world, ..
    } = &mut host.stage
    else {
        panic!()
    };
    let sequence = world.next_ex_sequence;
    let empty = frame::Builder::new().build().unwrap();
    let mut latest = None;
    loop {
        match application.send_frame(empty.span()) {
            Ok(body) => {
                latest = nfs_protocol::world::envelope::ZeroTail::new(1600)
                    .unwrap()
                    .decode(&body)
                    .unwrap()
                    .sequence()
                    .map(|s| s.number().value())
            }
            Err(application::Error::Window) => break,
            other => panic!("{other:?}"),
        }
    }
    let now = now + REPLICATION_INTERVAL_MS;
    let out = host.poll(now);
    assert!(!out.events.iter().any(|event| matches!(
        event,
        Event::RpcNotifications { .. } | Event::ApplicationError(_)
    )));
    assert_eq!(host.rpc_progress(), (0, 1));
    let Stage::Running { world, .. } = &host.stage else {
        panic!()
    };
    assert_eq!(world.next_ex_sequence, sequence);
    let mut control = crate::bits::BitWriter::new();
    control.put(1, 1).put(1, 4);
    let body = application::sequenced(HOST_SELECTOR, 3, latest.unwrap(), u32::MAX, &control);
    host.receive(&client.app(&body), now + 1, &mut listener);
    let out = host.poll(now + 2);
    assert!(out.events.contains(&Event::RpcNotifications {
        count: 1,
        remaining: 0
    }));
    let mut messages = vec![];
    for (bytes, bits) in host_frames(&client, &out.send) {
        let parsed = frame::parse(
            BitSpan::new(&bytes, 0, bits).unwrap(),
            frame::Direction::FromHost,
        )
        .unwrap();
        if let Some(groups) = parsed.messages {
            for group in groups.groups {
                for message in group.messages {
                    if let frame::Message::Opaque { index: 86, body } = message {
                        let mut owned = crate::bits::BitWriter::new();
                        owned.put_span(body);
                        messages.push((group.sequence, owned));
                    }
                }
            }
        }
    }
    assert_eq!(messages, vec![(Some(sequence), rpc.encode().unwrap())]);
    assert_eq!(host.rpc_progress(), (1, 0));
    assert!(
        !host
            .poll(now + REPLICATION_INTERVAL_MS + 3)
            .events
            .iter()
            .any(|event| matches!(event, Event::RpcNotifications { .. }))
    );
    assert_eq!(host.failures(), Failures::default());
}

#[test]
fn timed_split_keeps_full_record_batches_inside_session_frame_bound() {
    use crate::replication::section::{split_records, split_timed_records};
    let records = owned_section(2).records;
    let plain = split_records(records.clone(), application::OUTBOUND_FRAME_BITS).unwrap();
    assert_eq!(plain.len(), 1);
    let bound = plain[0].frame().unwrap().len();
    let (mut host, _, _, _) = established_with_content();
    host.max_replication_frame_bits = bound;
    // This previously passed the splitter, then failed during live startup.
    assert_eq!(
        host.queue_replication(plain[0].clone()),
        Err(Error::Application(application::Error::Bound))
    );
    let timed = split_timed_records(records.clone(), bound).unwrap();
    assert_eq!(timed.len(), 2);
    assert_eq!(
        timed
            .iter()
            .flat_map(|s| s.records.clone())
            .collect::<Vec<_>>(),
        records
    );
    for section in timed {
        assert!(section.frame().unwrap().len() <= bound);
        assert_eq!(section.float_bits, Some(0));
        assert!(section.flag);
        host.queue_replication(section).unwrap();
    }
    assert_eq!(host.replication_progress(), (0, 2));
    let one = owned_section(1);
    let too_small = one.frame().unwrap().len();
    assert_eq!(
        split_timed_records(one.records, too_small),
        Err(crate::replication::Error::Bound)
    );
}

#[test]
fn queued_notifications_for_sent_objects_ride_ahead_of_the_next_replication_frame() {
    use crate::{
        launchers::Enabled,
        replication::{
            Record,
            sublevel::{Rpc, root},
        },
    };
    use nfs_protocol::world::rpc::Serial;
    let (mut host, client, _, now) = established_with_content();
    let content = root::content().unwrap();
    let scene = |id| {
        let creation = root::creation(
            Rpc {
                selector: 0,
                serial: Serial::new(0).unwrap(),
            },
            [None; root::REFERENCES],
        )
        .unwrap();
        let mut section = owned_section(1);
        section.records = vec![Record::from_scene(id, creation, &content).unwrap()];
        section
    };
    let enabled = |scene| Enabled {
        scene,
        selector: 0,
        serial: Serial::new(0).unwrap(),
    };
    host.queue_replication(scene(1)).unwrap();
    let t = now + REPORT_TIMEOUT_MS;
    loading::after_exchange(&mut host, t);
    let t = t + 2 * STATE_WAIT_MS;
    assert!(matches!(
        host.poll(t).events[..],
        [Event::Replication { .. }]
    ));
    assert!(host.sent_objects.contains(&1));
    // Three sections wait in the replication queue (as the movement-driven
    // stream does in free roam); scene 2 is created by the first of them.
    host.queue_replication(scene(2)).unwrap();
    host.queue_replication(scene(3)).unwrap();
    host.queue_replication(scene(4)).unwrap();
    // A notification on the sent scene rides with the next frame; one on the
    // unsent scene 2 waits for its creation, and keeps the one behind it in
    // order, even under a continuous replication stream.
    host.queue_launcher_enabled(enabled(1)).unwrap();
    host.queue_launcher_enabled(enabled(2)).unwrap();
    host.queue_launcher_enabled(enabled(1)).unwrap();
    let parse_notifications = |out: &Output| {
        let frames = host_frames(&client, &out.send);
        let parsed = frame::parse(
            BitSpan::new(&frames[0].0, 0, frames[0].1).unwrap(),
            frame::Direction::FromHost,
        )
        .unwrap();
        parsed
            .messages
            .as_ref()
            .map_or(0, |m| m.groups[0].messages.len())
    };
    let out = host.poll(t + REPLICATION_INTERVAL_MS);
    assert_eq!(
        out.events,
        vec![
            Event::Replication {
                records: 1,
                fragments: 1,
                remaining: 2
            },
            Event::RpcNotifications {
                count: 1,
                remaining: 2
            }
        ]
    );
    assert_eq!(parse_notifications(&out), 1);
    assert_eq!(host.rpc_queue.len(), 2);
    let out = host.poll(t + 2 * REPLICATION_INTERVAL_MS);
    assert_eq!(
        out.events,
        vec![
            Event::Replication {
                records: 1,
                fragments: 1,
                remaining: 1
            },
            Event::RpcNotifications {
                count: 2,
                remaining: 0
            }
        ]
    );
    assert_eq!(parse_notifications(&out), 2);
    assert!(host.rpc_queue.is_empty());
    let out = host.poll(t + 3 * REPLICATION_INTERVAL_MS);
    assert_eq!(
        out.events,
        vec![Event::Replication {
            records: 1,
            fragments: 1,
            remaining: 0
        }]
    );
    assert_eq!(parse_notifications(&out), 0);
    // Without queued replication the RPC-only frame still serves the queue.
    host.queue_launcher_enabled(enabled(2)).unwrap();
    assert_eq!(
        host.poll(t + 4 * REPLICATION_INTERVAL_MS).events,
        vec![Event::RpcNotifications {
            count: 1,
            remaining: 0
        }]
    );
}

#[test]
fn ordered_content_cannot_overtake_notifications_under_a_full_send_window() {
    ordered_content_barrier(47, &[0x88; 50]);
}

#[test]
fn a_multiframe_publication_keeps_rpc_order_across_early_creations_and_queue_pressure() {
    use crate::{
        participants::HostRpc,
        replication::{
            Record,
            sublevel::{Rpc, root},
        },
    };
    use nfs_protocol::world::rpc::Serial;
    let (mut host, mut client, mut listener, now) = established_with_content();
    let content = root::content().unwrap();
    let scene = |id| {
        let creation = root::creation(
            Rpc {
                selector: 0,
                serial: Serial::new(0).unwrap(),
            },
            [None; root::REFERENCES],
        )
        .unwrap();
        let mut section = owned_section(1);
        section.records = vec![Record::from_scene(id, creation, &content).unwrap()];
        section
    };
    host.queue_replication(scene(1)).unwrap();
    let t = now + REPORT_TIMEOUT_MS;
    loading::after_exchange(&mut host, t);
    let t = t + 2 * STATE_WAIT_MS;
    host.poll(t);
    assert!(host.sent_objects.contains(&1));
    host.queue_replication(scene(2)).unwrap();
    let calls: Vec<_> = (0..MAX_RPC_QUEUE + 1)
        .map(|index| {
            HostRpc::Event(crate::logic::Fire {
                target: crate::logic::EntityRef {
                    ghost: if index == 20 { 2 } else { 1 },
                    entity: 1,
                },
                event: index as u32,
                player: None,
            })
        })
        .collect();
    for &call in &calls[..MAX_RPC_QUEUE] {
        host.queue_rpc(call).unwrap();
    }
    assert_eq!(
        host.queue_rpc(calls[MAX_RPC_QUEUE]),
        Err(Error::Application(application::Error::Window))
    );
    let mut decoded = Vec::new();
    for pass in 1..=calls.len().div_ceil(MAX_EAGER_NOTIFICATIONS) as u64 {
        let out = host.poll(t + pass * REPLICATION_INTERVAL_MS);
        for (bytes, bits) in host_frames(&client, &out.send) {
            let parsed = frame::parse(
                BitSpan::new(&bytes, 0, bits).unwrap(),
                frame::Direction::FromHost,
            )
            .unwrap();
            if let Some(messages) = parsed.messages {
                for group in messages.groups {
                    for message in group.messages {
                        if let frame::Message::Opaque { index: 86, body } = message {
                            // The parsed span borrows the complete frame at a
                            // bit offset; compare only this message's own bits.
                            let mut owned = crate::bits::BitWriter::new();
                            owned.put_span(body);
                            decoded.push((owned.bytes().to_vec(), owned.len()));
                        }
                    }
                }
            }
        }
        if pass == 1 {
            // The early creation rides with the first prefix, while its
            // callback stays at its causal position in the later prefix.
            assert!(host.sent_objects.contains(&2));
            assert_eq!(decoded.len(), MAX_EAGER_NOTIFICATIONS);
            host.queue_rpc(calls[MAX_RPC_QUEUE]).unwrap();
        }
        // Acknowledge the actual transport output so the later mode-callback
        // positions and the final retried call pass through serialization too.
        let last = out
            .send
            .iter()
            .filter_map(|wire| client.open(wire))
            .filter_map(|body| {
                nfs_protocol::world::envelope::ZeroTail::new(1600)
                    .unwrap()
                    .decode(&body)
                    .unwrap()
                    .sequence()
                    .map(|s| s.number().value())
            })
            .next_back()
            .expect("a published frame");
        let mut control = crate::bits::BitWriter::new();
        control.put(1, 1).put(1, 4);
        let ack = application::sequenced(HOST_SELECTOR, 3 + pass as u16, last, u32::MAX, &control);
        let receipt = host.receive(
            &client.app(&ack),
            t + pass * REPLICATION_INTERVAL_MS + 1,
            &mut listener,
        );
        assert!(
            !receipt
                .events
                .iter()
                .any(|event| matches!(event, Event::ApplicationError(_) | Event::Closed))
        );
    }
    assert_eq!(decoded.len(), calls.len());
    let expected: Vec<_> = calls
        .iter()
        .map(|call| {
            let wire = call.encode().unwrap();
            (wire.bytes().to_vec(), wire.len())
        })
        .collect();
    for (index, (actual, expected)) in decoded.iter().zip(&expected).enumerate() {
        assert_eq!(actual, expected, "publication RPC {index}");
    }
    assert!(host.rpc_queue.is_empty());
}

#[test]
fn speedwall_progress_keeps_its_reliable_finish_boundary_under_pressure() {
    let progress = crate::speedwall::Progress {
        score_bits: 0,
        time_ms_bits: 123_000f32.to_bits(),
        rep_bits: 9000f32.to_bits(),
        position: 2,
        event_id: 0xf123_4567,
        vehicle: b"ID_CAR_TEST".to_vec(),
    };
    ordered_content_barrier(
        crate::speedwall::CONTENT_TARGET,
        &progress.encode().unwrap(),
    );
}

fn ordered_content_barrier(target: u8, payload: &[u8]) {
    use crate::{
        logic::{EntityRef, Fire},
        participants::HostRpc,
        replication::{
            Record,
            sublevel::{Rpc, root},
        },
    };
    use nfs_protocol::world::rpc::Serial;
    let (mut host, mut client, mut listener, now) = established_with_content();
    let content = root::content().unwrap();
    let creation = root::creation(
        Rpc {
            selector: 0,
            serial: Serial::new(0).unwrap(),
        },
        [None; root::REFERENCES],
    )
    .unwrap();
    let mut initial = owned_section(0);
    initial
        .records
        .push(Record::from_scene(1, creation, &content).unwrap());
    host.queue_replication(initial).unwrap();
    let t = now + REPORT_TIMEOUT_MS;
    loading::after_exchange(&mut host, t);
    let t = t + 2 * STATE_WAIT_MS;
    host.poll(t);
    assert_eq!(host.replication_progress(), (1, 0));
    let event = |event| {
        HostRpc::Event(Fire {
            target: EntityRef {
                ghost: 1,
                entity: 1,
            },
            event,
            player: None,
        })
    };
    let empty_section = || Section {
        float_bits: None,
        flag: false,
        deleted: vec![],
        setup: None,
        records: vec![],
    };
    host.queue_replication_after(empty_section(), vec![event(3), event(4)])
        .unwrap();
    host.queue_rpc(event(5)).unwrap();
    assert_eq!(
        host.queue_content_after_replication(target, payload)
            .unwrap(),
        2
    );
    // Work queued after the command must not move its publication boundary.
    for _ in 0..4 {
        host.queue_replication(empty_section()).unwrap();
    }
    let Stage::Running {
        application, world, ..
    } = &mut host.stage
    else {
        panic!()
    };
    world.client_level_ready = true;
    let empty = frame::Builder::new().build().unwrap();
    let mut latest = None;
    loop {
        match application.send_frame(empty.span()) {
            Ok(body) => {
                latest = nfs_protocol::world::envelope::ZeroTail::new(1600)
                    .unwrap()
                    .decode(&body)
                    .unwrap()
                    .sequence()
                    .map(|s| s.number().value())
            }
            Err(application::Error::Window) => break,
            other => panic!("{other:?}"),
        }
    }
    let out = host.poll(t + 500);
    assert!(!out.events.iter().any(|e| matches!(
        e,
        Event::Content { .. } | Event::Replication { .. } | Event::RpcNotifications { .. }
    )));
    assert_eq!(host.content_progress(), (3, 2));
    assert_eq!(host.replication_progress(), (1, 5));
    let mut control = crate::bits::BitWriter::new();
    control.put(1, 1).put(1, 4);
    let body = application::sequenced(HOST_SELECTOR, 3, latest.unwrap(), u32::MAX, &control);
    host.receive(&client.app(&body), t + 501, &mut listener);
    let mut order = Vec::new();
    let mut content_chunks: Vec<frame::Message<'static>> = Vec::new();
    for step in 1..=4 {
        let out = host.poll(t + 501 + step * REPLICATION_INTERVAL_MS);
        for (bytes, bits) in host_frames(&client, &out.send) {
            let parsed = frame::parse(
                BitSpan::new(&bytes, 0, bits).unwrap(),
                frame::Direction::FromHost,
            )
            .unwrap();
            if let Some(messages) = parsed.messages {
                for group in messages.groups {
                    for message in group.messages {
                        match message {
                            frame::Message::Opaque { index: 86, .. } => order.push("state"),
                            frame::Message::Chunk {
                                ordinal,
                                target,
                                last,
                                bytes,
                            } => {
                                order.push("teleport");
                                content_chunks.push(frame::Message::Chunk {
                                    ordinal,
                                    target,
                                    last,
                                    bytes,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if host.content_progress().1 == 0 {
            assert!(
                host.replication_progress().1 > 0,
                "later work cannot starve the command"
            );
            break;
        }
    }
    assert_eq!(order, ["state", "state", "state", "teleport", "teleport"]);
    assert_eq!(host.content_progress(), (5, 0));
    let references = content_chunks.iter().collect::<Vec<_>>();
    assert_eq!(
        frame::reassemble(&references),
        Some((target, payload.to_vec()))
    );
}

#[test]
fn queued_replication_bits_measure_what_the_host_admits() {
    // Sized without the host's timestamp, a section can fit the frame and
    // still be refused once queued.
    let section = owned_section(2);
    let raw = replication_frame(&section, &[], 0).unwrap().len();
    let queued = queued_replication_bits(&section, &[]).unwrap();
    assert!(queued > raw, "{queued} <= {raw}");
    let mut host = host();
    host.max_replication_frame_bits = queued - 1;
    assert!(matches!(
        host.queue_replication(section.clone()),
        Err(Error::Application(application::Error::Bound))
    ));
    assert_eq!(host.replication_progress(), (0, 0));
    host.max_replication_frame_bits = queued;
    host.queue_replication(section).unwrap();
    assert_eq!(host.replication_bits, queued);
    const { assert!(MAX_REPLICATION_FRAME_BITS <= application::OUTBOUND_FRAME_BITS) };
}

mod loading;
