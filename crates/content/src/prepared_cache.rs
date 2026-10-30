// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded, atomic caches for compiled static content. Call from a blocking
//! preparation worker. A key includes installation, component/compiler revision
//! and every additional input; account and session state never belongs here.
use crate::{Error, sha256_hex};
use serde_json::json;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const FORMAT: &str = "afterhours-prepared-static-v1";
pub(crate) mod ordered_map;
const MAX_HEADER: usize = 4096;
pub const MAX_PAYLOAD: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Key(String);

impl Key {
    pub fn new(
        installation: &str,
        component: &str,
        revision: u32,
        inputs: &[u8],
    ) -> Result<Self, Error> {
        if installation.len() != 64
            || !installation.bytes().all(|b| b.is_ascii_hexdigit())
            || component.is_empty()
            || component.len() > 128
            || inputs.len() > MAX_PAYLOAD
        {
            return Err(Error::Cache("invalid prepared-content identity"));
        }
        // Length framing prevents a component/input boundary changing the key.
        let prefix = serde_json::to_vec(&(FORMAT, installation, component, revision))
            .map_err(|_| Error::Cache("prepared-content identity"))?;
        let mut identity = (prefix.len() as u64).to_le_bytes().to_vec();
        identity.extend(prefix);
        identity.extend(inputs);
        Ok(Self(sha256_hex(&identity)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Hit,
    Missing,
    Invalid,
}

pub struct Prepared<T> {
    pub value: T,
    pub source: Source,
}

fn regular(path: &Path) -> Result<bool, Error> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(Error::Cache(
            "prepared-content path is not an ordinary file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn load<T>(
    path: &Path,
    key: &Key,
    limit: usize,
    decode: &impl Fn(&[u8]) -> Result<T, Error>,
) -> Result<Prepared<Option<T>>, Error> {
    if !regular(path)? {
        return Ok(Prepared {
            value: None,
            source: Source::Missing,
        });
    }
    let mut file = File::open(path)?;
    let mut inspect = || -> Result<T, Error> {
        let length = file.metadata()?.len();
        if length < 4 || length > (limit + MAX_HEADER + 4) as u64 {
            return Err(Error::Cache("prepared-content file bound"));
        }
        let mut count = [0; 4];
        file.read_exact(&mut count)?;
        let count = u32::from_le_bytes(count) as usize;
        if count == 0 || count > MAX_HEADER || count as u64 + 4 > length {
            return Err(Error::Cache("prepared-content header bound"));
        }
        let mut header = vec![0; count];
        file.read_exact(&mut header)?;
        let header: serde_json::Value =
            serde_json::from_slice(&header).map_err(|_| Error::Cache("prepared-content header"))?;
        let size = length - count as u64 - 4;
        if header["format"] != FORMAT
            || header["key"] != key.0
            || header["bytes"].as_u64() != Some(size)
            || size > limit as u64
        {
            return Err(Error::Cache("prepared-content identity or length"));
        }
        let mut bytes = Vec::new();
        (&mut file).take(limit as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != size || header["sha256"] != sha256_hex(&bytes) {
            return Err(Error::Cache("prepared-content digest"));
        }
        decode(&bytes)
    };
    // Corrupt/stale/semantically invalid cache data is rebuilt. Opening a file
    // that is inaccessible or not ordinary was already rejected above.
    Ok(match inspect() {
        Ok(value) => Prepared {
            value: Some(value),
            source: Source::Hit,
        },
        Err(_) => Prepared {
            value: None,
            source: Source::Invalid,
        },
    })
}

struct Partial(PathBuf);
impl Drop for Partial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// One process builds a key at a time. Validate before publication, then rename
/// the complete file atomically. An error/cancellation leaves the previous file
/// intact. `decode` must reject invalid models as well as malformed bytes.
pub fn prepare<T>(
    root: &Path,
    key: &Key,
    limit: usize,
    wait: Duration,
    cancelled: impl Fn() -> bool,
    decode: impl Fn(&[u8]) -> Result<T, Error>,
    build: impl FnOnce() -> Result<Vec<u8>, Error>,
) -> Result<Prepared<T>, Error> {
    if limit == 0 || limit > MAX_PAYLOAD {
        return Err(Error::Cache("prepared-content payload bound"));
    }
    let check = || {
        if cancelled() {
            Err(Error::Cache("prepared-content preparation cancelled"))
        } else {
            Ok(())
        }
    };
    check()?;
    std::fs::create_dir_all(root)?;
    if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        return Err(Error::Cache("prepared-content root is a link"));
    }
    let path = root.join(format!("{}.bin", key.0));
    let lock_path = root.join(format!("{}.lock", key.0));
    regular(&lock_path)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    let started = Instant::now();
    loop {
        check()?;
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < wait => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::Cache("prepared-content builder wait timed out"));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    let cached = load(&path, key, limit, &decode)?;
    if let Some(value) = cached.value {
        check()?;
        return Ok(Prepared {
            value,
            source: Source::Hit,
        });
    }
    check()?;
    let bytes = build()?;
    check()?;
    if bytes.len() > limit {
        return Err(Error::Cache("built prepared content exceeds bound"));
    }
    let value = decode(&bytes)?;
    let header = serde_json::to_vec(&json!({
        "format": FORMAT, "key": key.0, "bytes": bytes.len(), "sha256": sha256_hex(&bytes)
    }))
    .map_err(|_| Error::Cache("prepared-content header"))?;
    if header.len() > MAX_HEADER {
        return Err(Error::Cache("prepared-content header bound"));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let partial = Partial(root.join(format!(
        "{}.{}.{}.partial",
        key.0,
        std::process::id(),
        nonce
    )));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial.0)?;
    file.write_all(&(header.len() as u32).to_le_bytes())?;
    file.write_all(&header)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    check()?;
    regular(&path)?;
    std::fs::rename(&partial.0, &path)?;
    Ok(Prepared {
        value,
        source: cached.source,
    })
}

#[cfg(test)]
mod tests;
