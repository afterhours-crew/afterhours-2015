// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! LSX envelopes and typed startup payloads. Scalar readers select attributes
//! by exact name; serializers retain the required attribute/child distinction.
//!
//! The parser accepts GetProfile, GetSetting and GetGameInfo request fields as
//! attributes or child text. Callers select the envelope kind and routing;
//! this module supplies no handshake, configuration or authentication policy.

use crate::xml::{self, Element};
use std::fmt;

pub const ROOT: &str = "LSX";
pub const REQUEST: &str = "Request";
pub const RESPONSE: &str = "Response";
pub const EVENT: &str = "Event";
/// The launcher's identity in the handshake.
pub const LAUNCHER: &str = "EALS";
/// The SDK service the game addresses after the handshake.
pub const SDK_SERVICE: &str = "EbisuSDK";
/// The SDK version literal the game sends in its ChallengeResponse.
pub const GAME_SDK_VERSION: &str = "9.10.1.7";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Xml(xml::Error),
    /// The root is not `LSX`.
    Root,
    /// The envelope child is not `Request`, `Response` or `Event`, or is missing.
    Kind,
    /// The `id` attribute is not a decimal number.
    Id,
    /// The kind element carries no payload element.
    Payload,
    /// A payload field is missing or malformed.
    Field,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lsx message {self:?}")
    }
}
impl std::error::Error for Error {}

impl From<xml::Error> for Error {
    fn from(e: xml::Error) -> Self {
        Self::Xml(e)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Request,
    Response,
    Event,
}

impl Kind {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            REQUEST => Some(Self::Request),
            RESPONSE => Some(Self::Response),
            EVENT => Some(Self::Event),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Request => REQUEST,
            Self::Response => RESPONSE,
            Self::Event => EVENT,
        }
    }
}

/// One LSX message: the kind element with its routing attributes and one
/// payload element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope {
    pub kind: Kind,
    pub sender: Option<String>,
    pub recipient: Option<String>,
    /// Missing IDs parse as zero. Event serialization omits a zero ID;
    /// request/response serialization continues to emit it explicitly.
    pub id: u64,
    pub payload: Element,
}

impl Envelope {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let root = xml::parse(bytes)?;
        if root.name != ROOT {
            return Err(Error::Root);
        }
        let child = root.children.first().ok_or(Error::Kind)?;
        let kind = Kind::parse(&child.name).ok_or(Error::Kind)?;
        // The game's dispatcher substitutes "0" for a missing id.
        let id = child
            .attr("id")
            .unwrap_or("0")
            .parse::<u64>()
            .map_err(|_| Error::Id)?;
        let payload = child.children.first().cloned().ok_or(Error::Payload)?;
        Ok(Self {
            kind,
            sender: child.attr("sender").map(str::to_owned),
            recipient: child.attr("recipient").map(str::to_owned),
            id,
            payload,
        })
    }

    pub fn to_xml(&self) -> String {
        let mut kind = Element::new(self.kind.name());
        if let Some(sender) = &self.sender {
            kind = kind.attribute("sender", sender);
        }
        if let Some(recipient) = &self.recipient {
            kind = kind.attribute("recipient", recipient);
        }
        if self.kind != Kind::Event || self.id != 0 {
            kind = kind.attribute("id", &self.id.to_string());
        }
        Element::new(ROOT)
            .child(kind.child(self.payload.clone()))
            .to_xml()
    }

    /// A launcher message; the caller supplies its envelope kind and routing.
    fn reply(kind: Kind, sender: &str, id: u64, payload: Element) -> Self {
        Self {
            kind,
            sender: Some(sender.to_owned()),
            recipient: None,
            id,
            payload,
        }
    }

    /// The launcher's opening challenge from `EALS` carrying the
    /// 32-character hex key the game must transform.
    pub fn challenge(kind: Kind, id: u64, key: &str, version: &str, build: &str) -> Self {
        Self::reply(
            kind,
            LAUNCHER,
            id,
            Element::new("Challenge")
                .attribute("key", key)
                .attribute("version", version)
                .attribute("build", build),
        )
    }

    /// The launcher's reply to the game's `ChallengeResponse`: `response` is
    /// the game's key transformed under the parameter-zero key.
    pub fn challenge_accepted(kind: Kind, id: u64, response: &str) -> Self {
        Self::reply(
            kind,
            LAUNCHER,
            id,
            Element::new("ChallengeAccepted").attribute("response", response),
        )
    }

    /// `GetConfigResponse` with one `Service` per (name, facility).
    pub fn config(kind: Kind, sender: &str, id: u64, services: &[(String, Facility)]) -> Self {
        let mut payload = Element::new("GetConfigResponse");
        for (name, facility) in services {
            payload = payload.child(
                Element::new("Service")
                    .attribute("Name", name)
                    .attribute("Facility", facility.name()),
            );
        }
        Self::reply(kind, sender, id, payload)
    }

    pub fn profile(kind: Kind, sender: &str, id: u64, profile: &Profile) -> Self {
        Self::reply(
            kind,
            sender,
            id,
            Element::new("GetProfileResponse")
                .attribute("UserId", &profile.user_id.to_string())
                .attribute("PersonaId", &profile.persona_id.to_string())
                .attribute("Persona", &profile.persona)
                .attribute("AvatarId", &profile.avatar_id),
        )
    }

    pub fn setting(kind: Kind, sender: &str, id: u64, value: &str) -> Self {
        Self::reply(
            kind,
            sender,
            id,
            Element::new("GetSettingResponse").attribute("Setting", value),
        )
    }

    pub fn game_info(kind: Kind, sender: &str, id: u64, value: &str) -> Self {
        Self::reply(
            kind,
            sender,
            id,
            Element::new("GetGameInfoResponse").attribute("GameInfo", value),
        )
    }

    /// Numeric connectivity flag expected by the SDK's signed scalar reader.
    pub fn internet_connected(kind: Kind, sender: &str, id: u64, connected: bool) -> Self {
        Self::reply(
            kind,
            sender,
            id,
            Element::new("InternetConnectedState")
                .attribute("connected", if connected { "1" } else { "0" }),
        )
    }

    /// Locally generated authorization code; callers own authentication policy.
    pub fn auth_code(kind: Kind, sender: &str, id: u64, value: &str) -> Self {
        Self::reply(
            kind,
            sender,
            id,
            Element::new("AuthCode").attribute("value", value),
        )
    }

    /// Presence of the selected local account, with no advertised game session.
    pub fn presence(kind: Kind, sender: &str, id: u64, presence: &PresenceUpdate) -> Self {
        Self::reply(
            kind,
            sender,
            id,
            Element::new("GetPresenceResponse")
                .attribute("UserId", &presence.user_id.to_string())
                .attribute("Presence", presence.state.name())
                .attribute("Title", "")
                .attribute("TitleId", "")
                .attribute("MultiplayerId", "")
                .attribute("RichPresence", &presence.rich_presence)
                .attribute("GamePresence", &presence.game_presence)
                .attribute("SessionId", &presence.session_id),
        )
    }

    /// The error payload the game's callbacks fall back to.
    pub fn error(kind: Kind, sender: &str, id: u64, code: i32, description: &str) -> Self {
        Self::reply(
            kind,
            sender,
            id,
            Element::new("ErrorSuccess")
                .attribute("Code", &code.to_string())
                .attribute("Description", description),
        )
    }

    /// The local SDK has no remote friends, blocked users, avatar images or
    /// achievement catalog. Empty collections use the client's ordinary readers;
    /// no external records or downloaded resources are fabricated.
    pub fn empty_collection(
        kind: Kind,
        sender: &str,
        id: u64,
        collection: LocalCollection,
    ) -> Self {
        let payload = match collection {
            LocalCollection::Blocked => {
                Element::new("GetBlockListResponse").attribute("Return", "")
            }
            LocalCollection::Friends => Element::new("QueryFriendsResponse"),
            LocalCollection::Images => Element::new("QueryImageResponse").attribute("Result", "0"),
            LocalCollection::Achievements => Element::new("AchievementSets"),
        };
        Self::reply(kind, sender, id, payload)
    }

    /// The typed meaning of a request's payload, from the game's point of view.
    pub fn incoming(&self) -> Result<Incoming, Error> {
        let p = &self.payload;
        let field = |name: &str| -> Result<String, Error> {
            p.attr(name)
                .map(str::to_owned)
                .or_else(|| p.first(name).map(|e| e.text.clone()))
                .ok_or(Error::Field)
        };
        Ok(match p.name.as_str() {
            "ChallengeResponse" => Incoming::ChallengeResponse {
                response: p.attr("response").ok_or(Error::Field)?.to_owned(),
                key: p.attr("key").ok_or(Error::Field)?.to_owned(),
                content_id: field("ContentId")?,
                title: field("Title")?,
                multiplayer_id: field("MultiplayerId")?,
                language: field("Language")?,
                version: field("Version")?,
            },
            "GetAuthCode" => Incoming::GetAuthCode {
                client_id: field("ClientId")?,
                scope: field("Scope")?,
            },
            "GetInternetConnectedState" => Incoming::GetInternetConnectedState,
            "GetBlockList" => Incoming::GetBlockList,
            "QueryFriends" => Incoming::QueryFriends {
                user_id: field("UserId")?.parse().map_err(|_| Error::Field)?,
            },
            "QueryImage" => Incoming::QueryImage {
                image_id: field("ImageId")?,
                width: field("Width")?.parse().map_err(|_| Error::Field)?,
                height: field("Height")?.parse().map_err(|_| Error::Field)?,
            },
            "QueryAchievements" => Incoming::QueryAchievements {
                user_id: field("UserId")?.parse().map_err(|_| Error::Field)?,
                persona_id: field("PersonaId")?.parse().map_err(|_| Error::Field)?,
                all: match field("All")?.as_str() {
                    "true" | "1" => true,
                    "false" | "0" => false,
                    _ => return Err(Error::Field),
                },
                game_ids: p
                    .children
                    .iter()
                    .filter(|c| c.name == "GameId")
                    .map(|c| c.text.clone())
                    .collect(),
            },
            "GetPresence" => Incoming::GetPresence {
                user_id: field("UserId")?.parse().map_err(|_| Error::Field)?,
            },
            "SetPresence" => Incoming::SetPresence(PresenceUpdate {
                user_id: field("UserId")?.parse().map_err(|_| Error::Field)?,
                state: PresenceState::parse(&field("Presence")?).ok_or(Error::Field)?,
                rich_presence: field("RichPresence")?,
                game_presence: field("GamePresence")?,
                session_id: field("SessionId")?,
            }),
            "GetConfig" => Incoming::GetConfig,
            "GetProfile" => Incoming::GetProfile {
                index: field("index")?.parse().map_err(|_| Error::Field)?,
            },
            "GetSetting" => Incoming::GetSetting {
                setting: Setting::parse(&field("SettingId")?).ok_or(Error::Field)?,
            },
            "GetGameInfo" => Incoming::GetGameInfo {
                info: GameInfo::parse(&field("GameInfoId")?).ok_or(Error::Field)?,
            },
            other => Incoming::Unknown {
                name: other.to_owned(),
            },
        })
    }
}

/// The game's requests the launcher side understands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Incoming {
    ChallengeResponse {
        response: String,
        key: String,
        content_id: String,
        title: String,
        multiplayer_id: String,
        language: String,
        version: String,
    },
    GetConfig,
    GetInternetConnectedState,
    GetBlockList,
    QueryFriends {
        user_id: u64,
    },
    QueryImage {
        image_id: String,
        width: u32,
        height: u32,
    },
    QueryAchievements {
        user_id: u64,
        persona_id: u64,
        all: bool,
        game_ids: Vec<String>,
    },
    GetPresence {
        user_id: u64,
    },
    SetPresence(PresenceUpdate),
    GetAuthCode {
        client_id: String,
        scope: String,
    },
    GetProfile {
        index: u64,
    },
    GetSetting {
        setting: Setting,
    },
    GetGameInfo {
        info: GameInfo,
    },
    Unknown {
        name: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalCollection {
    Blocked,
    Friends,
    Images,
    Achievements,
}

/// The SDK identity the launcher reports. Local values, not EA ids.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile {
    pub user_id: u64,
    pub persona_id: u64,
    pub persona: String,
    pub avatar_id: String,
}

/// Ephemeral presence supplied by one local SDK connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresenceUpdate {
    pub user_id: u64,
    pub state: PresenceState,
    pub rich_presence: String,
    pub game_presence: String,
    pub session_id: String,
}

impl PresenceUpdate {
    pub fn online(user_id: u64) -> Self {
        Self {
            user_id,
            state: PresenceState::Online,
            rich_presence: String::new(),
            game_presence: String::new(),
            session_id: String::new(),
        }
    }
}

macro_rules! names {
    ($(#[$doc:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
        pub enum $name {
            $($variant),+
        }
        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),+];
            /// The exact table string.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
            pub fn parse(text: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.name() == text)
            }
        }
    };
}

names! {
    /// Presence enum names in the SDK table's order.
    PresenceState {
        Offline = "OFFLINE", Online = "ONLINE", InGame = "INGAME", Busy = "BUSY",
        Idle = "IDLE", Joinable = "JOINABLE", JoinableInviteOnly = "JOINABLE_INVITE_ONLY",
    }
}

names! {
    /// The 34 `Facility` names of the configuration services, in table order.
    Facility {
        Sdk = "SDK", Profile = "PROFILE", Presence = "PRESENCE", Friends = "FRIENDS",
        Commerce = "COMMERCE", RecentPlayer = "RECENTPLAYER", Igo = "IGO", Misc = "MISC",
        Login = "LOGIN", Utility = "UTILITY", Xmpp = "XMPP", Chat = "CHAT",
        IgoEvent = "IGO_EVENT", EalsEvents = "EALS_EVENTS", LoginEvent = "LOGIN_EVENT",
        InviteEvent = "INVITE_EVENT", ProfileEvent = "PROFILE_EVENT",
        PresenceEvent = "PRESENCE_EVENT", FriendsEvent = "FRIENDS_EVENT",
        CommerceEvent = "COMMERCE_EVENT", ChatEvent = "CHAT_EVENT",
        DownloadEvent = "DOWNLOAD_EVENT", Permission = "PERMISSION", Resources = "RESOURCES",
        BlockedUsers = "BLOCKED_USERS", BlockedUserEvent = "BLOCKED_USER_EVENT",
        GetUserId = "GET_USERID", OnlineStatusEvent = "ONLINE_STATUS_EVENT",
        Achievement = "ACHIEVEMENT", AchievementEvent = "ACHIEVEMENT_EVENT",
        BroadcastEvent = "BROADCAST_EVENT", ProgressiveInstallation = "PROGRESSIVE_INSTALLATION",
        ProgressiveInstallationEvent = "PROGRESSIVE_INSTALLATION_EVENT", Content = "CONTENT",
    }
}

names! {
    /// The five `SettingId` names.
    Setting {
        Language = "LANGUAGE", Environment = "ENVIRONMENT", IgoAvailable = "IS_IGO_AVAILABLE",
        IgoEnabled = "IS_IGO_ENABLED", TelemetryEnabled = "IS_TELEMETRY_ENABLED",
    }
}

names! {
    /// The ten `GameInfoId` names in string-index order.
    GameInfo {
        Invalid = "INVALID", UpToDate = "UPTODATE", Languages = "LANGUAGES",
        FreeTrial = "FREETRIAL", Expiration = "EXPIRATION",
        ExpirationDuration = "EXPIRATION_DURATION", InstalledVersion = "INSTALLED_VERSION",
        InstalledLanguage = "INSTALLED_LANGUAGE", AvailableVersion = "AVAILABLE_VERSION",
        DisplayName = "DISPLAY_NAME",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncorrelated_challenge_event_round_trips_across_frame_splits() {
        // Constructed values: no captured challenge or launcher build is used.
        let xml = "<LSX><Event sender=\"EALS\"><Challenge key=\"0123456789abcdef0123456789abcdef\" version=\"1,2,3,4\" build=\"test\"/></Event></LSX>";
        let expected = Envelope::challenge(
            Kind::Event,
            0,
            "0123456789abcdef0123456789abcdef",
            "1,2,3,4",
            "test",
        );
        assert_eq!(Envelope::parse(xml.as_bytes()).unwrap(), expected);
        assert_eq!(expected.to_xml(), xml);
        let wire = crate::frame::encode(xml.as_bytes()).unwrap();
        for split in 0..=wire.len() {
            let mut frames = crate::frame::Frames::new();
            frames.feed(&wire[..split]).unwrap();
            frames.feed(&wire[split..]).unwrap();
            assert_eq!(
                Envelope::parse(&frames.next_frame().unwrap()).unwrap(),
                expected
            );
            assert!(frames.next_frame().is_none());
        }
    }

    #[test]
    fn event_routing_and_nonzero_ids_survive_without_changing_request_ids() {
        let xml = "<LSX><Event sender=\"local\" recipient=\"peer\" id=\"27\"><Notice value=\"x\"/></Event></LSX>";
        let event = Envelope::parse(xml.as_bytes()).unwrap();
        assert_eq!((event.kind, event.id), (Kind::Event, 27));
        assert_eq!(event.to_xml(), xml);
        assert_eq!(Kind::parse("event"), None);
        assert_eq!(Kind::parse("Event"), Some(Kind::Event));
        for kind in [Kind::Request, Kind::Response] {
            let envelope = Envelope::challenge(kind, 0, "key", "version", "build");
            assert!(envelope.to_xml().contains("id=\"0\""));
            assert_eq!(
                Envelope::parse(envelope.to_xml().as_bytes()).unwrap(),
                envelope
            );
        }
        assert_eq!(
            Envelope::parse(b"<LSX><Event id=\"bad\"><X/></Event></LSX>"),
            Err(Error::Id)
        );
        assert_eq!(Envelope::parse(b"<LSX><Event/></LSX>"), Err(Error::Payload));
    }

    #[test]
    fn enum_tables_keep_the_observed_order_and_spellings() {
        assert_eq!(Facility::ALL.len(), 34);
        assert_eq!(Facility::ALL[0], Facility::Sdk);
        assert_eq!(Facility::ALL[1], Facility::Profile);
        assert_eq!(Facility::ALL[33].name(), "CONTENT");
        assert_eq!(Facility::parse("PROFILE"), Some(Facility::Profile));
        assert_eq!(Facility::parse("profile"), None, "case-sensitive");
        assert_eq!(Setting::ALL.len(), 5);
        assert_eq!(Setting::ALL[3], Setting::IgoEnabled);
        assert_eq!(GameInfo::ALL.len(), 10);
        assert_eq!(GameInfo::ALL[3], GameInfo::FreeTrial);
        assert_eq!(GameInfo::parse("DISPLAY_NAME"), Some(GameInfo::DisplayName));
    }

    #[test]
    fn parses_the_game_requests_and_serializes_the_launcher_replies() {
        let xml = "<LSX><Request recipient=\"EALS\" id=\"1\"><ChallengeResponse response=\"ab\" key=\"cd\"><ContentId>c</ContentId><Title>t</Title><MultiplayerId>m</MultiplayerId><Language>en_US</Language><Version>9.10.1.7</Version></ChallengeResponse></Request></LSX>";
        let e = Envelope::parse(xml.as_bytes()).unwrap();
        assert_eq!(
            (e.kind, e.id, e.recipient.as_deref()),
            (Kind::Request, 1, Some("EALS"))
        );
        assert_eq!(
            e.incoming().unwrap(),
            Incoming::ChallengeResponse {
                response: "ab".into(),
                key: "cd".into(),
                content_id: "c".into(),
                title: "t".into(),
                multiplayer_id: "m".into(),
                language: "en_US".into(),
                version: GAME_SDK_VERSION.into(),
            }
        );
        assert_eq!(Envelope::parse(xml.as_bytes()).unwrap().to_xml(), xml);
        let get_config = Envelope::parse(
            b"<LSX><Request recipient=\"EbisuSDK\" id=\"2\"><GetConfig/></Request></LSX>",
        )
        .unwrap();
        assert_eq!(get_config.incoming().unwrap(), Incoming::GetConfig);
        for xml in [
            "<LSX><Request id=\"3\"><GetProfile index=\"0\"/></Request></LSX>",
            "<LSX><Request id=\"3\"><GetProfile><index>0</index></GetProfile></Request></LSX>",
        ] {
            let e = Envelope::parse(xml.as_bytes()).unwrap();
            assert_eq!(e.incoming().unwrap(), Incoming::GetProfile { index: 0 });
        }
        let e = Envelope::parse(
            b"<LSX><Request id=\"4\"><GetSetting SettingId=\"IS_IGO_ENABLED\"/></Request></LSX>",
        )
        .unwrap();
        assert_eq!(
            e.incoming().unwrap(),
            Incoming::GetSetting {
                setting: Setting::IgoEnabled
            }
        );
        let e = Envelope::parse(
            b"<LSX><Request id=\"5\"><GetGameInfo GameInfoId=\"FREETRIAL\"/></Request></LSX>",
        )
        .unwrap();
        assert_eq!(
            e.incoming().unwrap(),
            Incoming::GetGameInfo {
                info: GameInfo::FreeTrial
            }
        );
        let e = Envelope::parse(b"<LSX><Request><Whatever/></Request></LSX>").unwrap();
        assert_eq!(
            e.id, 0,
            "a missing id reads as 0 like the game's dispatcher"
        );
        assert_eq!(
            e.incoming().unwrap(),
            Incoming::Unknown {
                name: "Whatever".into()
            }
        );

        let challenge = Envelope::challenge(Kind::Request, 1, "0123", "10.5", "77");
        assert_eq!(
            challenge.to_xml(),
            "<LSX><Request sender=\"EALS\" id=\"1\"><Challenge key=\"0123\" version=\"10.5\" build=\"77\"/></Request></LSX>"
        );
        let services = vec![
            ("EbisuSDK".to_owned(), Facility::Sdk),
            ("EbisuSDK".to_owned(), Facility::Profile),
        ];
        assert_eq!(
            Envelope::config(Kind::Response, SDK_SERVICE, 2, &services).to_xml(),
            "<LSX><Response sender=\"EbisuSDK\" id=\"2\"><GetConfigResponse><Service Name=\"EbisuSDK\" Facility=\"SDK\"/><Service Name=\"EbisuSDK\" Facility=\"PROFILE\"/></GetConfigResponse></Response></LSX>"
        );
        let profile = Profile {
            user_id: 268_435_463,
            persona_id: 536_870_919,
            persona: "Offline Driver".into(),
            avatar_id: "".into(),
        };
        assert_eq!(
            Envelope::profile(Kind::Response, SDK_SERVICE, 3, &profile).to_xml(),
            "<LSX><Response sender=\"EbisuSDK\" id=\"3\"><GetProfileResponse UserId=\"268435463\" PersonaId=\"536870919\" Persona=\"Offline Driver\" AvatarId=\"\"/></Response></LSX>"
        );
        assert!(
            Envelope::setting(Kind::Response, SDK_SERVICE, 4, "false")
                .to_xml()
                .contains("Setting=\"false\"")
        );
        assert!(
            Envelope::game_info(Kind::Response, SDK_SERVICE, 5, "false")
                .to_xml()
                .contains("GameInfo=\"false\"")
        );
        assert!(
            Envelope::error(Kind::Response, SDK_SERVICE, 6, -1, "x")
                .to_xml()
                .contains("<ErrorSuccess Code=\"-1\" Description=\"x\"/>")
        );
        assert_eq!(
            Envelope::challenge_accepted(Kind::Response, 1, "ff").to_xml(),
            "<LSX><Response sender=\"EALS\" id=\"1\"><ChallengeAccepted response=\"ff\"/></Response></LSX>"
        );
    }

    #[test]
    fn rejects_bad_envelopes_and_fields() {
        assert_eq!(Envelope::parse(b"<lsx><Request/></lsx>"), Err(Error::Root));
        assert_eq!(Envelope::parse(b"<LSX/>"), Err(Error::Kind));
        assert_eq!(
            Envelope::parse(b"<LSX><Other id=\"1\"><X/></Other></LSX>"),
            Err(Error::Kind)
        );
        assert_eq!(
            Envelope::parse(b"<LSX><Request id=\"x\"><X/></Request></LSX>"),
            Err(Error::Id)
        );
        assert_eq!(
            Envelope::parse(b"<LSX><Request id=\"-1\"><X/></Request></LSX>"),
            Err(Error::Id)
        );
        assert_eq!(
            Envelope::parse(b"<LSX><Request id=\"1\"/></LSX>"),
            Err(Error::Payload)
        );
        assert!(matches!(
            Envelope::parse(b"<LSX><Request>"),
            Err(Error::Xml(_))
        ));
        let e = Envelope::parse(
            b"<LSX><Request id=\"1\"><ChallengeResponse key=\"k\"/></Request></LSX>",
        )
        .unwrap();
        assert_eq!(
            e.incoming(),
            Err(Error::Field),
            "response attribute missing"
        );
        let e = Envelope::parse(
            b"<LSX><Request id=\"1\"><GetSetting SettingId=\"NOPE\"/></Request></LSX>",
        )
        .unwrap();
        assert_eq!(e.incoming(), Err(Error::Field));
        let e =
            Envelope::parse(b"<LSX><Request id=\"1\"><GetProfile index=\"x\"/></Request></LSX>")
                .unwrap();
        assert_eq!(e.incoming(), Err(Error::Field));
    }

    #[test]
    fn authorization_code_codec_preserves_routing_and_exact_scalar_name() {
        let request = Envelope::parse(br#"<LSX><Request recipient="LocalLogin" id="42"><GetAuthCode ClientId="local-client" Scope=""/></Request></LSX>"#).unwrap();
        assert_eq!(
            request.incoming().unwrap(),
            Incoming::GetAuthCode {
                client_id: "local-client".into(),
                scope: String::new()
            }
        );
        assert_eq!(
            Envelope::auth_code(Kind::Response, "LocalLogin", 42, "a&b").to_xml(),
            "<LSX><Response sender=\"LocalLogin\" id=\"42\"><AuthCode value=\"a&amp;b\"/></Response></LSX>"
        );
        let missing =
            Envelope::parse(br#"<LSX><Request><GetAuthCode Scope=""/></Request></LSX>"#).unwrap();
        assert_eq!(missing.incoming(), Err(Error::Field));
    }

    #[test]
    fn connectivity_uses_the_numeric_attribute_expected_by_the_client() {
        let request = Envelope::parse(
            br#"<LSX><Request id="10"><GetInternetConnectedState/></Request></LSX>"#,
        )
        .unwrap();
        assert_eq!(
            request.incoming().unwrap(),
            Incoming::GetInternetConnectedState
        );
        for (value, text) in [(true, "1"), (false, "0")] {
            let reply = Envelope::internet_connected(Kind::Response, SDK_SERVICE, 10, value);
            assert_eq!(reply.payload.name, "InternetConnectedState");
            assert_eq!(reply.payload.attr("connected"), Some(text));
            assert!(reply.payload.children.is_empty());
        }
    }

    #[test]
    fn presence_requires_a_numeric_user_and_emits_named_status() {
        for value in ["", "-1", "18446744073709551616", "other"] {
            let input = format!("<LSX><Request><GetPresence UserId=\"{value}\"/></Request></LSX>");
            assert_eq!(
                Envelope::parse(input.as_bytes()).unwrap().incoming(),
                Err(Error::Field)
            );
        }
        let input = br#"<LSX><Request id="12"><GetPresence UserId="9001"/></Request></LSX>"#;
        assert_eq!(
            Envelope::parse(input).unwrap().incoming().unwrap(),
            Incoming::GetPresence { user_id: 9001 }
        );
        assert_eq!(
            Envelope::presence(
                Kind::Response,
                SDK_SERVICE,
                12,
                &PresenceUpdate::online(9001)
            )
            .to_xml(),
            "<LSX><Response sender=\"EbisuSDK\" id=\"12\"><GetPresenceResponse UserId=\"9001\" Presence=\"ONLINE\" Title=\"\" TitleId=\"\" MultiplayerId=\"\" RichPresence=\"\" GamePresence=\"\" SessionId=\"\"/></Response></LSX>"
        );
    }

    #[test]
    fn presence_update_parses_exact_enum_and_required_fields() {
        for status in PresenceState::ALL {
            let input = format!(
                "<LSX><Request><SetPresence UserId=\"9001\" Presence=\"{}\" RichPresence=\"Drive &amp; race\" GamePresence=\"\" SessionId=\"\"/></Request></LSX>",
                status.name()
            );
            let Incoming::SetPresence(value) = Envelope::parse(input.as_bytes())
                .unwrap()
                .incoming()
                .unwrap()
            else {
                panic!("wrong request")
            };
            assert_eq!(value.state, *status);
            assert_eq!(value.rich_presence, "Drive & race");
        }
        for input in [
            r#"<LSX><Request><SetPresence UserId="1" Presence="online" RichPresence="" GamePresence="" SessionId=""/></Request></LSX>"#,
            r#"<LSX><Request><SetPresence UserId="1" Presence="ONLINE" RichPresence="" GamePresence=""/></Request></LSX>"#,
        ] {
            assert_eq!(
                Envelope::parse(input.as_bytes()).unwrap().incoming(),
                Err(Error::Field)
            );
        }
    }
}
