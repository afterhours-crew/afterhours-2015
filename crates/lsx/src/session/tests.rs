// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::message::{Envelope, GAME_SDK_VERSION, Incoming};
use crate::xml::Element;

/// The game side of the exchange, built from the same primitives the game's
/// code was observed to use: a 32-character hex key, the
/// parameter-zero transform for the handshake, then the parameter derived
/// from the launcher's reply.
struct Game {
    key: String,
    cipher: Option<Aes128>,
    frames: Frames,
    next_id: u64,
}

impl Game {
    fn new(key: [u8; 16]) -> Self {
        Self {
            key: cipher::lower_hex(&key),
            cipher: None,
            frames: Frames::new(),
            next_id: 1,
        }
    }

    /// Read the launcher's frames; returns the decoded envelopes.
    fn read(&mut self, frames: &[Vec<u8>]) -> Vec<Envelope> {
        let mut out = Vec::new();
        for wire in frames {
            self.frames.feed(wire).unwrap();
        }
        while let Some(frame) = self.frames.next_frame() {
            let plain = match &self.cipher {
                None => frame,
                Some(aes) => {
                    let decoded = aes.decode_hex(&frame).unwrap();
                    assert!(
                        decoded.status.is_some(),
                        "every launcher frame reaches the block decoder"
                    );
                    decoded.visible
                }
            };
            out.push(Envelope::parse(&plain).unwrap());
        }
        out
    }

    fn send(&mut self, recipient: &str, payload: Element) -> Vec<u8> {
        let id = self.next_id;
        self.next_id += 1;
        let envelope = Envelope {
            kind: Kind::Request,
            sender: None,
            recipient: Some(recipient.to_owned()),
            id,
            payload,
        };
        let xml = envelope.to_xml();
        let wire = match &self.cipher {
            None => xml.into_bytes(),
            Some(aes) => aes.encode_hex(xml.as_bytes()).unwrap().into_bytes(),
        };
        frame::encode(&wire).unwrap()
    }

    /// The game's ChallengeResponse to a launcher challenge key.
    fn challenge_response(&mut self, launcher_key: &str) -> Vec<u8> {
        let response = Aes128::parameter_zero()
            .encode_hex(launcher_key.as_bytes())
            .unwrap();
        let payload = Element::new("ChallengeResponse")
            .attribute("response", &response)
            .attribute("key", &self.key)
            .child(Element::new("ContentId").text("1"))
            .child(Element::new("Title").text("Need for Speed"))
            .child(Element::new("MultiplayerId").text("1"))
            .child(Element::new("Language").text("en_US"))
            .child(Element::new("Version").text(GAME_SDK_VERSION));
        self.send("EALS", payload)
    }

    /// Verify the launcher's reply the way the game does and switch
    /// the transport.
    fn accept(&mut self, reply: &Envelope) {
        let response = reply.payload.attr("response").unwrap();
        let decoded = Aes128::parameter_zero()
            .decode_hex(response.as_bytes())
            .unwrap();
        assert_eq!(
            decoded.status,
            Some(-7),
            "a 32-byte key pads to a full block the game's decoder rejects"
        );
        assert_eq!(
            decoded.visible,
            self.key.as_bytes(),
            "but the wrapper still exposes the key"
        );
        let parameter = cipher::transport_parameter(response.as_bytes());
        self.cipher = Some(Aes128::for_parameter(parameter));
    }
}

fn started() -> (Launcher, Game, Vec<Envelope>) {
    let (launcher, out) = Launcher::new(config(), &mut Fixed(vec![0xab, 0xcd]));
    assert_eq!(out.events, vec![Event::ChallengeSent]);
    let mut game = Game::new([7; 16]);
    let read = game.read(&out.send);
    (launcher, game, read)
}

#[test]
fn opening_challenge_carries_our_key_in_the_clear() {
    let (launcher, _game, read) = started();
    assert_eq!(read.len(), 1);
    let challenge = &read[0];
    assert_eq!(challenge.kind, Kind::Event);
    assert_eq!(challenge.sender.as_deref(), Some("EALS"));
    assert_eq!(challenge.id, CHALLENGE_ID);
    assert_eq!(challenge.payload.name, "Challenge");
    assert_eq!(
        challenge.payload.attr("key"),
        Some("abcdabcdabcdabcdabcdabcdabcdabcd")
    );
    assert_eq!(challenge.payload.attr("version"), Some("13,805,11,6320"));
    assert_eq!(challenge.payload.attr("build"), Some("release"));
    assert!(!challenge.to_xml().contains("id="));
    assert_eq!(launcher.phase(), Phase::Challenging);
    assert_eq!(launcher.stats().frames_out, 1);
}

#[test]
fn full_startup_exchange_as_the_game_runs_it() {
    let (mut launcher, mut game, read) = started();
    let launcher_key = read[0].payload.attr("key").unwrap().to_owned();
    // Challenge response → accepted, transform switches on both sides.
    let out = launcher.receive(&game.challenge_response(&launcher_key));
    let parameter = cipher::transport_parameter(
        Aes128::parameter_zero()
            .encode_hex(game.key.as_bytes())
            .unwrap()
            .as_bytes(),
    );
    assert_eq!(out.events, vec![Event::HandshakeAccepted { parameter }]);
    assert_eq!(launcher.phase(), Phase::Established);
    let replies = game.read(&out.send);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].kind, Kind::Response);
    assert_eq!(replies[0].id, 1, "echoes the request id");
    assert_eq!(replies[0].payload.name, "ChallengeAccepted");
    game.accept(&replies[0]);
    // GetConfig → services.
    let out = launcher.receive(&game.send("EbisuSDK", Element::new("GetConfig")));
    assert_eq!(
        out.events,
        vec![Event::Answered {
            request: "GetConfig",
            id: 2
        }]
    );
    let config = &game.read(&out.send)[0];
    assert_eq!(config.sender.as_deref(), Some("EbisuSDK"));
    assert_eq!(config.payload.name, "GetConfigResponse");
    let services: Vec<(Option<&str>, Option<&str>)> = config
        .payload
        .children
        .iter()
        .map(|s| (s.attr("Name"), s.attr("Facility")))
        .collect();
    assert_eq!(
        services,
        vec![
            (Some("EbisuSDK"), Some("SDK")),
            (Some("EbisuSDK"), Some("PROFILE")),
            (Some("EbisuSDK"), Some("LOGIN")),
            (Some("EbisuSDK"), Some("PRESENCE"))
        ]
    );
    // GetProfile index 0 → identity; other indexes have no evidenced error reply.
    let out = launcher.receive(&game.send(
        "EbisuSDK",
        Element::new("GetProfile").attribute("index", "0"),
    ));
    let profile = &game.read(&out.send)[0];
    assert_eq!(profile.payload.name, "GetProfileResponse");
    assert_eq!(profile.payload.attr("UserId"), Some("536870913"));
    assert_eq!(profile.payload.attr("PersonaId"), Some("268435457"));
    assert_eq!(profile.payload.attr("Persona"), Some("Offline Driver"));
    let out = launcher.receive(&game.send(
        "EbisuSDK",
        Element::new("GetProfile").attribute("index", "1"),
    ));
    assert!(out.send.is_empty());
    assert_eq!(
        out.events,
        vec![Event::Unsupported {
            name: "GetProfile".into(),
            id: 4
        }]
    );
    // The two optional settings the initializer queries.
    let out = launcher.receive(&game.send(
        "EbisuSDK",
        Element::new("GetSetting").attribute("SettingId", "IS_IGO_ENABLED"),
    ));
    assert_eq!(
        game.read(&out.send)[0].payload.attr("Setting"),
        Some("false")
    );
    let out = launcher.receive(&game.send(
        "EbisuSDK",
        Element::new("GetGameInfo").attribute("GameInfoId", "FREETRIAL"),
    ));
    assert_eq!(
        game.read(&out.send)[0].payload.attr("GameInfo"),
        Some("false")
    );
    // Unknown requests are reported, not answered; a stray response is ignored.
    let out = launcher.receive(&game.send("EbisuSDK", Element::new("UnmodeledQuery")));
    assert_eq!(
        out.events,
        vec![Event::Unsupported {
            name: "UnmodeledQuery".into(),
            id: 7
        }]
    );
    assert!(out.send.is_empty());
    let stats = launcher.stats();
    assert_eq!(
        (stats.answered, stats.unsupported, stats.malformed),
        (5, 2, 0)
    );
    assert_eq!(stats.frames_in, 7);
    assert_eq!(stats.frames_out, 6);
}

#[test]
fn concatenated_and_fragmented_frames_after_the_handshake() {
    let (mut launcher, mut game, read) = started();
    let launcher_key = read[0].payload.attr("key").unwrap().to_owned();
    let out = launcher.receive(&game.challenge_response(&launcher_key));
    let reply = game.read(&out.send)[0].clone();
    game.accept(&reply);
    let mut wire = game.send("EbisuSDK", Element::new("GetConfig"));
    wire.extend(game.send(
        "EbisuSDK",
        Element::new("GetSetting").attribute("SettingId", "LANGUAGE"),
    ));
    let split = wire.len() / 3;
    let first = launcher.receive(&wire[..split]);
    let second = launcher.receive(&wire[split..]);
    let mut events = first.events;
    events.extend(second.events);
    assert_eq!(
        events,
        vec![
            Event::Answered {
                request: "GetConfig",
                id: 2
            },
            Event::Answered {
                request: "GetSetting",
                id: 3
            }
        ]
    );
    let mut frames = first.send;
    frames.extend(second.send);
    let replies = game.read(&frames);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1].payload.attr("Setting"), Some("en_US"));
}

#[test]
fn local_collections_complete_without_network_services_or_invented_entries() {
    let (mut launcher, mut game, read) = started();
    let key = read[0].payload.attr("key").unwrap();
    let out = launcher.receive(&game.challenge_response(key));
    let reply = game.read(&out.send)[0].clone();
    game.accept(&reply);
    for (request, reply_name) in [
        (Element::new("GetBlockList"), "GetBlockListResponse"),
        (
            Element::new("QueryFriends").attribute("UserId", "536870913"),
            "QueryFriendsResponse",
        ),
        (
            Element::new("QueryImage")
                .attribute("ImageId", "local-avatar")
                .attribute("Width", "64")
                .attribute("Height", "64"),
            "QueryImageResponse",
        ),
        (
            Element::new("QueryAchievements")
                .attribute("UserId", "536870913")
                .attribute("PersonaId", "268435457")
                .attribute("All", "true")
                .child(Element::new("GameId").text("local-title")),
            "AchievementSets",
        ),
    ] {
        let wire = game.send("EbisuSDK", request);
        let out = launcher.receive(&wire);
        assert_eq!(out.send.len(), 1, "{reply_name}");
        let reply = &game.read(&out.send)[0];
        assert_eq!(reply.payload.name, reply_name);
        assert!(reply.payload.children.is_empty());
        assert!(matches!(out.events.as_slice(), [Event::Answered { .. }]));
    }
    assert_eq!(launcher.stats().unsupported, 0);
    // The same query is safe to retry; another user's records are not ours.
    for user in ["536870913", "536870914"] {
        let out = launcher.receive(&game.send(
            "EbisuSDK",
            Element::new("QueryFriends").attribute("UserId", user),
        ));
        assert_eq!(out.send.len(), usize::from(user == "536870913"));
    }
    let out = launcher.receive(
        &game.send(
            "EbisuSDK",
            Element::new("QueryAchievements")
                .attribute("UserId", "536870913")
                .attribute("PersonaId", "99")
                .attribute("All", "false"),
        ),
    );
    assert!(matches!(out.events.as_slice(), [Event::Unsupported { .. }]));
    for request in [
        Element::new("QueryImage")
            .attribute("ImageId", "x")
            .attribute("Width", "4097")
            .attribute("Height", "64"),
        Element::new("QueryImage")
            .attribute("ImageId", "x")
            .attribute("Width", "-1")
            .attribute("Height", "64"),
        Element::new("QueryFriends"),
        Element::new("QueryAchievements")
            .attribute("UserId", "536870913")
            .attribute("PersonaId", "268435457")
            .attribute("All", "maybe"),
    ] {
        let out = launcher.receive(&game.send("EbisuSDK", request));
        assert!(out.send.is_empty());
        assert!(matches!(out.events.as_slice(), [Event::Malformed(_)]));
    }
}

#[test]
fn wrong_challenge_response_is_rejected_and_nothing_more_is_answered() {
    let (mut launcher, mut game, _) = started();
    let out = launcher.receive(&game.challenge_response("00000000000000000000000000000000"));
    assert_eq!(out.events, vec![Event::ChallengeMismatch]);
    assert!(out.send.is_empty());
    assert_eq!(launcher.phase(), Phase::Rejected);
    let out = launcher.receive(&game.send("EbisuSDK", Element::new("GetConfig")));
    assert_eq!(out.events, vec![Event::Unexpected { id: 2 }]);
    assert!(out.send.is_empty());
}

#[test]
fn out_of_order_and_malformed_input_is_reported_and_ignored() {
    let (mut launcher, mut game, _) = started();
    // A request before the handshake that is not a challenge response.
    let out = launcher.receive(&game.send("EbisuSDK", Element::new("GetConfig")));
    assert_eq!(out.events, vec![Event::Unexpected { id: 1 }]);
    // A response-kind message.
    let out = launcher.receive(b"<LSX><Response id=\"1\"><X/></Response></LSX>\0");
    assert_eq!(out.events, vec![Event::Unexpected { id: 1 }]);
    // Not XML, wrong root, no payload.
    let out = launcher.receive(b"garbage\0<x/>\0<LSX><Request id=\"2\"/></LSX>\0");
    assert_eq!(out.events.len(), 3);
    assert!(
        out.events
            .iter()
            .all(|e| matches!(e, Event::Malformed(Malformed::Message(_))))
    );
    assert_eq!(launcher.stats().malformed, 3);
    assert_eq!(launcher.phase(), Phase::Challenging, "still waiting");
    // Overflowing the pending buffer is refused.
    let out = launcher.receive(&vec![b'x'; frame::MAX_PENDING + 1]);
    assert_eq!(
        out.events,
        vec![Event::Malformed(Malformed::Frame(frame::Error::Bound))]
    );
    assert_eq!(launcher.phase(), Phase::Rejected);
    assert_eq!(launcher.frames.pending(), 0);
    // After the handshake, a frame that is not valid ciphertext is reported.
    let (mut launcher, mut game, read) = started();
    let launcher_key = read[0].payload.attr("key").unwrap().to_owned();
    let out = launcher.receive(&game.challenge_response(&launcher_key));
    let reply = game.read(&out.send)[0].clone();
    game.accept(&reply);
    let out = launcher.receive(b"<LSX><Request id=\"2\"><GetConfig/></Request></LSX>\0");
    assert_eq!(out.events, vec![Event::Malformed(Malformed::Transform)]);
    let out = launcher.receive(b"zz\0");
    assert_eq!(out.events, vec![Event::Malformed(Malformed::Transform)]);
}

#[test]
fn every_reply_is_a_request_the_game_can_type() {
    // Sanity: the launcher's replies parse back through the message layer.
    let (mut launcher, mut game, read) = started();
    let launcher_key = read[0].payload.attr("key").unwrap().to_owned();
    let out = launcher.receive(&game.challenge_response(&launcher_key));
    let accepted = game.read(&out.send)[0].clone();
    assert_eq!(
        Envelope::parse(accepted.to_xml().as_bytes())
            .unwrap()
            .incoming(),
        Ok(Incoming::Unknown {
            name: "ChallengeAccepted".into()
        }),
        "launcher replies are not game requests"
    );
}

#[test]
fn caller_selected_envelope_kinds_are_preserved() {
    let config = Config {
        challenge_kind: Kind::Response,
        reply_kind: Kind::Request,
        ..config()
    };
    let (mut launcher, out) = Launcher::new(config, &mut Fixed(vec![3]));
    let mut game = Game::new([5; 16]);
    let read = game.read(&out.send);
    assert_eq!(read[0].kind, Kind::Response);
    let launcher_key = read[0].payload.attr("key").unwrap().to_owned();
    let out = launcher.receive(&game.challenge_response(&launcher_key));
    let reply = game.read(&out.send)[0].clone();
    assert_eq!(reply.kind, Kind::Request);
    assert_eq!(reply.payload.name, "ChallengeAccepted");
}

#[test]
fn absent_setting_values_are_unsupported_instead_of_fabricated_empty_replies() {
    let config = Config {
        settings: vec![],
        game_info: vec![],
        ..config()
    };
    let (mut launcher, out) = Launcher::new(config, &mut Fixed(vec![3]));
    let mut game = Game::new([5; 16]);
    let read = game.read(&out.send);
    let out = launcher.receive(&game.challenge_response(read[0].payload.attr("key").unwrap()));
    let accepted = game.read(&out.send)[0].clone();
    game.accept(&accepted);
    for (name, field, value) in [
        ("GetSetting", "SettingId", "IS_IGO_ENABLED"),
        ("GetGameInfo", "GameInfoId", "FREETRIAL"),
    ] {
        let wire = game.send("EbisuSDK", Element::new(name).attribute(field, value));
        let out = launcher.receive(&wire);
        assert!(out.send.is_empty());
        assert_eq!(
            out.events,
            vec![Event::Unsupported {
                name: name.into(),
                id: game.next_id - 1
            }]
        );
    }
    assert_eq!(launcher.stats().answered, 1);
    assert_eq!(launcher.stats().unsupported, 2);
}

fn config() -> Config {
    Config::for_profile(Profile {
        user_id: 0x2000_0001,
        persona_id: 0x1000_0001,
        persona: "Offline Driver".into(),
        avatar_id: String::new(),
    })
}

fn established_with(profile: Profile, seed: u8) -> (Launcher, Game) {
    let (mut launcher, first) = Launcher::new(Config::for_profile(profile), &mut Fixed(vec![seed]));
    let mut game = Game::new([9; 16]);
    let hello = game.read(&first.send);
    let out = launcher.receive(&game.challenge_response(hello[0].payload.attr("key").unwrap()));
    let reply = game.read(&out.send);
    game.accept(&reply[0]);
    (launcher, game)
}

#[test]
fn concurrent_accounts_keep_profiles_and_codes_isolated_across_retries() {
    let pa = Profile {
        user_id: 11,
        persona_id: 12,
        persona: "A".into(),
        avatar_id: String::new(),
    };
    let pb = Profile {
        user_id: 21,
        persona_id: 22,
        persona: "B".into(),
        avatar_id: String::new(),
    };
    let (mut a, mut ga) = established_with(pa.clone(), 1);
    let (mut b, mut gb) = established_with(pb, 2);
    let request = || {
        Element::new("GetAuthCode")
            .attribute("ClientId", "local-client")
            .attribute("Scope", "")
    };
    let first = ga.send("Login", request());
    let code_a = ga.read(&a.receive(&first).send)[0]
        .payload
        .attr("value")
        .unwrap()
        .to_owned();
    let second = gb.send("Login", request());
    let code_b = gb.read(&b.receive(&second).send)[0]
        .payload
        .attr("value")
        .unwrap()
        .to_owned();
    assert_ne!(code_a, code_b);
    let retried = ga.read(&a.receive(&first).send);
    assert_eq!(retried[0].payload.attr("value"), Some(code_a.as_str()));
    assert_eq!(retried[0].sender.as_deref(), Some("Login"));
    assert!(!format!("{a:?}").contains(&code_a));
    for (launcher, game, expected) in [(&mut a, &mut ga, "11"), (&mut b, &mut gb, "21")] {
        let wire = game.send(
            "Profile",
            Element::new("GetProfile").attribute("index", "0"),
        );
        assert_eq!(
            game.read(&launcher.receive(&wire).send)[0]
                .payload
                .attr("UserId"),
            Some(expected)
        );
    }
    let (mut restarted, mut gr) = established_with(pa, 3);
    let wire = gr.send("Login", request());
    assert_ne!(
        gr.read(&restarted.receive(&wire).send)[0]
            .payload
            .attr("value"),
        Some(code_a.as_str())
    );
}

#[test]
fn authorization_requests_are_gated_and_bounded() {
    let (mut launcher, mut game, _) = started();
    let request = Element::new("GetAuthCode")
        .attribute("ClientId", "local-client")
        .attribute("Scope", "");
    assert!(
        launcher
            .receive(&game.send("Login", request))
            .send
            .is_empty()
    );
    let (mut launcher, mut game) = established_with(config().profile, 4);
    for (client, scope) in [
        ("".to_owned(), String::new()),
        ("x".repeat(257), String::new()),
        ("x".into(), "s".repeat(1025)),
    ] {
        let wire = game.send(
            "Login",
            Element::new("GetAuthCode")
                .attribute("ClientId", &client)
                .attribute("Scope", &scope),
        );
        let out = launcher.receive(&wire);
        assert!(out.send.is_empty());
        assert!(matches!(&out.events[..], [Event::Malformed(_)]));
    }
}

#[test]
fn invalid_peer_key_cannot_establish_a_session() {
    for key in ["short", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"] {
        let (mut launcher, mut game, read) = started();
        game.key = key.into();
        let out = launcher.receive(&game.challenge_response(read[0].payload.attr("key").unwrap()));
        assert!(out.send.is_empty());
        assert_eq!(launcher.phase(), Phase::Rejected);
    }
}

#[test]
fn connectivity_query_is_answered_before_local_authentication() {
    let (mut launcher, mut game) = established_with(config().profile, 6);
    let wire = game.send("EbisuSDK", Element::new("GetInternetConnectedState"));
    let out = launcher.receive(&wire);
    assert!(matches!(
        &out.events[..],
        [Event::Answered {
            request: "GetInternetConnectedState",
            ..
        }]
    ));
    let reply = game.read(&out.send);
    assert_eq!(reply[0].payload.attr("connected"), Some("1"));
    launcher.config.connected = false;
    let wire = game.send("EbisuSDK", Element::new("GetInternetConnectedState"));
    assert_eq!(
        game.read(&launcher.receive(&wire).send)[0]
            .payload
            .attr("connected"),
        Some("0")
    );
}

#[test]
fn presence_is_account_scoped_and_tracks_local_availability() {
    let profile = config().profile;
    let user_id = profile.user_id;
    let (mut launcher, mut game) = established_with(profile, 7);
    for online in [true, false, true] {
        launcher.config.connected = online;
        let wire = game.send(
            "PresenceService",
            Element::new("GetPresence").attribute("UserId", &user_id.to_string()),
        );
        let replies = game.read(&launcher.receive(&wire).send);
        assert_eq!(replies[0].sender.as_deref(), Some("PresenceService"));
        assert_eq!(
            replies[0].payload.attr("UserId"),
            Some(user_id.to_string().as_str())
        );
        assert_eq!(
            replies[0].payload.attr("Presence"),
            Some(if online { "ONLINE" } else { "OFFLINE" })
        );
    }
    let wire = game.send(
        "PresenceService",
        Element::new("GetPresence").attribute("UserId", &(user_id + 1).to_string()),
    );
    let out = launcher.receive(&wire);
    assert!(out.send.is_empty());
    assert!(matches!(&out.events[..], [Event::Unsupported { name, .. }] if name == "GetPresence"));
}

#[test]
fn presence_updates_round_trip_without_mutating_on_invalid_requests() {
    let profile = config().profile;
    let user_id = profile.user_id;
    let (mut launcher, mut game) = established_with(profile.clone(), 8);
    let update = |user: u64, rich: &str| {
        Element::new("SetPresence")
            .attribute("UserId", &user.to_string())
            .attribute("Presence", "INGAME")
            .attribute("RichPresence", rich)
            .attribute("GamePresence", "local drive")
            .attribute("SessionId", "local-session")
    };
    let wire = game.send("PresenceService", update(user_id, "Driving & testing"));
    let reply = game.read(&launcher.receive(&wire).send);
    assert_eq!(reply[0].payload.name, "ErrorSuccess");
    assert_eq!(reply[0].payload.attr("Code"), Some("0"));
    for (user, text) in [(user_id + 1, "other".into()), (user_id, "x".repeat(1025))] {
        let wire = game.send("PresenceService", update(user, &text));
        assert!(launcher.receive(&wire).send.is_empty());
    }
    let wire = game.send(
        "PresenceService",
        Element::new("GetPresence").attribute("UserId", &user_id.to_string()),
    );
    let reply = game.read(&launcher.receive(&wire).send);
    assert_eq!(reply[0].payload.attr("Presence"), Some("INGAME"));
    assert_eq!(
        reply[0].payload.attr("RichPresence"),
        Some("Driving & testing")
    );
    assert_eq!(reply[0].payload.attr("SessionId"), Some("local-session"));
    let (mut other, mut other_game) = established_with(profile, 9);
    let wire = other_game.send(
        "PresenceService",
        Element::new("GetPresence").attribute("UserId", &user_id.to_string()),
    );
    let reply = other_game.read(&other.receive(&wire).send);
    assert_eq!(reply[0].payload.attr("Presence"), Some("ONLINE"));
    assert_eq!(reply[0].payload.attr("SessionId"), Some(""));
}
