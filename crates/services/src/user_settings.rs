// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed per-account Util settings and Autolog preferences. Reads use supplied
//! owned state; writes produce a per-key change. The socket edge must refresh
//! from the account store before each request, persist the pending change, then
//! send the reply. A failed reply after commit leaves the durable value intact;
//! repeating the same save is idempotent. A single-key read of a key the
//! account does not hold answers `UTIL_USS_RECORD_NOT_FOUND`; nothing is
//! stored and no default value is invented.
use nfs_fire2::{Fields, Frame, HEADER_LEN};
use nfs_protocol::{autolog, util};
use serde_json::{Value as Json, json};
use std::{fmt, path::Path};
mod store;
pub use store::Store;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Config,
    Bounds,
    Ineligible,
    Encode,
    Storage,
    Conflict,
    Phase,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "settings {self:?}")
    }
}
impl std::error::Error for Error {}

pub const FORMAT: &str = "nfs-user-settings";
pub const VERSION: u64 = 1;
/// Entries per map and bytes per key/value: local bounds, not protocol limits.
pub const MAX_ENTRIES: usize = 64;
pub const MAX_KEY_BYTES: usize = 128;
pub const MAX_STRING_BYTES: usize = 1023;
pub const MAX_BODY_BYTES: usize = 16 * 1024;

pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: MAX_BODY_BYTES,
        max_depth: 2,
        max_values: 4 + 6 * MAX_ENTRIES,
        max_collection: MAX_ENTRIES,
        max_byte_string: MAX_STRING_BYTES + 1,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(MAX_BODY_BYTES + HEADER_LEN, 0, MAX_BODY_BYTES).expect("constant limits")
}

/// Routes this service answers when configured.
pub fn owns(component: u16, command: u16) -> bool {
    matches!(
        (component, command),
        (
            util::COMPONENT,
            util::USER_SETTINGS_LOAD_ALL | util::USER_SETTINGS_LOAD | util::USER_SETTINGS_SAVE
        ) | (
            autolog::COMPONENT,
            autolog::GET_USER_SETTINGS | autolog::SET_USER_SETTINGS
        )
    )
}

/// One account's settings. Keys are unique per map; order is insertion order
/// (the wire order of the content, then new keys).
#[derive(Clone, Default, Eq, PartialEq)]
pub struct Settings {
    pub strings: Vec<(Vec<u8>, Vec<u8>)>,
    pub integers: Vec<(Vec<u8>, u32)>,
    pub floats: Vec<(Vec<u8>, u32)>,
}
impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("strings", &self.strings.len())
            .field("integers", &self.integers.len())
            .field("floats", &self.floats.len())
            .finish()
    }
}

fn check_key(key: &[u8]) -> Result<(), Error> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES || key.contains(&0) {
        return Err(Error::Config);
    }
    Ok(())
}
fn unique<T>(entries: &[(Vec<u8>, T)]) -> Result<(), Error> {
    if entries.len() > MAX_ENTRIES {
        return Err(Error::Bounds);
    }
    for (i, (key, _)) in entries.iter().enumerate() {
        check_key(key)?;
        if entries[..i].iter().any(|(k, _)| k == key) {
            return Err(Error::Config);
        }
    }
    Ok(())
}
fn set<T: PartialEq>(entries: &mut Vec<(Vec<u8>, T)>, key: &[u8], value: T) -> Result<bool, Error> {
    check_key(key)?;
    if let Some(entry) = entries.iter_mut().find(|(k, _)| k == key) {
        if entry.1 == value {
            return Ok(false);
        }
        entry.1 = value;
        return Ok(true);
    }
    if entries.len() >= MAX_ENTRIES {
        return Err(Error::Bounds);
    }
    entries.push((key.to_vec(), value));
    Ok(true)
}
fn bytes_json(bytes: &[u8]) -> Json {
    match std::str::from_utf8(bytes) {
        Ok(text) if !bytes.contains(&0) => json!(text),
        _ => json!({"hex": bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()}),
    }
}
fn bytes_from_json(v: &Json) -> Result<Vec<u8>, Error> {
    if let Some(text) = v.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    let hex = v["hex"].as_str().ok_or(Error::Config)?;
    if hex.len() % 2 != 0 || !hex.is_ascii() || hex.len() > 2 * MAX_STRING_BYTES {
        return Err(Error::Config);
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| Error::Config))
        .collect()
}

impl Settings {
    pub fn validate(&self) -> Result<(), Error> {
        unique(&self.strings)?;
        unique(&self.integers)?;
        unique(&self.floats)?;
        if self.strings.iter().any(|(_, v)| v.len() > MAX_STRING_BYTES) {
            return Err(Error::Bounds);
        }
        Ok(())
    }
    pub fn from_json(v: &Json) -> Result<Self, Error> {
        if !v.as_object().is_some_and(|o| o.len() == 5) {
            return Err(Error::Config);
        }
        if v["format"] != FORMAT || v["version"] != VERSION {
            return Err(Error::Config);
        }
        let pairs = |name: &str| -> Result<Vec<(Vec<u8>, &Json)>, Error> {
            v[name]
                .as_array()
                .filter(|a| a.len() <= MAX_ENTRIES)
                .ok_or(Error::Config)?
                .iter()
                .map(|pair| {
                    let pair = pair
                        .as_array()
                        .filter(|p| p.len() == 2)
                        .ok_or(Error::Config)?;
                    Ok((bytes_from_json(&pair[0])?, &pair[1]))
                })
                .collect()
        };
        let word = |v: &Json| -> Result<u32, Error> {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or(Error::Config)
        };
        let settings = Self {
            strings: pairs("strings")?
                .into_iter()
                .map(|(k, v)| Ok((k, bytes_from_json(v)?)))
                .collect::<Result<_, Error>>()?,
            integers: pairs("integers")?
                .into_iter()
                .map(|(k, v)| Ok((k, word(v)?)))
                .collect::<Result<_, Error>>()?,
            floats: pairs("floats")?
                .into_iter()
                .map(|(k, v)| Ok((k, word(v)?)))
                .collect::<Result<_, Error>>()?,
        };
        settings.validate()?;
        Ok(settings)
    }
    pub fn to_json(&self) -> Json {
        json!({"format": FORMAT, "version": VERSION,
            "strings": self.strings.iter().map(|(k, v)| json!([bytes_json(k), bytes_json(v)])).collect::<Vec<_>>(),
            "integers": self.integers.iter().map(|(k, v)| json!([bytes_json(k), v])).collect::<Vec<_>>(),
            "floats": self.floats.iter().map(|(k, v)| json!([bytes_json(k), v])).collect::<Vec<_>>()})
    }
    pub fn load(path: &Path) -> Result<Self, Error> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| Error::Storage)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        if bytes.len() > 1024 * 1024 {
            return Err(Error::Bounds);
        }
        Self::from_json(&serde_json::from_slice(&bytes).map_err(|_| Error::Config)?)
    }
    /// Set one string setting; `Ok(true)` when the value changed.
    pub fn set_string(&mut self, key: &[u8], value: &[u8]) -> Result<bool, Error> {
        if value.len() > MAX_STRING_BYTES {
            return Err(Error::Bounds);
        }
        set(&mut self.strings, key, value.to_vec())
    }
    pub fn set_integer(&mut self, key: &[u8], value: u32) -> Result<bool, Error> {
        set(&mut self.integers, key, value)
    }
    pub fn set_float_bits(&mut self, key: &[u8], bits: u32) -> Result<bool, Error> {
        set(&mut self.floats, key, bits)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum Change {
    String(Vec<u8>, Vec<u8>),
    Preferences(Vec<(Vec<u8>, u32)>, Vec<(Vec<u8>, u32)>),
}
impl fmt::Debug for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(..) => f.write_str("Change::String(..)"),
            Self::Preferences(ints, floats) => f
                .debug_struct("Change::Preferences")
                .field("integers", &ints.len())
                .field("floats", &floats.len())
                .finish(),
        }
    }
}

/// One connection's view: the account's settings plus at most one staged write.
pub struct Session {
    persona: i64,
    settings: Settings,
    staged: Option<Change>,
}
impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("user_settings::Session")
            .field("staged", &self.staged.is_some())
            .finish_non_exhaustive()
    }
}

fn frame(wire: &[u8], route: (u16, u16)) -> Result<Frame<'_>, Error> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Error::Ineligible)?
        .ok_or(Error::Ineligible)?;
    let f = d.frame;
    if d.consumed != wire.len()
        || (f.fields.routing_a, f.fields.routing_b) != route
        || f.fields.category != 0
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || !f.metadata.is_empty()
    {
        return Err(Error::Ineligible);
    }
    Ok(f)
}
fn response(f: &Frame<'_>, body: &[u8]) -> Result<Vec<u8>, Error> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                category: 1,
                ..f.fields
            },
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .map_err(|_| Error::Encode)
}

impl Session {
    pub fn new(persona: i64, settings: Settings) -> Result<Self, Error> {
        settings.validate()?;
        if persona <= 0 {
            return Err(Error::Config);
        }
        Ok(Self {
            persona,
            settings,
            staged: None,
        })
    }
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    /// Apply the staged write; `true` when the settings changed.
    pub fn commit(&mut self) -> Result<bool, Error> {
        match self.staged.take() {
            Some(change) => change.apply(&mut self.settings),
            None => Ok(false),
        }
    }
    pub fn pending(&self) -> Option<Change> {
        self.staged.clone()
    }
    /// Replace a view before a request, or after its durable write. The edge
    /// must persist the pending change before calling this for a save.
    pub fn refresh(&mut self, settings: Settings) -> Result<(), Error> {
        settings.validate()?;
        self.settings = settings;
        self.staged = None;
        Ok(())
    }
    pub fn abort(&mut self) {
        self.staged = None;
    }

    /// `Err(Ineligible)` for frames that are not well-formed requests on
    /// an owned route; `Ok(None)` for unobserved shapes (no reply, no state).
    pub fn reply(&mut self, wire: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        if self.staged.is_some() {
            self.abort();
            return Err(Error::Phase);
        }
        let route = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Ineligible)?
            .ok_or(Error::Ineligible)?
            .frame
            .fields;
        let route = (route.routing_a, route.routing_b);
        if !owns(route.0, route.1) {
            return Err(Error::Ineligible);
        }
        self.staged = None;
        let f = frame(wire, route)?;
        let limits = body_limits();
        match route {
            (util::COMPONENT, util::USER_SETTINGS_LOAD_ALL) => {
                if !f.body.is_empty() {
                    return Err(Error::Ineligible);
                }
                let body = util::UserSettingsLoadAllResponse {
                    data_map: Some(util::ConfigEntries(
                        self.settings
                            .strings
                            .iter()
                            .map(|(k, v)| (k.as_slice(), v.as_slice()))
                            .collect(),
                    )),
                    ..Default::default()
                }
                .encode(limits)
                .map_err(|_| Error::Encode)?;
                response(&f, &body).map(Some)
            }
            (util::COMPONENT, util::USER_SETTINGS_LOAD) => {
                let q = util::UserSettingsLoadRequest::decode(f.body, limits)
                    .map_err(|_| Error::Ineligible)?;
                if q.unknown_field_count() != 0
                    || q.encode(limits).map_err(|_| Error::Ineligible)? != f.body
                {
                    return Err(Error::Ineligible);
                }
                let Some(key) = q.key.filter(|k| !k.is_empty()) else {
                    return Ok(None);
                };
                if q.user_id != Some(0) {
                    return Ok(None);
                }
                let Some((key, value)) = self.settings.strings.iter().find(|(k, _)| k == key)
                else {
                    return crate::error_reply(
                        f.fields,
                        nfs_protocol::metadata::UTIL_USS_RECORD_NOT_FOUND,
                    )
                    .map(Some)
                    .ok_or(Error::Encode);
                };
                let body = util::UserSettingsResponse {
                    key: Some(key),
                    data: Some(value),
                    ..Default::default()
                }
                .encode(limits)
                .map_err(|_| Error::Encode)?;
                response(&f, &body).map(Some)
            }
            (util::COMPONENT, util::USER_SETTINGS_SAVE) => {
                let q = util::UserSettingsSaveRequest::decode(f.body, limits)
                    .map_err(|_| Error::Ineligible)?;
                if q.unknown_field_count() != 0
                    || q.encode(limits).map_err(|_| Error::Ineligible)? != f.body
                {
                    return Err(Error::Ineligible);
                }
                let (Some(data), Some(key), Some(0)) = (q.data, q.key, q.user_id) else {
                    return Ok(None);
                };
                if key.is_empty() || key.len() > MAX_KEY_BYTES || data.len() > MAX_STRING_BYTES {
                    return Ok(None);
                }
                // Validate the write before answering; a full map is a refusal.
                let mut probe = self.settings.clone();
                probe.set_string(key, data)?;
                self.staged = Some(Change::String(key.to_vec(), data.to_vec()));
                response(&f, &[]).map(Some)
            }
            (autolog::COMPONENT, autolog::GET_USER_SETTINGS) => {
                let q = autolog::UserSettingsRequest::decode(f.body, limits)
                    .map_err(|_| Error::Ineligible)?;
                if q.unknown_field_count() != 0
                    || q.encode(limits).map_err(|_| Error::Ineligible)? != f.body
                {
                    return Err(Error::Ineligible);
                }
                if q.blaze_id != Some(self.persona) {
                    return Ok(None);
                }
                let body = autolog::UserSettingsResponse {
                    blaze_id: Some(self.persona),
                    settings_flt: Some(autolog::SettingsFloatBits(
                        self.settings
                            .floats
                            .iter()
                            .map(|(k, v)| (k.as_slice(), *v))
                            .collect(),
                    )),
                    settings_int: Some(autolog::SettingsIntegers(
                        self.settings
                            .integers
                            .iter()
                            .map(|(k, v)| (k.as_slice(), *v))
                            .collect(),
                    )),
                    ..Default::default()
                }
                .encode(limits)
                .map_err(|_| Error::Encode)?;
                response(&f, &body).map(Some)
            }
            (autolog::COMPONENT, autolog::SET_USER_SETTINGS) => {
                let q = autolog::UserSettingsUpdateRequest::decode(f.body, limits)
                    .map_err(|_| Error::Ineligible)?;
                if q.unknown_field_count() != 0
                    || q.encode(limits).map_err(|_| Error::Ineligible)? != f.body
                {
                    return Err(Error::Ineligible);
                }
                if q.blaze_id != Some(self.persona) {
                    return Ok(None);
                }
                let integers: Vec<(Vec<u8>, u32)> = q
                    .settings_int
                    .map(|m| m.0.iter().map(|(k, v)| (k.to_vec(), *v)).collect())
                    .unwrap_or_default();
                let floats: Vec<(Vec<u8>, u32)> = q
                    .settings_flt
                    .map(|m| m.0.iter().map(|(k, v)| (k.to_vec(), *v)).collect())
                    .unwrap_or_default();
                if integers.is_empty() && floats.is_empty() {
                    return Ok(None);
                }
                unique(&integers)?;
                unique(&floats)?;
                let mut probe = self.settings.clone();
                for (key, value) in &integers {
                    probe.set_integer(key, *value)?;
                }
                for (key, bits) in &floats {
                    probe.set_float_bits(key, *bits)?;
                }
                self.staged = Some(Change::Preferences(integers, floats));
                let body = autolog::UserSettingsUpdateResponse {
                    blaze_id: Some(self.persona),
                    success: Some(true),
                    ..Default::default()
                }
                .encode(limits)
                .map_err(|_| Error::Encode)?;
                response(&f, &body).map(Some)
            }
            _ => Err(Error::Ineligible),
        }
    }
}

impl Change {
    /// Apply a complete delta to a clone first, so invalid multi-key updates
    /// cannot leave partially modified state.
    pub fn apply(&self, settings: &mut Settings) -> Result<bool, Error> {
        let mut next = settings.clone();
        next.validate()?;
        match self {
            Self::String(key, value) => {
                next.set_string(key, value)?;
            }
            Self::Preferences(integers, floats) => {
                unique(integers)?;
                unique(floats)?;
                if integers.is_empty() && floats.is_empty() {
                    return Err(Error::Ineligible);
                }
                for (key, value) in integers {
                    next.set_integer(key, *value)?;
                }
                for (key, value) in floats {
                    next.set_float_bits(key, *value)?;
                }
            }
        }
        let changed = next != *settings;
        *settings = next;
        Ok(changed)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn request(route: (u16, u16), correlation: u32, body: &[u8]) -> Vec<u8> {
        nfs_fire2::encode(
            Frame {
                fields: Fields {
                    routing_a: route.0,
                    routing_b: route.1,
                    correlation,
                    ..Default::default()
                },
                metadata: &[],
                body,
            },
            frame_limits(),
        )
        .unwrap()
    }
    fn decoded(reply: &[u8]) -> (Fields, Vec<u8>) {
        let d = nfs_fire2::decode(reply, frame_limits()).unwrap().unwrap();
        assert_eq!(d.consumed, reply.len());
        assert!(d.frame.metadata.is_empty());
        (d.frame.fields, d.frame.body.to_vec())
    }
    fn content() -> Settings {
        Settings {
            strings: vec![(b"example_setting".to_vec(), b"7".to_vec())],
            integers: vec![(b"music".to_vec(), 1)],
            floats: vec![(b"volume".to_vec(), 0x3f80_0000)],
        }
    }
    const PERSONA: i64 = 1_000_123;

    #[test]
    fn content_round_trips_through_json_with_bounds() {
        let s = content();
        let reloaded = Settings::from_json(&s.to_json()).unwrap();
        assert_eq!(reloaded, s);
        let mut odd = s.clone();
        odd.strings.push((b"bin".to_vec(), vec![0xff, 0x00, 0x01]));
        assert_eq!(Settings::from_json(&odd.to_json()).unwrap(), odd);
        let mut dup = s.clone();
        dup.integers.push((b"music".to_vec(), 2));
        assert_eq!(dup.validate(), Err(Error::Config));
        let mut full = s.clone();
        for n in 0..MAX_ENTRIES {
            full.integers.push((format!("k{n}").into_bytes(), 0));
        }
        assert_eq!(full.validate(), Err(Error::Bounds));
        let mut v = s.to_json();
        v["version"] = json!(2);
        assert!(Settings::from_json(&v).is_err());
    }

    #[test]
    fn reads_answer_from_state_and_unknown_keys_get_no_reply() {
        let mut session = Session::new(PERSONA, content()).unwrap();
        let all = session.reply(&request((9, 12), 5, &[])).unwrap().unwrap();
        let (fields, body) = decoded(&all);
        assert_eq!(
            (
                fields.routing_a,
                fields.routing_b,
                fields.category,
                fields.correlation
            ),
            (9, 12, 1, 5)
        );
        let model = util::UserSettingsLoadAllResponse::decode(&body, body_limits()).unwrap();
        assert_eq!(
            model.data_map.unwrap().0,
            vec![(b"example_setting".as_slice(), b"7".as_slice())]
        );
        let one = util::UserSettingsLoadRequest {
            key: Some(b"example_setting"),
            user_id: Some(0),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        let reply = session.reply(&request((9, 10), 6, &one)).unwrap().unwrap();
        let (_, body) = decoded(&reply);
        let model = util::UserSettingsResponse::decode(&body, body_limits()).unwrap();
        assert_eq!(
            (model.key, model.data),
            (Some(b"example_setting".as_slice()), Some(b"7".as_slice()))
        );
        let missing = util::UserSettingsLoadRequest {
            key: Some(b"Nope"),
            user_id: Some(0),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        let absent = session
            .reply(&request((9, 10), 7, &missing))
            .unwrap()
            .unwrap();
        let d = nfs_fire2::decode(
            &absent,
            nfs_fire2::Limits::new(64, crate::ERROR_METADATA_BYTES, 0).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(d.consumed, absent.len());
        assert_eq!(
            (
                d.frame.fields.routing_a,
                d.frame.fields.routing_b,
                d.frame.fields.category,
                d.frame.fields.correlation
            ),
            (9, 10, 3, 7)
        );
        assert!(d.frame.body.is_empty());
        let m =
            nfs_protocol::metadata::Fire2Metadata::decode(d.frame.metadata, body_limits()).unwrap();
        assert_eq!(
            (m.context, m.error_code),
            (
                Some(0),
                Some(nfs_protocol::metadata::UTIL_USS_RECORD_NOT_FOUND)
            )
        );
        assert_eq!(
            nfs_protocol::metadata::error_name(9, m.error_code.unwrap()),
            Some("UTIL_USS_RECORD_NOT_FOUND")
        );
        assert!(session.pending().is_none());
        let foreign = autolog::UserSettingsRequest {
            blaze_id: Some(PERSONA + 1),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        assert_eq!(
            session.reply(&request((2050, 72), 8, &foreign)).unwrap(),
            None
        );
        let own = autolog::UserSettingsRequest {
            blaze_id: Some(PERSONA),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        let reply = session
            .reply(&request((2050, 72), 9, &own))
            .unwrap()
            .unwrap();
        let (_, body) = decoded(&reply);
        let model = autolog::UserSettingsResponse::decode(&body, body_limits()).unwrap();
        assert_eq!(model.blaze_id, Some(PERSONA));
        assert_eq!(
            model.settings_int.unwrap().0,
            vec![(b"music".as_slice(), 1)]
        );
        assert_eq!(
            model.settings_flt.unwrap().0,
            vec![(b"volume".as_slice(), 0x3f80_0000)]
        );
        assert_eq!(
            session.reply(&request((7, 4), 1, &[])),
            Err(Error::Ineligible)
        );
        assert_eq!(
            session.reply(&request((9, 12), 1, &[1, 2])),
            Err(Error::Ineligible)
        );
    }

    #[test]
    fn writes_apply_only_on_commit_and_report_changes() {
        let mut session = Session::new(PERSONA, content()).unwrap();
        let save = util::UserSettingsSaveRequest {
            data: Some(b"8"),
            key: Some(b"example_setting"),
            user_id: Some(0),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        let reply = session
            .reply(&request((9, 11), 10, &save))
            .unwrap()
            .unwrap();
        let (fields, body) = decoded(&reply);
        assert_eq!((fields.category, fields.correlation), (1, 10));
        assert!(body.is_empty());
        // Not applied until committed; an abort discards it.
        assert_eq!(session.settings().strings[0].1, b"7");
        session.abort();
        assert!(!session.commit().unwrap());
        assert_eq!(session.settings().strings[0].1, b"7");
        session
            .reply(&request((9, 11), 11, &save))
            .unwrap()
            .unwrap();
        assert!(session.commit().unwrap());
        assert_eq!(session.settings().strings[0].1, b"8");
        // The same value again is acknowledged but changes nothing.
        session
            .reply(&request((9, 11), 12, &save))
            .unwrap()
            .unwrap();
        assert!(!session.commit().unwrap());
        let new_key = util::UserSettingsSaveRequest {
            data: Some(b"x"),
            key: Some(b"Fresh"),
            user_id: Some(0),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        session
            .reply(&request((9, 11), 13, &new_key))
            .unwrap()
            .unwrap();
        assert!(session.commit().unwrap());
        assert_eq!(session.settings().strings.len(), 2);
        let update = autolog::UserSettingsUpdateRequest {
            blaze_id: Some(PERSONA),
            settings_int: Some(autolog::SettingsIntegers(vec![(b"music", 0)])),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        let reply = session
            .reply(&request((2050, 73), 14, &update))
            .unwrap()
            .unwrap();
        let (_, body) = decoded(&reply);
        let model = autolog::UserSettingsUpdateResponse::decode(&body, body_limits()).unwrap();
        assert_eq!((model.blaze_id, model.success), (Some(PERSONA), Some(true)));
        assert!(session.commit().unwrap());
        assert_eq!(session.settings().integers, vec![(b"music".to_vec(), 0)]);
        // A foreign persona or an empty update is not acknowledged.
        let foreign = autolog::UserSettingsUpdateRequest {
            blaze_id: Some(PERSONA + 1),
            settings_int: Some(autolog::SettingsIntegers(vec![(b"music", 5)])),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        assert_eq!(
            session.reply(&request((2050, 73), 15, &foreign)).unwrap(),
            None
        );
        let empty = autolog::UserSettingsUpdateRequest {
            blaze_id: Some(PERSONA),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        assert_eq!(
            session.reply(&request((2050, 73), 16, &empty)).unwrap(),
            None
        );
        assert!(!session.commit().unwrap());
        // A full string map refuses a new key instead of acknowledging it.
        let mut full = content();
        for n in 0..MAX_ENTRIES - 1 {
            full.strings.push((format!("k{n}").into_bytes(), vec![]));
        }
        let mut session = Session::new(PERSONA, full).unwrap();
        assert_eq!(
            session.reply(&request((9, 11), 17, &new_key)),
            Err(Error::Bounds)
        );
    }
}
