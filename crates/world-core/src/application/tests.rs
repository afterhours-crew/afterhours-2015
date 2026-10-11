// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::world::{
    admission::Decoded as Admission,
    envelope::ZeroTail,
    timing::{Control, Decoded as Timing},
};

const CHALLENGE: u32 = 0x1111_1111;
const ENGINE: u32 = 0x2222_2222;
const HOST_FRESH: u32 = 0x3333_3333;
const PEER_TOKEN: u32 = 0x4444_4444;
const CLIENT_FRESH: u32 = 0x5555_5555;
const CLIENT_SELECTOR: u16 = 7;

#[test]
fn tracked_frames_require_every_fragment_and_ignore_future_reports() {
    use delivery::Status;
    let (mut a, _) = admitted(0);
    let mut l = Retain::default();
    let mut bits = BitWriter::new();
    bits.put(0, 24);
    let (ticket, bodies) = a.send_tracked_frame(bits.span(), 8).unwrap();
    assert_eq!(bodies.len(), 3);
    let report = |number, ack, history| {
        sequenced(
            HOST_SELECTOR,
            number,
            ack,
            history,
            &control_payload(CONTROL_EMPTY, &[]),
        )
    };
    a.receive(&report(1, 4, u32::MAX), 4, &mut l).unwrap();
    assert_eq!(
        a.delivery_status(ticket),
        Some(Status::Pending),
        "unsent frontier ignored"
    );
    a.receive(&report(2, 2, 3), 5, &mut l).unwrap();
    assert_eq!(a.delivery_status(ticket), Some(Status::Pending));
    a.receive(&report(3, 3, 6), 6, &mut l).unwrap();
    assert_eq!(a.release_delivery(ticket), Some(Status::Rejected));
    let (retry, _) = a.send_tracked_frame(bits.span(), 8).unwrap();
    assert_ne!(retry, ticket);
    a.receive(&report(4, 3, u32::MAX), 7, &mut l).unwrap();
    assert_eq!(a.delivery_status(retry), Some(Status::Pending));
    a.receive(&report(5, 6, 7), 8, &mut l).unwrap();
    assert_eq!(a.release_delivery(retry), Some(Status::Accepted));
    assert_eq!(a.release_delivery(ticket), None);
    let before = a.stats().sent;
    bits.put(0, 16);
    assert!(
        a.send_tracked_frame(bits.span(), 1).is_err(),
        "max body/window preflight"
    );
    assert_eq!(a.stats().sent, before);
}

#[test]
fn receipt_tokens_are_bounded_and_old_pending_wire_numbers_cannot_survive_wrap() {
    use delivery::{Deliveries, Status};
    use nfs_protocol::world::history::{PeerReports, Sequence};
    let mut d = Deliveries::default();
    let old = d.insert(1, 1);
    d.sent(1, 1); // a new lifetime of wire sequence1, after modulo wrap
    assert_eq!(d.status(old), Some(Status::Rejected));
    let fresh = d.insert(1, 1);
    let mut reports = PeerReports::new(Sequence::new(0).unwrap());
    d.apply(reports.consume(Sequence::new(1).unwrap(), 1).unwrap());
    assert_eq!(d.remove(fresh), Some(Status::Accepted));
    assert_eq!(d.remove(old), Some(Status::Rejected));
    for sequence in 1..=32 {
        assert!(d.available().is_ok());
        d.insert(sequence, 1);
    }
    assert_eq!(d.available(), Err(Error::Bound));
}

fn app() -> Application {
    Application::new(
        Policy::default(),
        Box::new(Fixed::new([CHALLENGE, ENGINE, HOST_FRESH])),
    )
}

fn request13() -> Vec<u8> {
    Request13::new([17, 29]).encode()
}

fn answer16() -> Vec<u8> {
    OpaqueAnswer16::new([0xa5; 20], &[]).unwrap().encode()
}

fn client2(engine_echo: u32) -> Vec<u8> {
    Admission::new(Message::Client2 {
        peer_token: PEER_TOKEN,
        engine_echo,
        opaque: vec![0x5a; 45],
    })
    .unwrap()
    .encode()
    .unwrap()
}

fn client5() -> Vec<u8> {
    Client5 {
        peer_echo: PEER_TOKEN,
        client_fresh: CLIENT_FRESH,
        client_selector: CLIENT_SELECTOR,
        callback: vec![],
    }
    .new_body(HOST_SELECTOR)
    .unwrap()
}

/// A queued client envelope: one fragment of `frame` holding `bits` data bits.
fn queued(
    number: u16,
    ack: u16,
    history: u32,
    frame: u16,
    ordinal: u8,
    last: bool,
    bits: usize,
) -> Vec<u8> {
    let mut payload = BitWriter::new();
    payload
        .put(0, 1)
        .put(0, 1)
        .put(u64::from(frame), 16)
        .put(u64::from(ordinal), 6)
        .put(bits as u64, 15)
        .put_bool(last);
    for i in 0..bits {
        payload.put((i % 2) as u64, 1);
    }
    sequenced(HOST_SELECTOR, number, ack, history, &payload)
}

/// Drive a fresh application through admission; returns it plus the Host6 body.
fn admitted(now: u64) -> (Application, Vec<u8>) {
    let mut a = app();
    let mut listener = Retain::default();
    a.receive(&request13(), now, &mut listener).unwrap();
    a.receive(&answer16(), now + 1, &mut listener).unwrap();
    a.receive(&client2(ENGINE), now + 2, &mut listener).unwrap();
    let r = a.receive(&client5(), now + 3, &mut listener).unwrap();
    assert_eq!(a.phase_name(), PhaseName::Sequenced);
    (a, r.send[0].clone())
}

#[test]
fn startup_exchange_has_the_observed_sizes_and_echoes() {
    let mut a = app();
    let mut l = Retain::default();
    assert_eq!(request13().len(), 12, "observed 12-byte request");
    let r = a.receive(&request13(), 100, &mut l).unwrap();
    assert_eq!(r.event, Some(Event::ChallengeRequest { repeated: false }));
    assert_eq!(
        r.send[0].len(),
        8,
        "observed 8-byte challenge (35-byte wire)"
    );
    let challenge = Challenge15::decode(&r.send[0]).unwrap();
    assert_eq!(
        (challenge.value(), challenge.optional()),
        (CHALLENGE, &[][..])
    );
    // A retransmitted request gets the identical challenge.
    let again = a.receive(&request13(), 150, &mut l).unwrap();
    assert_eq!(
        again.event,
        Some(Event::ChallengeRequest { repeated: true })
    );
    assert_eq!(again.send, r.send);

    assert_eq!(answer16().len(), 24, "observed 24-byte answer");
    let r = a.receive(&answer16(), 200, &mut l).unwrap();
    assert_eq!(r.event, Some(Event::Answer { repeated: false }));
    assert_eq!(r.send[0].len(), 8, "observed 8-byte Host1 (35-byte wire)");
    assert!(matches!(
        Admission::decode(&r.send[0]).unwrap().message,
        Message::Host1 {
            engine_token: ENGINE
        }
    ));
    let again = a.receive(&answer16(), 210, &mut l).unwrap();
    assert_eq!(again.event, Some(Event::Answer { repeated: true }));
    assert_eq!(again.send, r.send);

    assert_eq!(client2(ENGINE).len(), 59, "observed 59-byte Client2");
    let r = a.receive(&client2(ENGINE), 300, &mut l).unwrap();
    assert_eq!(r.event, Some(Event::Client2 { repeated: false }));
    assert_eq!(
        (r.send[0].len(), r.send[1].len()),
        (4, 12),
        "Host9 31 B, Host4 39 B wires"
    );
    assert!(matches!(
        Admission::decode(&r.send[0]).unwrap().message,
        Message::Host9
    ));
    assert!(matches!(
        Admission::decode(&r.send[1]).unwrap().message,
        // Official hosts assigned 4; 1 is the client's own name for the host (E762).
        Message::Host4 { peer_echo: PEER_TOKEN, assigned_selector: 4, callback_kind: 0, ref opaque } if opaque.is_empty()
    ));
    let again = a.receive(&client2(ENGINE), 310, &mut l).unwrap();
    assert_eq!(again.event, Some(Event::Client2 { repeated: true }));
    assert_eq!(again.send, r.send);

    assert_eq!(client5().len(), 16, "observed 16-byte Client5");
    let r = a.receive(&client5(), 400, &mut l).unwrap();
    assert_eq!(r.event, Some(Event::Client5 { repeated: false }));
    assert_eq!(
        r.send[0].len(),
        13,
        "observed 13-byte Host6 (40/56-byte wire)"
    );
    let h6 = Admission::decode(&r.send[0]).unwrap();
    assert!(matches!(
        h6.message,
        Message::Transformed {
            kind: 6,
            selector: CLIENT_SELECTOR,
            ..
        }
    ));
    let fields = Host6::decode(&h6).unwrap();
    assert_eq!(
        (fields.client_echo, fields.host_fresh),
        (CLIENT_FRESH, HOST_FRESH)
    );
    assert_eq!(a.selectors(), Some((HOST_SELECTOR, CLIENT_SELECTOR)));
    assert_eq!(a.receive_history(), Some((0, 0)));
    let again = a.receive(&client5(), 410, &mut l).unwrap();
    assert_eq!(again.event, Some(Event::Client5 { repeated: true }));
    assert_eq!(again.send, r.send);
    assert_eq!(a.stats().inputs, 8);
}

#[test]
fn sequenced_encoder_agrees_with_the_timing_codec_and_control1_shape() {
    for control in [
        Control::Request {
            timestamp: 0x3ff0_0000_0000_0000,
        },
        Control::Reply {
            echo: 0x3ff0_0000_0000_0000,
            clock: 0x4000_0000_0000_0000,
        },
    ] {
        let (subtype, stamps) = match control {
            Control::Request { timestamp } => (CONTROL_TIMING_REQUEST, vec![timestamp]),
            Control::Reply { echo, clock } => (CONTROL_TIMING_REPLY, vec![echo, clock]),
        };
        let ours = sequenced(9, 5, 3, 0b101, &control_payload(subtype, &stamps));
        let codec = Timing::new(9, 5, 3, 0b101, control).unwrap().encode();
        assert_eq!(ours, codec);
    }
    let control1 = sequenced(1, 33, 1, 1, &control_payload(CONTROL_EMPTY, &[]));
    assert_eq!(control1.len(), 13, "13-byte empty control");
    let e = ZeroTail::new(64).unwrap().decode(&control1).unwrap();
    assert_eq!(e.effective_bits(), 101, "101 effective bits");
    let s = e.sequence().unwrap();
    assert_eq!(
        (s.number().value(), s.acknowledgement().value(), s.history()),
        (33, 1, 1)
    );
    assert!(matches!(
        Route::decode(e.payload()).unwrap(),
        Route::Control { subtype: 1, timestamps: [None, None], body } if body.is_empty()
    ));
}

#[test]
fn queued_frames_reach_the_listener_and_start_the_timing_exchange() {
    let (mut a, _) = admitted(1_000);
    let mut l = Retain::default();
    let first = queued(1, 0, 0, 0, 0, true, 560);
    let r = a.receive(&first, 1_010, &mut l).unwrap();
    assert_eq!(
        r.event,
        Some(Event::Queued {
            bits: 560,
            complete: true,
            accepted: false
        })
    );
    assert_eq!((l.frames, l.bits), (1, 560));
    assert_eq!(a.receive_history(), Some((1, 0)), "frontier 1, history 0");
    // The first queued input triggers the host's first timing request:
    // sequence 1, acknowledgement 1, history 0, 21 bytes (48-byte wire).
    let [request] = r.send.as_slice() else {
        panic!()
    };
    assert_eq!(request.len(), 21);
    let t = Timing::decode(request).unwrap();
    assert_eq!(
        (t.selector(), t.number(), t.acknowledgement(), t.history()),
        (CLIENT_SELECTOR, 1, 1, 0)
    );
    let Control::Request { timestamp } = t.control() else {
        panic!()
    };
    assert_eq!(f64::from_bits(timestamp), 0.007, "seconds since Host6");
    // A retransmission of sequence 1 is a duplicate; nothing is sent.
    let dup = a.receive(&first, 1_020, &mut l).unwrap();
    assert_eq!((dup.event, dup.send.len()), (Some(Event::Duplicate), 0));
    assert_eq!(l.frames, 1);
    // The client's reply echoes our timestamp: matched, with a round trip.
    let reply = Timing::new(
        HOST_SELECTOR,
        2,
        1,
        1,
        Control::Reply {
            echo: timestamp,
            clock: 0,
        },
    )
    .unwrap()
    .encode();
    let r = a.receive(&reply, 1_050, &mut l).unwrap();
    assert_eq!(
        r.event,
        Some(Event::TimingReply {
            matched: true,
            round_trip_ms: Some(40)
        })
    );
    assert!(r.send.is_empty());
    assert_eq!(a.receive_history(), Some((2, 0b01)));
    // The client's own request is answered with the pre-commit frontier/history.
    let req = Timing::new(HOST_SELECTOR, 3, 1, 1, Control::Request { timestamp: 77 })
        .unwrap()
        .encode();
    let r = a.receive(&req, 1_100, &mut l).unwrap();
    assert_eq!(r.event, Some(Event::TimingRequest));
    let [reply] = r.send.as_slice() else { panic!() };
    assert_eq!(
        reply.len(),
        29,
        "observed 29-byte timing reply (56-byte wire)"
    );
    let t = Timing::decode(reply).unwrap();
    assert_eq!((t.number(), t.acknowledgement(), t.history()), (2, 2, 0b01));
    assert!(
        matches!(t.control(), Control::Reply { echo: 77, clock } if f64::from_bits(clock) == 0.097)
    );
    assert_eq!(a.receive_history(), Some((3, 0b011)));
    // An empty control is consumed silently and counts as accepted.
    let control1 = sequenced(
        HOST_SELECTOR,
        4,
        2,
        0b11,
        &control_payload(CONTROL_EMPTY, &[]),
    );
    let r = a.receive(&control1, 1_110, &mut l).unwrap();
    assert_eq!((r.event, r.send.len()), (Some(Event::EmptyControl), 0));
    assert_eq!(a.receive_history(), Some((4, 0b0111)));
    // An unknown control is recorded unaccepted.
    let unknown = sequenced(HOST_SELECTOR, 5, 2, 0b11, &control_payload(7, &[]));
    let r = a.receive(&unknown, 1_120, &mut l).unwrap();
    assert_eq!(r.event, Some(Event::UnknownControl(7)));
    assert_eq!(a.receive_history(), Some((5, 0b01110)));
    let s = a.stats();
    assert_eq!(
        (
            s.timing_requests_sent,
            s.timing_replies_sent,
            s.timing_matched,
            s.empty_controls_received
        ),
        (1, 1, 1, 1)
    );
}

#[test]
fn fragments_reassemble_before_the_listener_sees_the_frame() {
    let (mut a, _) = admitted(0);
    let mut l = Retain::default();
    let r = a
        .receive(&queued(1, 0, 0, 3, 0, false, 100), 10, &mut l)
        .unwrap();
    assert_eq!(
        r.event,
        Some(Event::Queued {
            bits: 100,
            complete: false,
            accepted: true
        })
    );
    assert_eq!(l.frames, 0);
    let r = a
        .receive(&queued(2, 0, 0, 3, 1, true, 50), 20, &mut l)
        .unwrap();
    assert_eq!(
        r.event,
        Some(Event::Queued {
            bits: 50,
            complete: true,
            accepted: false
        })
    );
    assert_eq!((l.frames, l.bits), (1, 150));
    assert_eq!(
        a.receive_history(),
        Some((2, 0b10)),
        "bit 1 is the accepted pending fragment"
    );
    // A skipped ordinal discards the frame.
    let r = a
        .receive(&queued(3, 0, 0, 4, 1, true, 8), 30, &mut l)
        .unwrap();
    assert_eq!(
        r.event,
        Some(Event::Queued {
            bits: 8,
            complete: false,
            accepted: false
        })
    );
}

#[test]
fn out_of_window_and_foreign_inputs_leave_state_unchanged() {
    let (mut a, _) = admitted(0);
    let mut l = Retain::default();
    let r = a
        .receive(&queued(40, 0, 0, 0, 0, true, 8), 10, &mut l)
        .unwrap();
    assert_eq!(r.event, Some(Event::OutOfWindow));
    assert_eq!(a.receive_history(), Some((0, 0)));
    assert_eq!(
        a.receive(
            &Timing::new(2, 1, 0, 0, Control::Request { timestamp: 1 })
                .unwrap()
                .encode(),
            20,
            &mut l
        ),
        Err(Error::Selector)
    );
    assert_eq!(a.receive(&[0; 3], 30, &mut l), Err(Error::Shape));
    assert_eq!(a.receive(&request13(), 40, &mut l), Err(Error::Phase));
    assert_eq!(a.receive(&vec![0; 1201], 50, &mut l), Err(Error::Bound));
    assert_eq!(a.receive(&[0; 12], 10, &mut l), Err(Error::Clock));
    assert_eq!(a.receive_history(), Some((0, 0)));
    assert_eq!(a.stats().inputs, 5, "four admission inputs and one ignored");

    let mut b = app();
    b.receive(&request13(), 0, &mut l).unwrap();
    assert_eq!(
        b.receive(&Request13::new([1, 2]).encode(), 1, &mut l),
        Err(Error::ChangedRetry)
    );
    b.receive(&answer16(), 2, &mut l).unwrap();
    assert_eq!(b.receive(&client2(ENGINE + 1), 3, &mut l), Err(Error::Echo));
    assert_eq!(b.phase_name(), PhaseName::AwaitClient2);
    b.receive(&client2(ENGINE), 4, &mut l).unwrap();
    let wrong_peer = Client5 {
        peer_echo: PEER_TOKEN + 1,
        client_fresh: CLIENT_FRESH,
        client_selector: CLIENT_SELECTOR,
        callback: vec![],
    }
    .new_body(HOST_SELECTOR)
    .unwrap();
    assert_eq!(b.receive(&wrong_peer, 5, &mut l), Err(Error::Echo));
    assert_eq!(b.phase_name(), PhaseName::AwaitClient5);
}

#[test]
fn an_expired_challenge_is_replaced_by_a_fresh_one() {
    let mut a = Application::new(Policy::default(), Box::new(Fixed::new([1, 2, 3])));
    let mut l = Retain::default();
    let first = a.receive(&request13(), 0, &mut l).unwrap();
    let second = a
        .receive(&request13(), CHALLENGE_WINDOW_MS, &mut l)
        .unwrap();
    assert_eq!(
        second.event,
        Some(Event::ChallengeRequest { repeated: false })
    );
    assert_eq!(Challenge15::decode(&first.send[0]).unwrap().value(), 1);
    assert_eq!(Challenge15::decode(&second.send[0]).unwrap().value(), 2);
}

#[test]
fn timers_send_periodic_requests_and_an_empty_control_under_window_pressure() {
    let (mut a, _) = admitted(0);
    let mut l = Retain::default();
    assert!(
        a.poll(100).unwrap().is_empty(),
        "no request before queued data"
    );
    a.receive(&queued(1, 0, 0, 0, 0, true, 8), 200, &mut l)
        .unwrap();
    assert!(a.poll(1_000).unwrap().is_empty());
    let sent = a.poll(2_200).unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(Timing::decode(&sent[0]).unwrap().number(), 2);
    // Fill the window with unacknowledged frames: 2 sent, 29 more makes 31.
    let frame = [0xaa; 29];
    for _ in 0..29 {
        a.send_frame(BitSpan::new(&frame, 0, 232).unwrap()).unwrap();
    }
    assert_eq!(
        a.send_frame(BitSpan::new(&frame, 0, 232).unwrap()),
        Err(Error::Window),
        "the native scheduler stops queued sends at a full window"
    );
    let sent = a.poll(2_300).unwrap();
    assert_eq!(sent.len(), 1, "empty control at 31 outstanding");
    assert_eq!(sent[0].len(), 13);
    let e = ZeroTail::new(64).unwrap().decode(&sent[0]).unwrap();
    assert_eq!(e.sequence().unwrap().number().value(), 32);
    assert!(
        a.poll(2_400).unwrap().is_empty(),
        "within the control interval"
    );
    assert_eq!(a.poll(2_800).unwrap().len(), 1);
    // The client acknowledging our sends relieves the pressure.
    a.receive(&queued(2, 30, 0, 1, 0, true, 8), 3_000, &mut l)
        .unwrap();
    assert!(a.poll(3_400).unwrap().is_empty());
    assert_eq!(a.stats().empty_controls_sent, 2);
}

#[test]
fn send_frame_wraps_a_single_fragment_with_the_advertisement_absent() {
    let (mut a, _) = admitted(0);
    let frame = [0x5a; 29];
    let body = a.send_frame(BitSpan::new(&frame, 0, 232).unwrap()).unwrap();
    assert_eq!(body.len(), 46, "368 effective bits in 46 bytes");
    let e = ZeroTail::new(64).unwrap().decode(&body).unwrap();
    assert_eq!(
        (e.connection_selector(), e.effective_bits()),
        (CLIENT_SELECTOR, 368)
    );
    let s = e.sequence().unwrap();
    assert_eq!(
        (s.number().value(), s.acknowledgement().value(), s.history()),
        (1, 0, 0)
    );
    let Route::Queued {
        advertised: None,
        body,
    } = Route::decode(e.payload()).unwrap()
    else {
        panic!()
    };
    let f = Fragment::decode(body).unwrap();
    assert_eq!(
        (f.frame(), f.ordinal(), f.is_final(), f.data().len()),
        (0, 0, true, 232)
    );
    assert_eq!(f.data().read_u32(0, 8).unwrap(), 0x5a);
    let next = a.send_frame(BitSpan::new(&frame, 0, 8).unwrap()).unwrap();
    let e = ZeroTail::new(64).unwrap().decode(&next).unwrap();
    assert_eq!(e.sequence().unwrap().number().value(), 2);
    let Route::Queued { body, .. } = Route::decode(e.payload()).unwrap() else {
        panic!()
    };
    assert_eq!(Fragment::decode(body).unwrap().frame(), 1);
    assert_eq!(
        app().send_frame(BitSpan::new(&frame, 0, 8).unwrap()),
        Err(Error::Phase)
    );
}

#[test]
fn send_frame_fragments_splits_at_the_e101_size_and_needs_window_room() {
    let (mut a, _) = admitted(0);
    let sent_before = a.stats().sent;
    let frame: Vec<u8> = (0..1_125u32).map(|i| (i * 7 % 251) as u8).collect();
    let span = BitSpan::new(&frame, 0, 9_000).unwrap();
    let bodies = a.send_frame_fragments(span, DEFAULT_FRAGMENT_BITS).unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0].len(), 1_024, "1,024-byte first fragment body");
    let mut assembler = Assembler::new(1 << 16).unwrap();
    let mut frames = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        let e = ZeroTail::new(1600).unwrap().decode(body).unwrap();
        assert_eq!(e.sequence().unwrap().number().value(), 1 + i as u16);
        let Route::Queued {
            advertised: None,
            body: q,
        } = Route::decode(e.payload()).unwrap()
        else {
            panic!()
        };
        let f = Fragment::decode(q).unwrap();
        assert_eq!(
            (f.frame(), usize::from(f.ordinal()), f.is_final()),
            (0, i, i == 1)
        );
        assert_eq!(
            f.data().len(),
            if i == 0 { DEFAULT_FRAGMENT_BITS } else { 944 }
        );
        if let Outcome::Complete(whole) = assembler.push(f).unwrap() {
            frames.push((whole.data().bytes().to_vec(), whole.data().len()));
        }
    }
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].1, 9_000);
    assert_eq!(&frames[0].0[..1_125], &frame[..]);
    assert_eq!(a.stats().sent, sent_before + 2);
    // A small frame is one fragment, identical to send_frame's output shape.
    let small = [0x5a; 29];
    let one = a
        .send_frame_fragments(BitSpan::new(&small, 0, 232).unwrap(), DEFAULT_FRAGMENT_BITS)
        .unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].len(), 46);
    let e = ZeroTail::new(64).unwrap().decode(&one[0]).unwrap();
    assert_eq!(e.sequence().unwrap().number().value(), 3);
    // Window: 3 outstanding; 27 more single fragments reach 30; a two-fragment
    // frame then does not fit (31 is the limit) and nothing is consumed.
    for _ in 0..27 {
        a.send_frame(BitSpan::new(&small, 0, 232).unwrap()).unwrap();
    }
    assert_eq!(
        a.send_frame_fragments(span, DEFAULT_FRAGMENT_BITS),
        Err(Error::Window)
    );
    let next = a.send_frame(BitSpan::new(&small, 0, 8).unwrap()).unwrap();
    let e = ZeroTail::new(64).unwrap().decode(&next).unwrap();
    assert_eq!(
        e.sequence().unwrap().number().value(),
        31,
        "nothing was consumed"
    );
    assert_eq!(
        app().send_frame_fragments(span, DEFAULT_FRAGMENT_BITS),
        Err(Error::Phase)
    );
}
