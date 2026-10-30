// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Check reusable local policy before constructing installation graphs.
use super::*;
use nfs_content::{
    Error,
    prepared_cache::{self, Key, Source},
};
use std::collections::BTreeMap;

type Documents = BTreeMap<String, Value>;
const MAX_BYTES: usize = 8 * 1024 * 1024;
// Bump when a dependency changes generated semantics without changing the
// installation builder revision. Packaged builds also include their revision.
const COMPILER_REVISION: u32 = 1;

fn key(fingerprint: &str, fixed: &Documents) -> Result<Key, Error> {
    let inputs = serde_json::to_vec(&json!({
        "policy": fixed,
        "build": option_env!("AFTERHOURS_REVISION"),
        "sources": [include_str!("../local_setup.rs"), include_str!("garage.rs"), include_str!("policy.rs"), include_str!("prepared.rs")],
    })).map_err(|_| Error::Cache("local policy cache inputs"))?;
    Key::new(
        fingerprint,
        "local-configuration",
        COMPILER_REVISION,
        &inputs,
    )
}

fn decode(
    bytes: &[u8],
    fixed: &Documents,
    scenes: &nfs_content::scenes::Catalog,
) -> Result<Documents, Error> {
    let invalid = || Error::Cache("invalid prepared local configuration");
    let documents: Documents = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let dynamic = ["garage-logic", "challenge-content", "world-content"];
    if documents.len() != fixed.len() + dynamic.len()
        || fixed
            .iter()
            .any(|(name, value)| documents.get(name) != Some(value))
        || dynamic.iter().any(|name| !documents.contains_key(*name))
    {
        return Err(invalid());
    }
    crate::garage_logic::GarageLogic::from_json(&documents["garage-logic"])
        .map_err(|_| invalid())?;
    nfs_services::challenges::Catalog::from_json(&documents["challenge-content"])
        .map_err(|_| invalid())?;
    crate::content::WorldContent::from_json_with_scenes(&documents["world-content"], Some(scenes))
        .and_then(|world| world.validate_runtime())
        .map_err(|_| invalid())?;
    Ok(documents)
}

pub(super) fn documents(
    root: &Path,
    generated: &crate::install_content::Generated,
    build: impl FnOnce() -> Result<Documents, Box<dyn std::error::Error>>,
) -> Result<Documents, Box<dyn std::error::Error>> {
    let fixed: Documents = policy::documents()
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    let key = key(&generated.prepared.fingerprint.digest, &fixed)?;
    let scenes = nfs_content::scenes::Catalog::load(&generated.scene_catalog)?;
    let prepared = prepared_cache::prepare(
        &root.join("artifacts/prepared-content"),
        &key,
        MAX_BYTES,
        std::time::Duration::from_secs(600),
        || false,
        |bytes| decode(bytes, &fixed, &scenes),
        || {
            let documents =
                build().map_err(|error| Error::Content(format!("local configuration: {error}")))?;
            serde_json::to_vec(&documents)
                .map_err(|_| Error::Cache("local configuration serialization"))
        },
    )?;
    match prepared.source {
        Source::Hit => {
            eprintln!("[setup] Loaded verified cached garage, challenge and world settings.")
        }
        Source::Missing => {
            eprintln!("[setup] Prepared and cached garage, challenge and world settings.")
        }
        Source::Invalid => {
            eprintln!("[setup] Rebuilt invalid cached garage, challenge and world settings.")
        }
    }
    Ok(prepared.value)
}

#[cfg(test)]
mod tests;
