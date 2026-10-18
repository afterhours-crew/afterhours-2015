// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Cache entries: `<root>/<fingerprint>/` holding one file per [`Kind`] and a
//! `manifest.json` written last. An entry is used only when its manifest names
//! the same fingerprint and builder revision and every output digest matches.
use crate::{BUILDER_REVISION, Error, FORMAT, Fingerprint, Kind, Prepared, VERSION, sha256_hex};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MANIFEST: &str = "manifest.json";
const MAX_MANIFEST_BYTES: u64 = 1 << 20;
const MAX_OUTPUT_BYTES: u64 = 64 << 20;

fn manifest(fingerprint: &Fingerprint, outputs: &BTreeMap<Kind, Vec<u8>>) -> Value {
    let inputs: Vec<_> = fingerprint
        .inputs
        .iter()
        .map(
            |i| json!({"path": i.path, "kind": i.kind.key(), "sha256": i.sha256, "bytes": i.bytes}),
        )
        .collect();
    let files: serde_json::Map<String, Value> = outputs
        .iter()
        .map(|(kind, bytes)| {
            (
                kind.key().to_owned(),
                json!({"file": kind.file_name(), "sha256": sha256_hex(bytes), "bytes": bytes.len()}),
            )
        })
        .collect();
    json!({
        "format": FORMAT,
        "version": VERSION,
        "builder_revision": BUILDER_REVISION,
        "fingerprint": fingerprint.digest,
        "inputs": inputs,
        "outputs": files,
    })
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Cache("cache file exceeds its bound"));
    }
    Ok(bytes)
}

/// Check an existing entry against the fingerprint.
pub fn verify(dir: &Path, fingerprint: &Fingerprint) -> Result<(), Error> {
    let manifest: Value =
        serde_json::from_slice(&read_bounded(&dir.join(MANIFEST), MAX_MANIFEST_BYTES)?)
            .map_err(|_| Error::Cache("manifest is not JSON"))?;
    if manifest["format"] != FORMAT
        || manifest["version"] != VERSION
        || manifest["builder_revision"] != BUILDER_REVISION
        || manifest["fingerprint"] != fingerprint.digest.as_str()
    {
        return Err(Error::Cache(
            "manifest is for another build or installation",
        ));
    }
    let outputs = manifest["outputs"]
        .as_object()
        .ok_or(Error::Cache("manifest outputs"))?;
    if outputs.len() != Kind::ALL.len() {
        return Err(Error::Cache("manifest output set"));
    }
    for kind in Kind::ALL {
        let entry = outputs
            .get(kind.key())
            .ok_or(Error::Cache("manifest output set"))?;
        if entry["file"] != kind.file_name() {
            return Err(Error::Cache("manifest output name"));
        }
        let bytes = read_bounded(&dir.join(kind.file_name()), MAX_OUTPUT_BYTES)?;
        if entry["sha256"] != sha256_hex(&bytes).as_str() || entry["bytes"] != bytes.len() {
            return Err(Error::Cache("output differs from manifest"));
        }
    }
    Ok(())
}

fn known(name: &std::ffi::OsStr) -> bool {
    name == MANIFEST || Kind::ALL.iter().any(|k| name == k.file_name())
}

/// Remove an invalid entry, but only when it holds nothing but entry files.
fn remove_entry(dir: &Path) -> Result<(), Error> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() || !known(&entry.file_name()) {
            return Err(Error::Cache(
                "invalid cache entry holds unexpected files; remove it manually",
            ));
        }
        files.push(entry.path());
    }
    for file in files {
        std::fs::remove_file(file)?;
    }
    std::fs::remove_dir(dir)?;
    Ok(())
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn temporary(root: &Path, digest: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    root.join(format!(
        ".build-{}-{}-{nanos}",
        &digest[..16.min(digest.len())],
        std::process::id()
    ))
}

/// Return the verified entry for `fingerprint`, building and publishing it
/// with `build` when absent or invalid.
pub fn prepare(
    root: &Path,
    fingerprint: Fingerprint,
    build: impl FnOnce(
        &Fingerprint,
        &mut dyn FnMut(crate::Progress),
    ) -> Result<BTreeMap<Kind, Vec<u8>>, Error>,
    report: &mut dyn FnMut(crate::Progress),
) -> Result<Prepared, Error> {
    if fingerprint.digest.len() != 64 || !fingerprint.digest.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::Cache("fingerprint"));
    }
    std::fs::create_dir_all(root)?;
    let dir = root.join(&fingerprint.digest);
    let found = |built| Prepared {
        directory: dir.clone(),
        fingerprint: fingerprint.clone(),
        built,
    };
    report(crate::Progress::CacheCheck);
    if dir.exists() {
        if verify(&dir, &fingerprint).is_ok() {
            report(crate::Progress::CacheReused);
            return Ok(found(false));
        }
        remove_entry(&dir)?;
    }
    report(crate::Progress::CacheRebuilding);
    let outputs = build(&fingerprint, report)?;
    if Kind::ALL.iter().any(|k| !outputs.contains_key(k)) || outputs.len() != Kind::ALL.len() {
        return Err(Error::Cache("builder output set"));
    }
    let temp = temporary(root, &fingerprint.digest);
    report(crate::Progress::Publishing);
    std::fs::create_dir(&temp)?;
    let written = (|| {
        for (kind, bytes) in &outputs {
            write_synced(&temp.join(kind.file_name()), bytes)?;
        }
        let text = serde_json::to_vec_pretty(&manifest(&fingerprint, &outputs))
            .map_err(|_| Error::Cache("manifest encoding"))?;
        write_synced(&temp.join(MANIFEST), &text)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_dir_all(&temp);
        return Err(error);
    }
    match std::fs::rename(&temp, &dir) {
        Ok(()) => Ok(found(true)),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp);
            // Another process may have published the same entry first.
            if dir.exists() && verify(&dir, &fingerprint).is_ok() {
                Ok(found(false))
            } else {
                Err(Error::Io(error))
            }
        }
    }
}
