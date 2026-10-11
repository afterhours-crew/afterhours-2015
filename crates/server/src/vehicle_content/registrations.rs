// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_world::content::{
    self, Error, Message, Registration, Registrations, SubLevelNames,
    bindings::{Bindings, name_key},
};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    pub name: u16,
    pub level: u16,
    pub stream: u16,
    pub asset_bundle: u16,
    pub content_key: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Registry {
    names: BTreeMap<Vec<u8>, u16>,
    keys: BTreeMap<u32, Vec<u8>>,
    levels: BTreeMap<u16, Registration>,
    streams: BTreeSet<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prepared {
    pub bindings: Vec<Binding>,
    pub messages: Vec<Message>,
}

fn normalized(name: &[u8]) -> Result<Vec<u8>, Error> {
    name_key(name)?;
    if name.is_empty() {
        return Err(Error::Shape);
    }
    Ok(name.to_ascii_lowercase())
}

fn vehicle_entry(name: u16, level: u16, stream: u16) -> Registration {
    Registration {
        handle: name,
        next: level,
        region: 0,
        word: u32::from(stream),
        byte: 0,
        flag: true,
        enum3: 3,
        word2: 0,
        bit: true,
        enum2: 1,
        word3: 0,
        text: vec![],
    }
}

fn asset_bundle(stream: u16) -> Result<u16, Error> {
    let asset = stream.checked_sub(3).ok_or(Error::Unsupported)?;
    if asset >= 2048 || asset == 2003 {
        return Err(Error::Unsupported);
    }
    Ok(asset)
}

impl Registry {
    pub fn from_messages(messages: &[Message]) -> Result<Self, Error> {
        Bindings::from_messages(messages)?;
        let mut result = Self::default();
        for message in messages {
            match message {
                Message::Names(batch) => {
                    for (id, name) in &batch.entries {
                        let name = normalized(name)?;
                        let key = name_key(&name)?;
                        if result.names.insert(name.clone(), *id).is_some()
                            || result.keys.insert(key, name).is_some()
                        {
                            return Err(Error::Shape);
                        }
                    }
                }
                Message::Registrations(batch) => {
                    for entry in &batch.entries {
                        result.streams.insert(entry.word as u16);
                        result.levels.insert(entry.next, entry.clone());
                    }
                }
                Message::LoadLevel(_) => {}
            }
        }
        Ok(result)
    }

    fn root(&self, name: &[u8]) -> Result<Option<&Registration>, Error> {
        let Some(id) = self.names.get(name) else {
            return Ok(None);
        };
        let mut matches = self
            .levels
            .values()
            .filter(|entry| entry.handle == *id && entry.region == 0);
        let result = matches.next();
        if matches.next().is_some() {
            return Err(Error::Shape);
        }
        Ok(result)
    }
    pub fn mesh_level(&self, name: &str) -> Result<u16, Error> {
        self.root(&normalized(name.as_bytes())?)?
            .map(|entry| entry.next)
            .ok_or(Error::Unsupported)
    }
    pub fn prepare(&mut self, names: &[&str]) -> Result<Prepared, Error> {
        if names.len() > 32 {
            return Err(Error::Bound);
        }
        let mut staged = self.clone();
        let mut result = Prepared {
            bindings: Vec::with_capacity(names.len()),
            messages: Vec::new(),
        };
        for name in names {
            result
                .bindings
                .push(staged.vehicle(name, &mut result.messages)?);
        }
        for message in &result.messages {
            message.encode()?;
        }
        *self = staged;
        Ok(result)
    }

    fn vehicle(&mut self, requested: &str, messages: &mut Vec<Message>) -> Result<Binding, Error> {
        let name = normalized(requested.as_bytes())?;
        let key = name_key(&name)?;
        if self.keys.get(&key).is_some_and(|other| *other != name) {
            return Err(Error::Shape);
        }
        if let Some(entry) = self.root(&name)? {
            let stream = entry.word as u16;
            if *entry != vehicle_entry(entry.handle, entry.next, stream) {
                return Err(Error::Unsupported);
            }
            return Ok(Binding {
                name: entry.handle,
                level: entry.next,
                stream,
                asset_bundle: asset_bundle(stream)?,
                content_key: key,
            });
        }
        if self.levels.len() >= content::MAX_ITEMS {
            return Err(Error::Bound);
        }
        let level = self
            .levels
            .last_key_value()
            .map_or(0, |(id, _)| *id)
            .checked_add(1)
            .filter(|id| *id != u16::MAX)
            .ok_or(Error::Bound)?;
        let mut stream = self
            .streams
            .last()
            .copied()
            .unwrap_or(3)
            .max(3)
            .checked_add(1)
            .ok_or(Error::Bound)?;
        if stream == 2006 {
            stream += 1;
        }
        let asset_bundle = asset_bundle(stream)?;
        let handle = if let Some(id) = self.names.get(&name) {
            *id
        } else {
            if self.names.len() >= content::MAX_ITEMS {
                return Err(Error::Bound);
            }
            let id = match self.names.values().max() {
                Some(id) => id.checked_add(1).ok_or(Error::Bound)?,
                None => 0,
            };
            self.names.insert(name.clone(), id);
            self.keys.insert(key, name);
            messages.push(Message::Names(SubLevelNames {
                word: 1,
                entries: vec![(id, requested.as_bytes().to_vec())],
            }));
            id
        };
        let entry = vehicle_entry(handle, level, stream);
        self.levels.insert(level, entry.clone());
        self.streams.insert(stream);
        messages.push(Message::Registrations(Registrations {
            header: 4,
            entries: vec![entry],
        }));
        Ok(Binding {
            name: handle,
            level,
            stream,
            asset_bundle,
            content_key: key,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
