// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Process-independent static compiler products. Live/player state is never an
//! input. The running executable's digest invalidates all products after any
//! compiler, dependency, build-option or schema change, including local builds.
use nfs_content::{
    Error,
    assets::AssetIndex,
    prepared_cache::{self, Key},
};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) mod parallel;
pub mod roads;
pub mod scoring;

pub struct Cache {
    root: PathBuf,
    installation: String,
    executable: String,
}

impl Cache {
    pub fn for_index(root: &Path, index: &AssetIndex) -> Result<Option<Self>, Error> {
        let Some(installation) = index.fingerprint() else {
            return Ok(None);
        };
        let mut file = std::fs::File::open(std::env::current_exe()?)?;
        const MAX_EXECUTABLE: u64 = 256 * 1024 * 1024;
        if file.metadata()?.len() > MAX_EXECUTABLE {
            return Err(Error::Cache("compiler identity bound"));
        }
        let mut hash = openssl::sha::Sha256::new();
        let mut buffer = [0; 64 * 1024];
        let mut total = 0;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > MAX_EXECUTABLE {
                return Err(Error::Cache("compiler identity bound"));
            }
            hash.update(&buffer[..count]);
        }
        Ok(Some(Self {
            root: root.join("artifacts/prepared-runtime"),
            installation: installation.to_owned(),
            executable: hash.finish().iter().map(|b| format!("{b:02x}")).collect(),
        }))
    }

    pub fn prepare<T>(
        &self,
        component: &str,
        inputs: &[u8],
        decode: impl Fn(&[u8]) -> Result<T, Error>,
        build: impl FnOnce() -> Result<Vec<u8>, Error>,
    ) -> Result<T, Error> {
        let mut identity = self.executable.as_bytes().to_vec();
        identity.extend_from_slice(inputs);
        let key = Key::new(&self.installation, component, 1, &identity)?;
        let started = Instant::now();
        let prepared = prepared_cache::prepare(
            &self.root,
            &key,
            prepared_cache::MAX_PAYLOAD,
            Duration::from_secs(600),
            || false,
            decode,
            build,
        )?;
        tracing::info!(component, source = ?prepared.source, elapsed_ms = started.elapsed().as_millis(),
            "static runtime content ready");
        Ok(prepared.value)
    }
}

#[cfg(test)]
mod tests;
