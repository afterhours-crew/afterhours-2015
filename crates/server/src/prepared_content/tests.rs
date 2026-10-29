// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::event_logic::{
    Content,
    tests::{awards, event},
};
use nfs_content::events::Event;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "afterhours-static-cache-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn cache(&self) -> Cache {
        Cache {
            root: self.0.clone(),
            installation: "a".repeat(64),
            executable: "b".repeat(64),
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn decode(bytes: &[u8]) -> Result<Content, Error> {
    let event = serde_json::from_slice(bytes).map_err(|_| Error::Cache("event format"))?;
    Content::new(event, "SpeedWall_SprintRace".into())
        .and_then(|c| c.with_awards(awards()))
        .map_err(|_| Error::Cache("event shape"))
}

#[test]
fn cached_sequence_reconstructs_wire_profiles_and_rejects_invalid_channels() {
    use crate::sequence_content::authored::{Authored, Instances, tests::fixture};
    let dir = Directory::new();
    let (players, authored, first, second) = fixture();
    let bytes = serde_json::to_vec(&authored).unwrap();
    let decode = |bytes: &[u8]| {
        serde_json::from_slice::<Authored>(bytes).map_err(|_| Error::Cache("prepared sequence"))
    };
    let cold = dir
        .cache()
        .prepare("sequence", b"parent", decode, || Ok(bytes.clone()))
        .unwrap();
    let warm = dir
        .cache()
        .prepare("sequence", b"parent", decode, || {
            panic!("warm sequence rebuilt")
        })
        .unwrap();
    for owner in [first, second] {
        let original = Instances::default()
            .start(&authored, &mut players.clone(), owner)
            .unwrap();
        let cached = Instances::default()
            .start(&warm, &mut players.clone(), owner)
            .unwrap();
        assert_eq!(format!("{original:?}"), format!("{cached:?}"));
    }
    assert_eq!(format!("{cold:?}"), format!("{warm:?}"));
    let mut invalid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    invalid["channels"] = serde_json::json!({});
    assert!(
        dir.cache()
            .prepare("invalid-sequence", b"parent", decode, || Ok(
                serde_json::to_vec(&invalid).unwrap()
            ))
            .is_err()
    );
    assert!(serde_json::from_slice::<Authored>(&serde_json::to_vec(&invalid).unwrap()).is_err());
}

#[test]
fn compiled_event_is_reused_across_cache_instances_and_source_changes_invalidate() {
    let dir = Directory::new();
    let source = event();
    let bytes = serde_json::to_vec(&source).unwrap();
    let cold = dir
        .cache()
        .prepare("event", b"categories", decode, || Ok(bytes.clone()))
        .unwrap();
    let warm = dir
        .cache()
        .prepare("event", b"categories", decode, || panic!("warm recompiled"))
        .unwrap();
    assert_eq!(cold.event(), warm.event());
    assert_eq!(
        format!("{:?}", cold.creation_content()),
        format!("{:?}", warm.creation_content())
    );
    let builds = AtomicU64::new(0);
    let mut changed = dir.cache();
    changed.installation = "c".repeat(64);
    changed
        .prepare("event", b"categories", decode, || {
            builds.fetch_add(1, Ordering::Relaxed);
            Ok(bytes.clone())
        })
        .unwrap();
    let mut changed = dir.cache();
    changed.executable = "d".repeat(64);
    changed
        .prepare("event", b"categories", decode, || {
            builds.fetch_add(1, Ordering::Relaxed);
            Ok(bytes.clone())
        })
        .unwrap();
    dir.cache()
        .prepare("event", b"changed-categories", decode, || {
            builds.fetch_add(1, Ordering::Relaxed);
            Ok(bytes.clone())
        })
        .unwrap();
    assert_eq!(builds.load(Ordering::Relaxed), 3);
}

#[test]
fn structurally_decodable_but_invalid_model_is_never_published() {
    let dir = Directory::new();
    let mut invalid = event();
    invalid.game_mode.laps = 0;
    let bytes = serde_json::to_vec(&invalid).unwrap();
    assert!(serde_json::from_slice::<Event>(&bytes).is_ok());
    assert!(
        dir.cache()
            .prepare("event", b"", decode, || Ok(bytes))
            .is_err()
    );
    assert!(!std::fs::read_dir(&dir.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|v| v == "bin")
    }));
    let bytes = serde_json::to_vec(&event()).unwrap();
    dir.cache()
        .prepare("event", b"", decode, || Ok(bytes))
        .unwrap();
    dir.cache()
        .prepare("event", b"", decode, || {
            panic!("correct replacement not cached")
        })
        .unwrap();
}

#[test]
fn structured_identity_maps_reject_duplicate_keys_and_float_bits_survive() {
    use nfs_world::scoring::{participant_properties::Property, value::Value};
    let mut event = event();
    let property = Property {
        channel: "constructed/channel".into(),
        port: 7,
    };
    event
        .award_values
        .insert(property.clone(), Value::Float(-0.));
    let bytes = serde_json::to_vec(&event).unwrap();
    let decoded: Event = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(format!("{decoded:?}"), format!("{event:?}"));
    let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let row = json["award_values"][0].clone();
    json["award_values"].as_array_mut().unwrap().push(row);
    assert!(serde_json::from_value::<Event>(json).is_err());
    for bits in [
        1,
        0x8000_0000,
        0x3eaa_aaab,
        0x7f7f_ffff,
        0x0080_0000,
        0x3f80_0001,
    ] {
        let value = f32::from_bits(bits);
        let decoded: f32 = serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(decoded.to_bits(), bits);
    }
}

#[test]
fn private_constructor_invariants_cannot_be_bypassed_by_a_snapshot() {
    use nfs_world::scoring::{
        curve::{Curve, Interpolation, Knot},
        math,
        value::Text,
    };
    let curve = Curve::from_authored(
        [0., 1.],
        [0., 10.],
        1.,
        1.,
        Interpolation::Linear,
        vec![
            Knot {
                x: 0.7,
                y: 0.8,
                coefficient: 0.,
            },
            Knot {
                x: 0.3,
                y: 0.2,
                coefficient: 0.,
            },
        ],
    )
    .unwrap();
    let value = serde_json::to_value(&curve).unwrap();
    let restored: Curve = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(curve, restored); // Authored non-monotonic order is valid.
    let mut invalid = value;
    invalid["knots"] = json!([]);
    assert!(serde_json::from_value::<Curve>(invalid).is_err());
    assert!(serde_json::from_value::<math::Program>(json!({"instructions":[]})).is_err());
    assert!(serde_json::from_value::<Text>(json!("a\0b")).is_err());
    assert!(serde_json::from_value::<Text>(json!("x".repeat(1024))).is_err());
    let config = nfs_world::race_pacing::Config::new(
        vec![vec![nfs_world::race_pacing::Schedule {
            target: Some(curve),
            skill: None,
        }]],
        0.,
        1.,
        nfs_world::race_pacing::AiOnlyMode::First,
    )
    .unwrap();
    let mut value = serde_json::to_value(config).unwrap();
    value["groups"] = json!([]);
    assert!(serde_json::from_value::<nfs_world::race_pacing::Config>(value).is_err());
}
