// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The machine-readable contract a desktop front end consumes.
//!
//! Every object a command prints with `--json`, and every line of a session
//! event stream, is a [`Record`]: a Rust type with a stable name, a
//! `version` field and a JSON Schema generated from the type itself. The
//! schemas are committed under `crates/launcher/schema/<name>.schema.json`,
//! printed by `nfs-afterhours schema <name>` and shipped with each release;
//! a unit test fails when the committed files differ from the types.
//!
//! Versioning: until the desktop launcher's first release every record is
//! version 1 and may change in lockstep with it. From that release on, a
//! change that removes or renames a field, changes a field's type or
//! meaning, or removes an enum value bumps that record's version. Adding
//! a field or an enum value does not. `version --json` reports the
//! current version of every record and the optional features this build
//! provides.

use crate::{
    api, events, map, profile,
    records::{self, AccountRecord, Preflight, Status},
    sessions,
};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::Serialize;
use std::{borrow::Cow, collections::BTreeMap, path::PathBuf};

/// A JSON record with a stable name and version.
pub trait Record: JsonSchema + Serialize {
    /// Schema file stem and key in `version --json`.
    const NAME: &'static str;
    /// Current version; the value of the record's `version` field.
    const VERSION: u32;
}

impl Record for Preflight {
    const NAME: &'static str = "check";
    const VERSION: u32 = records::VERSION;
}
impl Record for Status {
    const NAME: &'static str = "session-status";
    const VERSION: u32 = records::VERSION;
}
impl Record for StatusReport {
    const NAME: &'static str = "status";
    const VERSION: u32 = 1;
}
impl Record for StopReport {
    const NAME: &'static str = "stop";
    const VERSION: u32 = 1;
}
impl Record for PlayReport {
    const NAME: &'static str = "play";
    const VERSION: u32 = 1;
}
impl Record for RecoverReport {
    const NAME: &'static str = "recover";
    const VERSION: u32 = 1;
}
impl Record for SettingsReport {
    const NAME: &'static str = "settings";
    const VERSION: u32 = 1;
}
impl Record for VersionReport {
    const NAME: &'static str = "version";
    const VERSION: u32 = 1;
}
impl Record for profile::Profile {
    const NAME: &'static str = "profile";
    const VERSION: u32 = profile::FORMAT_VERSION;
}
impl Record for profile::Listing {
    const NAME: &'static str = "profile-list";
    const VERSION: u32 = profile::FORMAT_VERSION;
}
impl Record for profile::images::CarImagesReport {
    const NAME: &'static str = "car-images";
    const VERSION: u32 = profile::images::CarImagesReport::VERSION;
}
impl Record for map::MapImagesReport {
    const NAME: &'static str = "map-images";
    const VERSION: u32 = map::MapImagesReport::VERSION;
}
impl Record for sessions::SessionList {
    const NAME: &'static str = "sessions";
    const VERSION: u32 = sessions::FORMAT_VERSION;
}
impl Record for api::ApiRecord {
    const NAME: &'static str = "api";
    const VERSION: u32 = api::FORMAT_VERSION;
}
impl Record for api::ApiError {
    const NAME: &'static str = "api-error";
    const VERSION: u32 = api::FORMAT_VERSION;
}
impl Record for events::Event {
    const NAME: &'static str = "event";
    const VERSION: u32 = events::FORMAT_VERSION;
}

/// One entry of [`records`].
#[derive(Clone, Copy)]
pub struct RecordInfo {
    pub name: &'static str,
    pub version: u32,
    schema_name: fn() -> Cow<'static, str>,
    generate: fn() -> schemars::Schema,
}

fn info<T: Record>() -> RecordInfo {
    RecordInfo {
        name: T::NAME,
        version: T::VERSION,
        schema_name: T::schema_name,
        generate: || {
            // Draft 7 for the widest TypeScript and validator support; the
            // schemas describe what this program writes, not what it reads.
            SchemaSettings::draft07()
                .for_serialize()
                .into_generator()
                .into_root_schema_for::<T>()
        },
    }
}

/// Every record, sorted by name.
pub fn records() -> [RecordInfo; 16] {
    let mut all = [
        info::<api::ApiRecord>(),
        info::<api::ApiError>(),
        info::<Preflight>(),
        info::<profile::images::CarImagesReport>(),
        info::<events::Event>(),
        info::<map::MapImagesReport>(),
        info::<profile::Profile>(),
        info::<profile::Listing>(),
        info::<RecoverReport>(),
        info::<Status>(),
        info::<sessions::SessionList>(),
        info::<SettingsReport>(),
        info::<StatusReport>(),
        info::<StopReport>(),
        info::<PlayReport>(),
        info::<VersionReport>(),
    ];
    all.sort_by_key(|r| r.name);
    all
}

/// Names of every record, sorted.
pub fn record_names() -> Vec<&'static str> {
    records().iter().map(|r| r.name).collect()
}

/// The JSON Schema of one record, with every record's `version` property
/// fixed to its current value wherever the record appears.
pub fn schema(name: &str) -> Option<serde_json::Value> {
    let all = records();
    let record = all.iter().find(|r| r.name == name)?;
    let mut value = (record.generate)().to_value();
    stamp_version(&mut value, record.version);
    let versions: BTreeMap<String, u32> = all
        .iter()
        .map(|r| ((r.schema_name)().into_owned(), r.version))
        .collect();
    if let Some(definitions) = value
        .get_mut("definitions")
        .and_then(serde_json::Value::as_object_mut)
    {
        for (name, definition) in definitions {
            if let Some(&version) = versions.get(name) {
                stamp_version(definition, version);
            }
        }
    }
    Some(value)
}

fn stamp_version(schema: &mut serde_json::Value, version: u32) {
    if let Some(property) = schema.pointer_mut("/properties/version")
        && let Some(property) = property.as_object_mut()
    {
        property.insert("const".into(), version.into());
    }
}

/// A record's schema as committed: pretty JSON with a final newline.
pub fn schema_text(name: &str) -> Option<String> {
    let value = schema(name)?;
    let mut text = serde_json::to_string_pretty(&value).ok()?;
    text.push('\n');
    Some(text)
}

/// `status --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct StatusReport {
    pub version: u32,
    /// The current session's latest status, or `null` when no session was
    /// ever started in this data directory. A live state whose supervisor
    /// is gone is reported as `interrupted`.
    pub session: Option<Status>,
}

impl StatusReport {
    pub fn new(session: Option<Status>) -> Self {
        Self {
            version: <Self as Record>::VERSION,
            session,
        }
    }
}

/// What `stop` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StopOutcome {
    /// A stop request was written; the supervisor closes the game, then the
    /// server. Watch `status` or the event stream for `finished`.
    Requested,
    /// The current session had already ended; nothing was written.
    AlreadyOver,
}

/// `stop --json`. With no session at all, `stop` exits with code 4 instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct StopReport {
    pub version: u32,
    pub outcome: StopOutcome,
    /// The current session's status when the request was made.
    pub status: Status,
    /// The stop request file, for `requested`.
    pub stop_file: Option<PathBuf>,
}

impl StopReport {
    pub fn new(outcome: StopOutcome, status: Status, stop_file: Option<PathBuf>) -> Self {
        Self {
            version: <Self as Record>::VERSION,
            outcome,
            status,
            stop_file,
        }
    }
}

/// `play --json`: the request was written. Failures, such as no running
/// `serve` session or a game already running, exit with code 4 or 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct PlayReport {
    pub version: u32,
    /// The session's status when the request was made. The game's progress
    /// follows in the session's later statuses and events.
    pub status: Status,
    /// The play request file the serving supervisor consumes.
    pub play_file: PathBuf,
}

impl PlayReport {
    pub fn new(status: Status, play_file: PathBuf) -> Self {
        Self {
            version: <Self as Record>::VERSION,
            status,
            play_file,
        }
    }
}

/// `settings --json`: the saved traffic and rival amounts, which every later
/// `setup`, `serve` and `start` uses and a running session applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SettingsReport {
    pub version: u32,
    /// Ambient traffic as a percentage of the original amount; 0 turns it
    /// off and 100 is the original.
    pub traffic_percent: u16,
    /// Roaming rivals kept active; 0 turns them off and `null` keeps the
    /// original number.
    pub rival_limit: Option<u16>,
    /// Largest `traffic_percent` this build accepts.
    pub max_traffic_percent: u16,
    /// Largest `rival_limit` this build accepts.
    pub max_rivals: u16,
    /// Whether a `rival_limit` above the game's own number adds roaming
    /// rivals (other installed racers near the player, each at most once).
    /// When `false` it only caps the game's own.
    pub rivals_above_original: bool,
    /// Whether saving applies to a running session within about a second
    /// (the server watches `settings.json`); when `false`, a running
    /// session keeps the values it started with and saving is refused.
    pub live: bool,
    /// `settings.json` in the data directory; it may not exist yet.
    pub file: PathBuf,
    /// Whether this command saved the file.
    pub saved: bool,
}

impl SettingsReport {
    pub fn new(settings: &crate::settings::Settings, file: PathBuf, saved: bool) -> Self {
        Self {
            version: <Self as Record>::VERSION,
            traffic_percent: settings.traffic_percent,
            rival_limit: settings.rival_limit,
            max_traffic_percent: crate::settings::MAX_TRAFFIC_PERCENT,
            max_rivals: crate::settings::MAX_RIVALS,
            rivals_above_original: crate::settings::RIVALS_ABOVE_ORIGINAL,
            live: true,
            file,
            saved,
        }
    }
}

/// `recover --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct RecoverReport {
    pub version: u32,
    /// The account record now in `account.json`.
    pub account: AccountRecord,
    /// Whether `account.json` was written now; `false` when it already
    /// named this account.
    pub written: bool,
}

impl RecoverReport {
    pub fn new(account: AccountRecord, written: bool) -> Self {
        Self {
            version: <Self as Record>::VERSION,
            account,
            written,
        }
    }
}

/// Optional features a build provides. Screens whose capability is absent
/// should say the feature is not available yet rather than fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// `profile` and `profile --list`.
    Profile,
    /// `sessions`.
    Sessions,
    /// `--events` on `setup`, `serve` and `start`.
    Events,
    /// `play`: start the game inside a running `serve` session (Windows).
    Play,
    /// `car-images`, and per-car `image` in `profile`.
    CarImages,
    /// `map-images`: the game's world map and the world box it covers.
    MapImages,
    /// `settings`, and `--traffic` and `--rivals` on `setup`, `serve` and
    /// `start`: the amounts of traffic and roaming rivals.
    WorldSettings,
    /// `map-feed` events with `--events` on `serve` and `start`: live
    /// positions of the players' cars, traffic and roaming rivals.
    MapFeed,
    /// `--api` on `setup`, `serve` and `start`, and `api`: the session over
    /// HTTP on this computer, with its event stream, `stop` and `play`, the
    /// saved `settings` to read and change, and the selected `profile`.
    Api,
}

/// Capabilities of this build, in a fixed order.
pub const CAPABILITIES: [Capability; 9] = [
    Capability::Profile,
    Capability::Sessions,
    Capability::Events,
    Capability::Play,
    Capability::CarImages,
    Capability::MapImages,
    Capability::WorldSettings,
    Capability::MapFeed,
    Capability::Api,
];

/// Identity of the program the launcher library is running in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Build {
    /// `nfs-afterhours`, or `Afterhours` for the single-executable bundle.
    pub program: &'static str,
    /// That program's package version.
    pub version: &'static str,
    /// Release tag, or `development`.
    pub release: &'static str,
    /// Source commit, when the build recorded one.
    pub revision: Option<&'static str>,
}

impl Build {
    /// The standalone `nfs-afterhours` binary of this crate.
    pub const LAUNCHER: Build = Build {
        program: "nfs-afterhours",
        version: env!("CARGO_PKG_VERSION"),
        release: match option_env!("AFTERHOURS_RELEASE") {
            Some(release) => release,
            None => "development",
        },
        revision: option_env!("AFTERHOURS_REVISION"),
    };
}

/// `version --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct VersionReport {
    pub version: u32,
    /// `Afterhours` for the single-executable bundle, `nfs-afterhours` for
    /// the standalone launcher.
    pub program: String,
    /// The program's package version, such as `0.3.0`. For `Afterhours`
    /// this is the bundle version that release tags name.
    pub program_version: String,
    /// The release tag the program was built for, or `development`.
    pub release: String,
    /// The source commit the program was built from; `null` for builds
    /// that did not record one.
    pub revision: Option<String>,
    /// Version of the launcher library (`nfs-afterhours`) inside it.
    pub launcher_version: String,
    /// Operating system: `windows`, `linux` or `macos`.
    pub os: String,
    /// Whether `start` can launch and route the game on this platform.
    pub game_launch: bool,
    /// Current version of every record, by schema name.
    pub records: BTreeMap<String, u32>,
    /// Optional features this build provides.
    pub capabilities: Vec<Capability>,
}

impl VersionReport {
    pub fn new(build: &Build) -> Self {
        Self {
            version: <Self as Record>::VERSION,
            program: build.program.to_owned(),
            program_version: build.version.to_owned(),
            release: build.release.to_owned(),
            revision: build
                .revision
                .filter(|revision| !revision.is_empty())
                .map(str::to_owned),
            launcher_version: env!("CARGO_PKG_VERSION").to_owned(),
            os: std::env::consts::OS.to_owned(),
            game_launch: crate::platform::supports_game_launch(),
            records: records()
                .iter()
                .map(|r| (r.name.to_owned(), r.version))
                .collect(),
            capabilities: CAPABILITIES.to_vec(),
        }
    }
}

/// Test support: a strict validator for the subset of JSON Schema that
/// schemars generates. Every property a record prints must be described
/// by its schema, so a hand-written serializer cannot drift from it.
#[cfg(test)]
pub(crate) mod testing {
    use serde_json::Value;
    use std::collections::BTreeSet;

    /// Validate `value` against the committed contract for record `name`.
    pub(crate) fn validate(name: &str, value: &impl serde::Serialize) {
        let schema = super::schema(name).unwrap_or_else(|| panic!("no record {name}"));
        let value = serde_json::to_value(value).unwrap();
        if let Err(error) = at(&schema, &schema, &value, "$") {
            panic!("{name} does not match its schema: {error}\n{value:#}");
        }
    }

    pub(crate) fn at(
        root: &Value,
        schema: &Value,
        value: &Value,
        path: &str,
    ) -> Result<(), String> {
        let evaluated = check(root, schema, value, path)?;
        if let Some(object) = value.as_object()
            && let Some(key) = object.keys().find(|k| !evaluated.contains(*k))
        {
            return Err(format!("{path}.{key} is not described by the schema"));
        }
        Ok(())
    }

    fn type_matches(name: &str, value: &Value) -> bool {
        match name {
            "null" => value.is_null(),
            "boolean" => value.is_boolean(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "string" => value.is_string(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => false,
        }
    }

    /// Check one instance location; returns the object keys this schema
    /// evaluated there, like `unevaluatedProperties`.
    fn check(
        root: &Value,
        schema: &Value,
        value: &Value,
        path: &str,
    ) -> Result<BTreeSet<String>, String> {
        let mut evaluated = BTreeSet::new();
        if let Some(allowed) = schema.as_bool() {
            return if allowed {
                Ok(value
                    .as_object()
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default())
            } else {
                Err(format!("{path} is not allowed"))
            };
        }
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            let pointer = reference
                .strip_prefix('#')
                .ok_or_else(|| format!("{path}: external reference {reference}"))?;
            let target = root
                .pointer(pointer)
                .ok_or_else(|| format!("{path}: missing {reference}"))?;
            evaluated.extend(check(root, target, value, path)?);
        }
        if let Some(constant) = schema.get("const")
            && constant != value
        {
            return Err(format!("{path} is {value}, expected {constant}"));
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array)
            && !values.contains(value)
        {
            return Err(format!("{path} is {value}, not one of {values:?}"));
        }
        match schema.get("type") {
            Some(Value::String(name)) if !type_matches(name, value) => {
                return Err(format!("{path} is {value}, expected {name}"));
            }
            Some(Value::Array(names))
                if !names
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|n| type_matches(n, value)) =>
            {
                return Err(format!("{path} is {value}, expected one of {names:?}"));
            }
            _ => {}
        }
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
            && value.as_f64().is_some_and(|v| v < minimum)
        {
            return Err(format!("{path} is below {minimum}"));
        }
        if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
            for branch in all {
                evaluated.extend(check(root, branch, value, path)?);
            }
        }
        for (keyword, exactly_one) in [("anyOf", false), ("oneOf", true)] {
            if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
                let mut errors = Vec::new();
                let mut passed = 0;
                for branch in branches {
                    match check(root, branch, value, path) {
                        Ok(keys) => {
                            passed += 1;
                            evaluated.extend(keys);
                        }
                        Err(error) => errors.push(error),
                    }
                }
                if passed == 0 || (exactly_one && passed > 1) {
                    return Err(format!(
                        "{path}: {passed} {keyword} branches matched ({})",
                        errors.join("; ")
                    ));
                }
            }
        }
        if let Some(object) = value.as_object() {
            let properties = schema.get("properties").and_then(Value::as_object);
            if let Some(properties) = properties {
                for (key, property) in properties {
                    if let Some(item) = object.get(key) {
                        at(root, property, item, &format!("{path}.{key}"))?;
                        evaluated.insert(key.clone());
                    }
                }
            }
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(key) {
                        return Err(format!("{path}.{key} is required"));
                    }
                }
            }
            if let Some(additional) = schema.get("additionalProperties") {
                for (key, item) in object {
                    if properties.is_some_and(|p| p.contains_key(key)) {
                        continue;
                    }
                    at(root, additional, item, &format!("{path}.{key}"))?;
                    evaluated.insert(key.clone());
                }
            }
        }
        if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
            for (index, item) in array.iter().enumerate() {
                at(root, items, item, &format!("{path}[{index}]"))?;
            }
        }
        Ok(evaluated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::State;

    const SCHEMA_DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/schema");
    /// Set to rewrite the committed schemas from the types.
    const UPDATE: &str = "AFTERHOURS_UPDATE_SCHEMAS";

    fn status() -> Status {
        Status {
            version: records::VERSION,
            id: "a".repeat(32),
            utc: "2026-10-19T12:00:00Z".into(),
            state: State::Ready,
            message: "Server is ready.".into(),
            worker_pid: 10,
            action: Some(crate::records::Action::Serve),
            game_running: false,
            game_pid: None,
            server_pid: Some(11),
        }
    }

    #[test]
    fn committed_schemas_match_the_types() {
        let directory = std::path::Path::new(SCHEMA_DIRECTORY);
        let update = std::env::var_os(UPDATE).is_some();
        let mut stale = Vec::new();
        for name in record_names() {
            let path = directory.join(format!("{name}.schema.json"));
            let expected = schema_text(name).unwrap();
            // Checkouts may convert line endings; the content is what counts.
            let committed = std::fs::read_to_string(&path)
                .map(|text| text.replace("\r\n", "\n"))
                .unwrap_or_default();
            if committed != expected {
                if update {
                    std::fs::create_dir_all(directory).unwrap();
                    std::fs::write(&path, expected).unwrap();
                } else {
                    stale.push(name);
                }
            }
        }
        let mut extra = Vec::new();
        for entry in std::fs::read_dir(directory).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".schema.json");
            if !stem.is_some_and(|stem| record_names().contains(&stem)) {
                extra.push(name);
            }
        }
        assert!(
            stale.is_empty() && extra.is_empty(),
            "crates/launcher/schema is out of date (stale: {stale:?}, unknown files: {extra:?}).\n\
             Regenerate with `{UPDATE}=1 cargo test -p nfs-afterhours --lib contract`, \
             review the diff and decide whether a record version must change."
        );
    }

    #[test]
    fn every_record_version_is_stamped_into_its_schema() {
        let names = record_names();
        let mut sorted = names.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "record names are unique");
        for record in records() {
            let schema = schema(record.name).unwrap();
            assert_eq!(
                schema.pointer("/properties/version/const"),
                Some(&serde_json::Value::from(record.version)),
                "{}",
                record.name
            );
            assert_eq!(
                schema["required"]
                    .as_array()
                    .map(|r| r.contains(&"version".into())),
                Some(true),
                "{} must always print its version",
                record.name
            );
        }
        // Nested records carry their own version wherever they appear.
        let status = schema("status").unwrap();
        assert_eq!(
            status.pointer("/definitions/Status/properties/version/const"),
            Some(&serde_json::Value::from(records::VERSION))
        );
        assert!(schema("no-such-record").is_none());
    }

    #[test]
    fn a_definition_name_means_the_same_type_in_every_schema() {
        // Front ends may generate all records into one namespace.
        let mut seen: BTreeMap<String, (String, serde_json::Value)> = BTreeMap::new();
        for name in record_names() {
            let schema = schema(name).unwrap();
            let mut definitions: Vec<(String, serde_json::Value)> = schema["definitions"]
                .as_object()
                .map(|d| d.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            let title = schema["title"].as_str().unwrap().to_owned();
            let mut root = schema.clone();
            let root = root.as_object_mut().unwrap();
            for key in ["$schema", "title", "definitions"] {
                root.remove(key);
            }
            definitions.push((title, serde_json::Value::Object(root.clone())));
            for (definition, body) in definitions {
                if let Some((first, previous)) = seen.get(&definition) {
                    let mut a = previous.clone();
                    let mut b = body.clone();
                    for v in [&mut a, &mut b] {
                        if let Some(o) = v.as_object_mut() {
                            o.remove("description");
                        }
                    }
                    assert_eq!(a, b, "{definition} differs between {first} and {name}");
                } else {
                    seen.insert(definition, (name.to_owned(), body));
                }
            }
        }
    }

    #[test]
    fn reports_match_their_schemas_and_the_validator_is_strict() {
        testing::validate("status", &StatusReport::new(None));
        testing::validate("status", &StatusReport::new(Some(status())));
        testing::validate("session-status", &status());
        testing::validate("play", &PlayReport::new(status(), PathBuf::from("play")));
        testing::validate(
            "stop",
            &StopReport::new(
                StopOutcome::Requested,
                status(),
                Some(PathBuf::from("stop")),
            ),
        );
        testing::validate(
            "stop",
            &StopReport::new(StopOutcome::AlreadyOver, status(), None),
        );
        testing::validate(
            "recover",
            &RecoverReport::new(AccountRecord::post_intro("b".repeat(32)), true),
        );
        let report = VersionReport::new(&Build::LAUNCHER);
        testing::validate("version", &report);
        assert_eq!(report.records.len(), records().len());
        assert_eq!(report.records["event"], events::FORMAT_VERSION);
        assert_eq!(report.capabilities, CAPABILITIES);

        let schema = schema("status").unwrap();
        let mut extra = serde_json::to_value(StatusReport::new(Some(status()))).unwrap();
        extra["session"]["undocumented"] = true.into();
        assert!(testing::at(&schema, &schema, &extra, "$").is_err());
        let mut wrong = serde_json::to_value(StatusReport::new(None)).unwrap();
        wrong["version"] = 2.into();
        assert!(testing::at(&schema, &schema, &wrong, "$").is_err());
        let mut state = serde_json::to_value(StatusReport::new(Some(status()))).unwrap();
        state["session"]["state"] = "paused".into();
        assert!(testing::at(&schema, &schema, &state, "$").is_err());
        let mut missing = serde_json::to_value(StatusReport::new(None)).unwrap();
        missing.as_object_mut().unwrap().remove("session");
        assert!(testing::at(&schema, &schema, &missing, "$").is_err());
    }

    #[test]
    fn a_build_without_a_revision_reports_null() {
        let mut build = Build::LAUNCHER;
        build.revision = Some("");
        assert_eq!(VersionReport::new(&build).revision, None);
        build.revision = Some("0123abc");
        build.program = "Afterhours";
        let report = VersionReport::new(&build);
        assert_eq!(report.revision.as_deref(), Some("0123abc"));
        assert_eq!(report.program, "Afterhours");
        assert_eq!(report.launcher_version, env!("CARGO_PKG_VERSION"));
    }
}
