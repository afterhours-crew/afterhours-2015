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
    host.poll(now);
    host.poll(now + STATE_WAIT_MS);
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
    host.poll(now + REPLICATION_INTERVAL_MS);
    let Stage::Running { world, .. } = &host.stage else {
        panic!()
    };
    assert!(matches!(
        world.player_entry,
        entry::Entry::Ready { player: 1 }
    ));
    assert_eq!(host.application().unwrap().delivery_status(ticket), None);
    let t = now + REPLICATION_INTERVAL_MS + 1;
    host.receive(
        &client.app(&message_frame(4, frame::EX_STATE, 4, 1)),
        t,
        &mut listener,
    );
    assert!(
        !host
            .poll(t + 1)
            .events
            .iter()
            .any(|e| matches!(e, Event::FirstPlayerEntered { .. }))
    );
    host.receive(
        &client.app(&message_frame(5, frame::EX_STATE, 4, 0)),
        t + 2,
        &mut listener,
    );
    let out = host.poll(t + 3);
    assert_eq!(out.events, vec![Event::FirstPlayerEntered { player: 1 }]);
    let decoded = host_frames(&client, &out.send);
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
        &client.app(&message_frame(6, frame::EX_STATE, 4, 1)),
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
    host.receive(
        &client.app(&message_frame(4, frame::EX_STATE, 4, 1)),
        now + 2,
        &mut listener,
    );
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
    host.receive(
        &client.app(&message_frame(4, frame::EX_STATE, 4, 1)),
        now + 2,
        &mut listener,
    );
    host.receive(
        &client.app(&message_frame(5, frame::EX_STATE, 4, 0)),
        now + 3,
        &mut listener,
    );
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
    assert!(host.queue_file(4, &data, now).is_err());
    let mut receiver = Receiver::default();
    let mut rejected_start = false;
    let mut rejected_block = false;
    let mut complete = false;
    for (number, step) in (3..).zip(0..20) {
        let t = now + step * 2 + 1;
        let out = host.poll(t);
        if out.events.contains(&Event::FileComplete { handler: 4 }) {
            complete = true;
            break;
        }
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
    let (mut host, client, _, now) = established_with_content();
    let section = owned_section(16);
    let expected = section.frame().unwrap();
    assert!(expected.len() > DEFAULT_FRAGMENT_BITS);
    host.queue_replication(section).unwrap();
    assert_eq!(host.replication_progress(), (0, 1));
    assert!(host.poll(now + REPORT_TIMEOUT_MS - 1).events.is_empty());
    let t = now + REPORT_TIMEOUT_MS;
    assert!(matches!(host.poll(t).events[..], [Event::StateSent { .. }]));
    assert_eq!(host.poll(t + STATE_WAIT_MS).events, vec![Event::TimeSent]);
    assert!(host.poll(t + 2 * STATE_WAIT_MS - 1).events.is_empty());
    let out = host.poll(t + 2 * STATE_WAIT_MS);
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
    host.poll(t);
    host.poll(t + STATE_WAIT_MS);
    assert!(matches!(
        host.poll(t + 2 * STATE_WAIT_MS).events[..],
        [Event::Replication { .. }]
    ));
    assert_eq!(host.rpc_queue.len(), MAX_RPC_QUEUE);
    let mut previous = None;
    for step in 1..=2 {
        let out = host.poll(t + 2 * STATE_WAIT_MS + step * REPLICATION_INTERVAL_MS);
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
    let expected = section.frame().unwrap();
    host.queue_replication(section.clone()).unwrap();
    other.queue_replication(section).unwrap();
    let mut player = owned_section(1);
    player.records[0].id = 3;
    host.queue_replication(player).unwrap();
    let t = now + REPORT_TIMEOUT_MS;
    for observer in [&mut host, &mut other] {
        observer.poll(t);
        observer.poll(t + STATE_WAIT_MS);
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
    host.poll(t);
    host.poll(t + STATE_WAIT_MS);
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
    let expected = replication_frame(&section, &before, sequence).unwrap();
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
fn typed_dispatch_retains_the_section_under_application_backpressure() {
    let (mut host, mut client, mut listener, now) = established_with_content();
    let t = now + REPORT_TIMEOUT_MS;
    host.poll(t);
    host.poll(t + STATE_WAIT_MS);
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
            .put(0, 7);
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
        .put(0, 7);
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
    let (mut host, _client, _listener, now) = established_with_content();
    assert!(host.poll(now + REPORT_TIMEOUT_MS - 1).events.is_empty());
    assert!(matches!(
        host.poll(now + REPORT_TIMEOUT_MS).events[..],
        [Event::StateSent { .. }]
    ));
    assert_eq!(
        host.poll(now + REPORT_TIMEOUT_MS + STATE_WAIT_MS).events,
        vec![Event::TimeSent]
    );
    assert!(
        host.poll(now + REPORT_TIMEOUT_MS + 2 * STATE_WAIT_MS)
            .events
            .is_empty()
    );
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
        .put(0, 7);
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
    let out = host.poll(2_850);
    assert_eq!(
        out.events,
        vec![Event::StateSent {
            registered: 4,
            reported: 4
        }],
        "coverage reached: state 7 at once"
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
