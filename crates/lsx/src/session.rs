// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A connection-owned LSX handshake and local service policy, without IO.

use crate::{
    cipher::{self, Aes128},
    frame::{self, Frames},
    message::{
        Envelope, Facility, GameInfo, Incoming, Kind, LocalCollection, PresenceState,
        PresenceUpdate, Profile, SDK_SERVICE, Setting,
    },
};
use std::fmt;

/// Source of the launcher's challenge key bytes.
pub trait Random {
    fn fill(&mut self, bytes: &mut [u8]);
}

/// Deterministic bytes for tests and fixtures (repeats its pattern).
#[derive(Clone, Debug)]
pub struct Fixed(pub Vec<u8>);

impl Random for Fixed {
    fn fill(&mut self, bytes: &mut [u8]) {
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = self.0.get(i % self.0.len().max(1)).copied().unwrap_or(0);
        }
    }
}

/// Local launcher policy. Startup support is separate from backend login.
#[derive(Clone, Debug)]
pub struct Config {
    /// Version and build attributes of the opening challenge.
    pub version: String,
    pub build: String,
    /// The `GetConfigResponse` services: the name the game will address and
    /// the facility it serves. The `PROFILE` entry's name is where the game
    /// sends `GetProfile`; the `SDK` entry's name is where it sends
    /// the settings queries.
    pub services: Vec<(String, Facility)>,
    pub profile: Profile,
    /// Local-service availability reported to the SDK; no Internet probe is made.
    pub connected: bool,
    pub settings: Vec<(Setting, String)>,
    pub game_info: Vec<(GameInfo, String)>,
    /// Envelope kind of the opening challenge (`Event` by default).
    pub challenge_kind: Kind,
    /// Envelope kind of every reply (`Response` by default).
    pub reply_kind: Kind,
}

impl Config {
    /// The caller supplies its account identity; no global default account is used.
    pub fn for_profile(profile: Profile) -> Self {
        Self {
            version: "13,805,11,6320".to_owned(),
            build: "release".to_owned(),
            services: vec![
                (SDK_SERVICE.to_owned(), Facility::Sdk),
                (SDK_SERVICE.to_owned(), Facility::Profile),
                (SDK_SERVICE.to_owned(), Facility::Login),
                (SDK_SERVICE.to_owned(), Facility::Presence),
            ],
            profile,
            connected: true,
            settings: vec![
                (Setting::Language, "en_US".to_owned()),
                (Setting::Environment, "production".to_owned()),
                (Setting::IgoAvailable, "false".to_owned()),
                (Setting::IgoEnabled, "false".to_owned()),
                (Setting::TelemetryEnabled, "false".to_owned()),
            ],
            game_info: vec![
                (GameInfo::UpToDate, "true".to_owned()),
                (GameInfo::Languages, "en_US".to_owned()),
                (GameInfo::FreeTrial, "false".to_owned()),
                (GameInfo::InstalledVersion, "1.0.0.0".to_owned()),
                (GameInfo::InstalledLanguage, "en_US".to_owned()),
                (GameInfo::AvailableVersion, "1.0.0.0".to_owned()),
                (GameInfo::DisplayName, "Need for Speed".to_owned()),
            ],
            challenge_kind: Kind::Event,
            reply_kind: Kind::Response,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// The opening challenge went out.
    ChallengeSent,
    /// The game's challenge response verified; the transform is now keyed by
    /// this transport parameter.
    HandshakeAccepted { parameter: u32 },
    /// The game's `response` did not match our key; no reply was sent.
    ChallengeMismatch,
    /// A request was answered: the payload name and the request id.
    Answered { request: &'static str, id: u64 },
    /// A request this launcher does not model; no reply was sent.
    Unsupported { name: String, id: u64 },
    /// A message out of order (before the handshake and not a challenge
    /// response, after a rejection, or a response-kind message); ignored.
    Unexpected { id: u64 },
    /// A frame that did not decode or parse. A framing bound rejects the session.
    Malformed(Malformed),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Malformed {
    Transform,
    Message(crate::message::Error),
    Frame(frame::Error),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Output {
    /// Frames to write, in order (each already NUL-terminated).
    pub send: Vec<Vec<u8>>,
    pub events: Vec<Event>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub frames_in: usize,
    pub frames_out: usize,
    pub answered: usize,
    pub unsupported: usize,
    pub malformed: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// Challenge sent, waiting for the game's response.
    Challenging,
    /// Handshake verified; the transform is active.
    Established,
    /// The challenge response or framing bounds were rejected; nothing more is answered.
    Rejected,
}

/// One launcher connection.
pub struct Launcher {
    config: Config,
    frames: Frames,
    phase: Phase,
    /// Our challenge key, 32 lowercase hex characters.
    challenge_key: String,
    /// The transform after the handshake.
    cipher: Option<Aes128>,
    stats: Stats,
    /// Fresh local-only code, redacted by Debug and never logged.
    auth_code: String,
    presence: PresenceUpdate,
}

impl fmt::Debug for Launcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Launcher")
            .field("phase", &self.phase)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

/// The opening Event has no correlation id; the codec represents absence as zero.
pub const CHALLENGE_ID: u64 = 0;

impl Launcher {
    /// Start a connection: returns the launcher and the frames to send first
    /// (the challenge).
    pub fn new(config: Config, random: &mut dyn Random) -> (Self, Output) {
        let mut key = [0u8; 16];
        random.fill(&mut key);
        let mut code = [0u8; 32];
        random.fill(&mut code);
        let mut launcher = Self {
            presence: PresenceUpdate::online(config.profile.user_id),
            config,
            frames: Frames::new(),
            phase: Phase::Challenging,
            challenge_key: cipher::lower_hex(&key),
            cipher: None,
            stats: Stats::default(),
            auth_code: cipher::lower_hex(&code),
        };
        let challenge = Envelope::challenge(
            launcher.config.challenge_kind,
            CHALLENGE_ID,
            &launcher.challenge_key,
            &launcher.config.version,
            &launcher.config.build,
        );
        let mut out = Output::default();
        launcher.emit(&challenge, &mut out);
        out.events.push(Event::ChallengeSent);
        (launcher, out)
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Bytes read from the game.
    pub fn receive(&mut self, bytes: &[u8]) -> Output {
        let mut out = Output::default();
        if let Err(e) = self.frames.feed(bytes) {
            self.frames = Frames::new();
            self.phase = Phase::Rejected;
            self.stats.malformed += 1;
            out.events.push(Event::Malformed(Malformed::Frame(e)));
            return out;
        }
        while let Some(frame) = self.frames.next_frame() {
            self.stats.frames_in += 1;
            self.message(&frame, &mut out);
        }
        out
    }

    fn message(&mut self, frame: &[u8], out: &mut Output) {
        let plain = match &self.cipher {
            None => frame.to_vec(),
            Some(aes) => match aes.decode_hex(frame) {
                Ok(decoded) if !decoded.visible.is_empty() => decoded.visible,
                _ => {
                    self.stats.malformed += 1;
                    out.events.push(Event::Malformed(Malformed::Transform));
                    return;
                }
            },
        };
        let envelope = match Envelope::parse(&plain) {
            Ok(e) => e,
            Err(e) => {
                self.stats.malformed += 1;
                out.events.push(Event::Malformed(Malformed::Message(e)));
                return;
            }
        };
        if envelope.kind != Kind::Request {
            out.events.push(Event::Unexpected { id: envelope.id });
            return;
        }
        let incoming = match envelope.incoming() {
            Ok(i) => i,
            Err(e) => {
                self.stats.malformed += 1;
                out.events.push(Event::Malformed(Malformed::Message(e)));
                return;
            }
        };
        match (self.phase, incoming) {
            (Phase::Rejected, _) => out.events.push(Event::Unexpected { id: envelope.id }),
            (Phase::Challenging, Incoming::ChallengeResponse { response, key, .. }) => {
                self.handshake(envelope.id, &response, &key, out);
            }
            (Phase::Challenging, _) => out.events.push(Event::Unexpected { id: envelope.id }),
            (Phase::Established, incoming) => self.answer(&envelope, incoming, out),
        }
    }

    fn handshake(&mut self, id: u64, response: &str, game_key: &str, out: &mut Output) {
        if game_key.len() != 32 || !game_key.bytes().all(|b| b.is_ascii_hexdigit()) {
            self.phase = Phase::Rejected;
            out.events.push(Event::ChallengeMismatch);
            return;
        }
        let zero = Aes128::parameter_zero();
        let expected = zero
            .encode_hex(self.challenge_key.as_bytes())
            .expect("32 bytes");
        if !response.eq_ignore_ascii_case(&expected) {
            self.phase = Phase::Rejected;
            out.events.push(Event::ChallengeMismatch);
            return;
        }
        let Ok(reply) = zero.encode_hex(game_key.as_bytes()) else {
            self.phase = Phase::Rejected;
            out.events.push(Event::ChallengeMismatch);
            return;
        };
        let accepted = Envelope::challenge_accepted(self.config.reply_kind, id, &reply);
        // The reply itself travels in the clear; the game switches its
        // transport after reading it.
        self.emit(&accepted, out);
        let parameter = cipher::transport_parameter(reply.as_bytes());
        self.cipher = Some(Aes128::for_parameter(parameter));
        self.phase = Phase::Established;
        self.stats.answered += 1;
        out.events.push(Event::HandshakeAccepted { parameter });
    }

    fn answer(&mut self, request: &Envelope, incoming: Incoming, out: &mut Output) {
        let sender = request
            .recipient
            .clone()
            .unwrap_or_else(|| SDK_SERVICE.to_owned());
        let id = request.id;
        let kind = self.config.reply_kind;
        let (name, reply) = match incoming {
            Incoming::GetAuthCode { client_id, scope } => {
                if client_id.is_empty()
                    || client_id.len() > 256
                    || scope.len() > 1024
                    || client_id.chars().chain(scope.chars()).any(char::is_control)
                {
                    self.stats.malformed += 1;
                    out.events.push(Event::Malformed(Malformed::Message(
                        crate::message::Error::Field,
                    )));
                    return;
                }
                (
                    "GetAuthCode",
                    Envelope::auth_code(kind, &sender, id, &self.auth_code),
                )
            }
            Incoming::GetInternetConnectedState => (
                "GetInternetConnectedState",
                Envelope::internet_connected(kind, &sender, id, self.config.connected),
            ),
            Incoming::GetBlockList => (
                "GetBlockList",
                Envelope::empty_collection(kind, &sender, id, LocalCollection::Blocked),
            ),
            Incoming::QueryFriends { user_id } => {
                if user_id != self.config.profile.user_id {
                    self.unsupported("QueryFriends".into(), id, out);
                    return;
                }
                (
                    "QueryFriends",
                    Envelope::empty_collection(kind, &sender, id, LocalCollection::Friends),
                )
            }
            Incoming::QueryImage {
                image_id,
                width,
                height,
            } => {
                if image_id.len() > 1024
                    || image_id.chars().any(char::is_control)
                    || width > 4096
                    || height > 4096
                {
                    self.stats.malformed += 1;
                    out.events.push(Event::Malformed(Malformed::Message(
                        crate::message::Error::Field,
                    )));
                    return;
                }
                (
                    "QueryImage",
                    Envelope::empty_collection(kind, &sender, id, LocalCollection::Images),
                )
            }
            Incoming::QueryAchievements {
                user_id,
                persona_id,
                game_ids,
                ..
            } => {
                if user_id != self.config.profile.user_id
                    || persona_id != self.config.profile.persona_id
                {
                    self.unsupported("QueryAchievements".into(), id, out);
                    return;
                }
                if game_ids.len() > 32
                    || game_ids
                        .iter()
                        .any(|s| s.len() > 256 || s.chars().any(char::is_control))
                {
                    self.stats.malformed += 1;
                    out.events.push(Event::Malformed(Malformed::Message(
                        crate::message::Error::Field,
                    )));
                    return;
                }
                (
                    "QueryAchievements",
                    Envelope::empty_collection(kind, &sender, id, LocalCollection::Achievements),
                )
            }
            Incoming::GetPresence { user_id } => {
                if user_id != self.config.profile.user_id {
                    self.unsupported("GetPresence".into(), id, out);
                    return;
                }
                let mut presence = self.presence.clone();
                if !self.config.connected {
                    presence.state = PresenceState::Offline;
                }
                (
                    "GetPresence",
                    Envelope::presence(kind, &sender, id, &presence),
                )
            }
            Incoming::SetPresence(presence) => {
                if presence.user_id != self.config.profile.user_id {
                    self.unsupported("SetPresence".into(), id, out);
                    return;
                }
                if [
                    &presence.rich_presence,
                    &presence.game_presence,
                    &presence.session_id,
                ]
                .iter()
                .any(|value| value.len() > 1024 || value.chars().any(char::is_control))
                {
                    self.stats.malformed += 1;
                    out.events.push(Event::Malformed(Malformed::Message(
                        crate::message::Error::Field,
                    )));
                    return;
                }
                self.presence = presence;
                ("SetPresence", Envelope::error(kind, &sender, id, 0, ""))
            }
            Incoming::GetConfig => (
                "GetConfig",
                Envelope::config(kind, &sender, id, &self.config.services),
            ),
            Incoming::GetProfile { index: 0 } => (
                "GetProfile",
                Envelope::profile(kind, &sender, id, &self.config.profile),
            ),
            Incoming::GetProfile { .. } => {
                self.unsupported("GetProfile".into(), id, out);
                return;
            }
            Incoming::GetSetting { setting } => {
                let Some(value) = self
                    .config
                    .settings
                    .iter()
                    .find(|(s, _)| *s == setting)
                    .map(|(_, v)| v.as_str())
                else {
                    self.unsupported("GetSetting".into(), id, out);
                    return;
                };
                ("GetSetting", Envelope::setting(kind, &sender, id, value))
            }
            Incoming::GetGameInfo { info } => {
                let Some(value) = self
                    .config
                    .game_info
                    .iter()
                    .find(|(i, _)| *i == info)
                    .map(|(_, v)| v.as_str())
                else {
                    self.unsupported("GetGameInfo".into(), id, out);
                    return;
                };
                ("GetGameInfo", Envelope::game_info(kind, &sender, id, value))
            }
            Incoming::ChallengeResponse { .. } => {
                out.events.push(Event::Unexpected { id });
                return;
            }
            Incoming::Unknown { name } => {
                self.unsupported(name, id, out);
                return;
            }
        };
        self.emit(&reply, out);
        self.stats.answered += 1;
        out.events.push(Event::Answered { request: name, id });
    }

    fn unsupported(&mut self, name: String, id: u64, out: &mut Output) {
        self.stats.unsupported += 1;
        out.events.push(Event::Unsupported { name, id });
    }

    /// Serialize, transform when the handshake is done, and frame.
    fn emit(&mut self, envelope: &Envelope, out: &mut Output) {
        let xml = envelope.to_xml();
        let wire = match &self.cipher {
            None => xml.into_bytes(),
            Some(aes) => match aes.encode_hex(xml.as_bytes()) {
                Ok(hex) => hex.into_bytes(),
                Err(_) => return,
            },
        };
        if let Some(frame) = frame::encode(&wire) {
            self.stats.frames_out += 1;
            out.send.push(frame);
        }
    }
}

#[cfg(test)]
mod tests;
