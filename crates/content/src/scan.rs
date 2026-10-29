// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One pass over every EBX asset: partition GUID and primary class by name,
//! so import pointers and asset classes can be resolved.
use crate::Error;
use nfs_frostbite::Limits;
use nfs_frostbite::ebx::{Object, Partition, Pointer};
use nfs_frostbite::install::{EbxLocation, Installation, Store};
use std::collections::HashMap;

pub struct Assets {
    pub(crate) scene_cache: crate::graph::scenes::cache::Cache,
    store: Store,
    index: HashMap<String, Vec<EbxLocation>>,
    by_guid: HashMap<[u8; 16], String>,
    primary: HashMap<String, String>,
    objects: HashMap<String, Vec<Object>>,
    limits: Limits,
}

impl Assets {
    /// Resolve either kind of graph edge, retaining object identity for cycle checks.
    pub fn target(
        &mut self,
        source: &str,
        pointer: Pointer,
    ) -> Result<Option<(String, usize)>, Error> {
        match pointer {
            Pointer::Null => Ok(None),
            Pointer::Internal(index) => {
                if index >= self.objects(source)?.len() {
                    return Err(Error::Content(format!(
                        "object index out of range in {source}"
                    )));
                }
                Ok(Some((source.to_owned(), index)))
            }
            Pointer::Import {
                partition,
                instance,
            } => {
                let name =
                    self.by_guid.get(&partition).cloned().ok_or_else(|| {
                        Error::Content("graph reference to unknown partition".into())
                    })?;
                let index = self
                    .objects(&name)?
                    .iter()
                    .position(|o| o.guid == Some(instance))
                    .ok_or_else(|| Error::Content(format!("instance missing from {name}")))?;
                Ok(Some((name, index)))
            }
        }
    }

    pub fn scan(install: &Installation) -> Result<Self, Error> {
        let limits = *install.limits();
        let index = install.ebx_index()?;
        let mut store = Store::open(install)?;
        let mut by_guid = HashMap::with_capacity(index.len());
        let mut primary = HashMap::with_capacity(index.len());
        let mut names: Vec<&String> = index.keys().collect();
        names.sort();
        for name in names {
            // The first listing is authoritative, as for every reader of a name.
            let data = store.bytes(&index[name][0].entry)?;
            let part = Partition::parse(&data, &limits)?;
            if let Some(other) = by_guid.insert(part.guid, name.clone()) {
                return Err(Error::Content(format!(
                    "partition GUID shared by {other} and {name}"
                )));
            }
            primary.insert(
                name.clone(),
                part.primary_class().unwrap_or_default().to_owned(),
            );
        }
        Ok(Self {
            scene_cache: Default::default(),
            store,
            index,
            by_guid,
            primary,
            objects: HashMap::new(),
            limits,
        })
    }

    /// Asset names whose primary class is `class`, sorted.
    pub fn named(&self, class: &str) -> Vec<String> {
        let mut out: Vec<_> = self
            .primary
            .iter()
            .filter(|(_, c)| *c == class)
            .map(|(n, _)| n.clone())
            .collect();
        out.sort();
        out
    }

    pub fn objects(&mut self, name: &str) -> Result<&[Object], Error> {
        if !self.objects.contains_key(name) {
            let location = self
                .index
                .get(name)
                .ok_or_else(|| Error::Content(format!("asset {name} is missing")))?;
            let data = self.store.bytes(&location[0].entry)?;
            let objects = Partition::parse(&data, &self.limits)?.objects(&self.limits)?;
            self.objects.insert(name.to_owned(), objects);
        }
        Ok(&self.objects[name])
    }

    /// The object an import pointer names, with its asset name.
    pub fn resolve(&mut self, pointer: Option<Pointer>) -> Result<(String, Object), Error> {
        let Some(Pointer::Import {
            partition,
            instance,
        }) = pointer
        else {
            return Err(Error::Content("expected an import reference".into()));
        };
        let name = self.by_guid.get(&partition).cloned().ok_or_else(|| {
            Error::Content(format!(
                "reference to unknown partition {}",
                nfs_frostbite::hex(&partition)
            ))
        })?;
        let object = self
            .objects(&name)?
            .iter()
            .find(|o| o.guid == Some(instance))
            .cloned()
            .ok_or_else(|| Error::Content(format!("instance missing from {name}")))?;
        Ok((name, object))
    }
}
