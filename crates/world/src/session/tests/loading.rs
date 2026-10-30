// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::bits::BitWriter;

fn ack() -> frame::LevelLoadedAck {
    frame::LevelLoadedAck {
        level: b"test/level/long-enough-for-two-chunks".to_vec(),
        identity: [4; 16],
    }
}

/// Queue/resource tests start after the exchange exercised on the wire below.
/// This is only a fixture; it deliberately does not claim to validate loading.
pub(super) fn after_exchange(host: &mut Host, now: u64) {
    let Stage::Running { world, .. } = &mut host.stage else {
        panic!("established session required")
    };
    world.state_sent = Some((now, world.state_reports));
    world.time_sent = Some((now, world.state_reports));
    world.client_level_linked = true;
    world.client_time_ready = true;
    host.level_loaded = Some(ack());
}

pub(super) fn exchange(
    host: &mut Host,
    client: &mut Client,
    listener: &mut AcceptSignals,
    now: u64,
) -> u64 {
    host.receive(
        &client.app(&queued(3, 1, vec![frame::Message::LevelLoadedAck(ack())])),
        now,
        listener,
    );
    assert!(matches!(
        host.poll(now + 1).events[..],
        [Event::StateSent { .. }]
    ));
    host.receive(
        &client.app(&queued(4, 2, vec![frame::Message::State(1)])),
        now + 2,
        listener,
    );
    assert_eq!(host.poll(now + 3).events, vec![Event::TimeSent]);
    host.receive(
        &client.app(&queued(5, 3, vec![frame::Message::State(0)])),
        now + 4,
        listener,
    );
    now + 5
}

pub(super) fn acknowledge_level(number: u16, sequence: u8) -> Vec<u8> {
    queued(
        number,
        sequence,
        vec![frame::Message::LevelLoadedAck(ack())],
    )
}

pub(super) fn state_response(number: u16, sequence: u8, state: u8) -> Vec<u8> {
    queued(number, sequence, vec![frame::Message::State(state)])
}

fn messages(sequence: u8, messages: Vec<frame::Message<'_>>) -> BitWriter {
    frame::Builder::new()
        .messages(
            None,
            vec![frame::Group {
                channel: 0,
                sequence: Some(sequence),
                messages,
            }],
        )
        .build()
        .unwrap()
}

fn queued(number: u16, sequence: u8, input: Vec<frame::Message<'_>>) -> Vec<u8> {
    let bits = messages(sequence, input);
    let mut payload = BitWriter::new();
    payload
        .put(0, 1)
        .put(0, 1)
        .put(u64::from(number), 16)
        .put(0, 6)
        .put(bits.len() as u64, 15)
        .put(1, 1)
        .put_span(bits.span());
    application::sequenced(HOST_SELECTOR, number, 0, 0, &payload)
}

#[test]
fn elapsed_time_early_states_and_old_reliable_retries_cannot_supply_loading_proofs() {
    let (mut host, mut client, mut listener, start) = established_with_content();
    host.queue_replication(owned_section(1)).unwrap();
    let t = start + 3 * REPORT_TIMEOUT_MS;
    assert!(
        host.poll(t).events.is_empty(),
        "time cannot replace level acknowledgement"
    );
    for (number, value) in [(3, 1), (4, 0)] {
        host.receive(
            &client.app(&queued(
                number,
                (number - 2) as u8,
                vec![frame::Message::State(value)],
            )),
            t + u64::from(number),
            &mut listener,
        );
    }
    assert!(host.poll(t + 10).events.is_empty());
    let chunks =
        frame::chunks(frame::EX_LEVEL_LOADED_ACK, ack().encode().unwrap().bytes()).unwrap();
    assert_eq!(chunks.len(), 2);
    host.receive(&client.app(&queued(5, 3, chunks)), t + 11, &mut listener);
    assert_eq!(host.loaded_level(), Some(&ack()));
    assert!(
        host.poll(t + 12)
            .events
            .iter()
            .any(|e| matches!(e, Event::StateSent { .. }))
    );
    let later = t + 4 * STATE_WAIT_MS;
    assert!(
        host.poll(later).events.is_empty(),
        "early State1 cannot acknowledge a later State7"
    );
    let state_one = queued(6, 5, vec![frame::Message::State(1)]);
    let mut refused = Retain::default();
    host.receive(&client.app(&state_one), later + 1, &mut refused);
    assert!(host.poll(later + 2).events.is_empty());
    host.receive(
        &client.app(&queued(7, 5, vec![frame::Message::State(1)])),
        later + 3,
        &mut listener,
    );
    assert_eq!(host.poll(later + 4).events, vec![Event::TimeSent]);
    // New application frame, old reliable message: it must not become a new
    // acknowledgement merely because the host has now sent its clock.
    host.receive(
        &client.app(&queued(8, 2, vec![frame::Message::State(0)])),
        later + 5,
        &mut listener,
    );
    assert!(host.poll(later + 5 * STATE_WAIT_MS).events.is_empty());
    host.receive(
        &client.app(&queued(9, 6, vec![frame::Message::State(0)])),
        later + 6 * STATE_WAIT_MS,
        &mut listener,
    );
    assert!(
        host.poll(later + 6 * STATE_WAIT_MS + 1)
            .events
            .iter()
            .any(|e| matches!(e, Event::Replication { .. }))
    );
    host.receive(
        &client.close(),
        later + 6 * STATE_WAIT_MS + 2,
        &mut listener,
    );
    assert_eq!(host.stage(), StageName::Closed);
    assert!(host.loaded_level().is_none());
    assert!(super::host().loaded_level().is_none());
}

#[test]
fn chunked_loaded_identity_requires_accepted_complete_message_and_is_not_replayed() {
    struct Gate<'a>(&'a std::cell::Cell<bool>);
    impl Listener for Gate<'_> {
        fn frame(&mut self, _: application::Queued<'_>) -> bool {
            self.0.get()
        }
    }
    fn input(bits: &BitWriter) -> application::Queued<'_> {
        application::Queued {
            advertised: None,
            frame: 0,
            data: bits.span(),
        }
    }
    let accepted = std::cell::Cell::new(true);
    let mut gate = Gate(&accepted);
    let mut observed = Observing {
        inner: &mut gate,
        seen: Seen::default(),
        waiting_level: None,
        messages: Default::default(),
        now_ms: 1,
        invalid: false,
    };
    let chunks =
        frame::chunks(frame::EX_LEVEL_LOADED_ACK, ack().encode().unwrap().bytes()).unwrap();
    let first = messages(0, vec![chunks[0].clone()]);
    let last = messages(1, vec![chunks[1].clone()]);
    assert!(observed.frame(input(&first)));
    assert!(observed.seen.level_loaded.is_none());
    accepted.set(false);
    assert!(!observed.frame(input(&last)));
    assert!(observed.seen.level_loaded.is_none());
    accepted.set(true);
    assert!(observed.frame(input(&last)));
    assert_eq!(observed.seen.level_loaded.take(), Some(ack()));
    assert!(observed.frame(input(&first)));
    assert!(observed.frame(input(&last)));
    assert!(observed.seen.level_loaded.is_none());
    assert!(!observed.invalid);
}
