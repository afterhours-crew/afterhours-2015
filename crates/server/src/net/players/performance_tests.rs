// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::bits::BitWriter;

fn profile(label: &[u8]) -> frame::PerformanceProfile {
    frame::PerformanceProfile {
        words: [2, 12, 3600],
        quantities: [24 << 30, 6 << 30],
        label: label.to_vec(),
        category: 1,
    }
}
fn wire(sequence: u8, messages: Vec<frame::Message<'_>>) -> BitWriter {
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
fn receive(listener: &mut PlayerListener, wire: &BitWriter) -> bool {
    listener.frame(Queued {
        advertised: None,
        frame: 0,
        data: wire.span(),
    })
}

#[test]
fn profile_waits_for_complete_chunks_and_does_not_advance_gameplay() {
    let mut listener = PlayerListener::new(7);
    let before = listener.observation(0, 0, 0, false);
    let value = profile(b"constructed hardware");
    let body = value.encode().unwrap();
    let chunks = frame::chunks(frame::EX_PERFORMANCE_PROFILE, body.bytes()).unwrap();
    assert_eq!(chunks.len(), 2);
    for (i, chunk) in chunks.iter().enumerate() {
        assert!(receive(&mut listener, &wire(i as u8, vec![chunk.clone()])));
        assert_eq!(listener.performance_profile.is_some(), i == 1);
    }
    assert_eq!(listener.performance_profile.as_ref(), Some(&value));
    let after = listener.observation(0, 0, 0, false);
    assert_eq!(before.participants, after.participants);
    assert_eq!(before.owned_queues, after.owned_queues);
    assert_eq!(after.health, Default::default());
    // Reliable retries are consumed without replacing state or generating work.
    for (i, chunk) in chunks.iter().enumerate() {
        assert!(receive(&mut listener, &wire(i as u8, vec![chunk.clone()])));
    }
    assert_eq!(listener.performance_profile, Some(value));
    assert_eq!(
        listener.observation(0, 0, 0, false).owned_queues,
        before.owned_queues
    );
}

#[test]
fn invalid_tail_and_domain_refusal_do_not_commit_profile_or_message_history() {
    let mut listener = PlayerListener::new(7);
    let value = profile(b"private hardware name");
    let message = frame::Message::PerformanceProfile(value.clone());
    let good = wire(0, vec![message.clone()]);
    let mut extra = good.clone();
    extra.put(0, 8);
    assert!(!receive(&mut listener, &extra));
    assert!(listener.performance_profile.is_none());
    let refused = wire(
        0,
        vec![
            message,
            frame::Message::CreatePlayer {
                name: b"local".to_vec(),
                flag: true,
                slot: 0,
            },
        ],
    );
    assert!(!receive(&mut listener, &refused));
    assert!(listener.performance_profile.is_none());
    assert!(receive(&mut listener, &good));
    assert_eq!(listener.performance_profile, Some(value));
    assert!(!format!("{listener:?}").contains("private hardware name"));
}

#[test]
fn latest_profile_is_bounded_connection_state_and_old_messages_cannot_replace_it() {
    let mut one = PlayerListener::new(7);
    let mut two = PlayerListener::new(8);
    let initial = profile(b"one");
    let updated = profile(b"updated");
    let other = profile(b"two");
    let old = wire(0, vec![frame::Message::PerformanceProfile(initial.clone())]);
    assert!(receive(&mut one, &old));
    assert!(two.performance_profile.is_none());
    assert!(receive(
        &mut two,
        &wire(0, vec![frame::Message::PerformanceProfile(other.clone())])
    ));
    assert!(receive(
        &mut one,
        &wire(1, vec![frame::Message::PerformanceProfile(updated.clone())])
    ));
    assert!(receive(&mut one, &old));
    assert_eq!(one.performance_profile, Some(updated));
    assert_eq!(two.performance_profile, Some(other));
    assert!(PlayerListener::new(7).performance_profile.is_none());
}
