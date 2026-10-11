// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Initial, Kind, MAX_OBJECTS, Record, entity, sublevel};
use crate::bits::BitWriter;
pub use nfs_protocol::world::ghost::Setup;
use nfs_protocol::world::{
    BitSpan,
    ghost::{Limits, Prefix, Profile, SetupProfile},
};
use std::collections::BTreeMap;

pub const MAX_SECTION_BITS: usize = 1024 * 1024;
pub const MAX_SECTION_RECORDS: usize = 8191;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Section {
    pub float_bits: Option<u32>,
    pub flag: bool,
    pub deleted: Vec<u16>,
    pub setup: Option<Setup>,
    pub records: Vec<Record>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Boundary {
    pub id: u16,
    pub kind: Kind,
    pub start: usize,
    pub initial_end: Option<usize>,
    pub end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decoded {
    pub section: Section,
    pub boundaries: Vec<Boundary>,
    pub bits: usize,
}

type Decoded_ = super::Decoded;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Partial {
    pub declared: usize,
    pub deleted: Vec<u16>,
    pub records: Vec<Record>,
    pub boundaries: Vec<Boundary>,
    pub failure: Option<(Error, usize)>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Bindings {
    objects: BTreeMap<u16, Kind>,
    scenes: BTreeMap<u16, sublevel::Profile>,
    content: Option<std::sync::Arc<sublevel::Content>>,
    entities: BTreeMap<u16, std::sync::Arc<entity::creation::Binding>>,
    entity_content: Option<std::sync::Arc<entity::creation::Content>>,
}

fn id_valid(id: u16) -> Result<(), Error> {
    if id == 0 || usize::from(id) > MAX_OBJECTS {
        Err(Error::Shape)
    } else {
        Ok(())
    }
}

impl Bindings {
    pub fn with_sublevels(content: sublevel::Content) -> Self {
        Self {
            content: Some(std::sync::Arc::new(content)),
            ..Self::default()
        }
    }
    pub fn bind_entity(&mut self, id: u16, asset: entity::creation::Asset) -> Result<(), Error> {
        id_valid(id)?;
        let content = self.entity_content.as_ref().ok_or(Error::Unsupported)?;
        let binding = content.binding(asset)?;
        if self.objects.insert(id, Kind::Entity).is_some() {
            return Err(Error::DuplicateObject);
        }
        self.entities.insert(id, std::sync::Arc::new(binding));
        Ok(())
    }
    pub fn with_entities(mut self, content: entity::creation::Content) -> Self {
        self.entity_content = Some(std::sync::Arc::new(content));
        self
    }
    pub fn len(&self) -> usize {
        self.objects.len()
    }
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
    pub fn get(&self, id: u16) -> Option<Kind> {
        self.objects.get(&id).copied()
    }
    pub fn is_vehicle(&self, id: u16) -> bool {
        self.entities
            .get(&id)
            .is_some_and(|binding| binding.vehicle_profile().is_ok())
    }
    pub fn is_sequence(&self, id: u16) -> bool {
        self.entities.get(&id).is_some_and(|binding| {
            binding
                .profile()
                .is_ok_and(|profile| profile == &crate::sequences::profile())
        })
    }
    pub fn reset(&mut self) {
        self.objects.clear();
        self.scenes.clear();
        self.entities.clear();
    }

    pub fn decode(&mut self, input: BitSpan<'_>) -> Result<Decoded, Error> {
        if input.len() > MAX_SECTION_BITS {
            return Err(Error::Bound);
        }
        let prefix = Prefix::decode(
            input,
            Profile::NFS16_92AA6FF4,
            Limits {
                max_input_bits: MAX_SECTION_BITS,
                max_records: MAX_SECTION_RECORDS as u32,
                max_deletions: MAX_SECTION_RECORDS,
            },
        )
        .map_err(|_| Error::Truncated)?;
        let mut next = self.clone();
        let mut deleted = Vec::new();
        for &id in prefix.deleted() {
            let id = id as u16;
            id_valid(id)?;
            next.objects.remove(&id).ok_or(Error::UnknownObject)?;
            next.scenes.remove(&id);
            next.entities.remove(&id);
            deleted.push(id);
        }
        let first = prefix
            .first_record(SetupProfile::ClientReceivePacked)
            .map_err(|_| Error::Truncated)?;
        let mut cursor = first
            .as_ref()
            .map_or(prefix.remaining().start(), |r| r.body().start() - 14)
            - input.start();
        let setup = first.as_ref().map(|r| r.setup());
        let mut records = Vec::new();
        let mut boundaries = Vec::new();
        for _ in 0..prefix.remaining_records() {
            let span = input.after(cursor).map_err(|_| Error::Truncated)?;
            let id = span.read_u32(0, 13).map_err(|_| Error::Truncated)? as u16;
            id_valid(id)?;
            let bound = next.get(id);
            let create = span.read_u32(13, 1).map_err(|_| Error::Truncated)? != 0;
            if create && bound.is_some() {
                return Err(Error::DuplicateObject);
            }
            let decoded = Record::decode_context(
                span,
                bound,
                next.content.as_deref(),
                next.scenes.get(&id),
                next.entity_content.as_deref(),
                next.entities.get(&id),
            )?;
            let kind = decoded.record.update.kind();
            if create {
                next.objects.insert(id, kind);
                if let Some(profile) = decoded.record.scene_profile() {
                    next.scenes.insert(id, profile.clone());
                }
                if let Some(binding) = decoded.record.entity_binding() {
                    next.entities.insert(id, std::sync::Arc::clone(binding));
                }
            }
            boundaries.push(Boundary {
                id,
                kind,
                start: cursor,
                initial_end: decoded.initial_bits.map(|bits| cursor + bits),
                end: cursor + decoded.bits,
            });
            cursor += decoded.bits;
            records.push(decoded.record);
        }
        *self = next;
        Ok(Decoded {
            section: Section {
                float_bits: prefix.float_bits(),
                flag: prefix.flag(),
                deleted,
                setup,
                records,
            },
            boundaries,
            bits: cursor,
        })
    }

    pub fn decode_partial(&self, input: BitSpan<'_>) -> Result<Partial, Error> {
        if input.len() > MAX_SECTION_BITS {
            return Err(Error::Bound);
        }
        let prefix = Prefix::decode(
            input,
            Profile::NFS16_92AA6FF4,
            Limits {
                max_input_bits: MAX_SECTION_BITS,
                max_records: MAX_SECTION_RECORDS as u32,
                max_deletions: MAX_SECTION_RECORDS,
            },
        )
        .map_err(|_| Error::Truncated)?;
        let mut next = self.clone();
        let deleted: Vec<u16> = prefix.deleted().iter().map(|id| *id as u16).collect();
        for &id in &deleted {
            next.objects.remove(&id);
            next.scenes.remove(&id);
            next.entities.remove(&id);
        }
        let first = prefix
            .first_record(SetupProfile::ClientReceivePacked)
            .map_err(|_| Error::Truncated)?;
        let mut cursor = first
            .as_ref()
            .map_or(prefix.remaining().start(), |r| r.body().start() - 14)
            - input.start();
        let mut records = Vec::new();
        let mut boundaries = Vec::new();
        let mut failure = None;
        for _ in 0..prefix.remaining_records() {
            let result = (|| -> Result<(Decoded_, usize), Error> {
                let span = input.after(cursor).map_err(|_| Error::Truncated)?;
                let id = span.read_u32(0, 13).map_err(|_| Error::Truncated)? as u16;
                id_valid(id)?;
                let bound = next.get(id);
                let create = span.read_u32(13, 1).map_err(|_| Error::Truncated)? != 0;
                if create && bound.is_some() {
                    return Err(Error::DuplicateObject);
                }
                let decoded = Record::decode_context(
                    span,
                    bound,
                    next.content.as_deref(),
                    next.scenes.get(&id),
                    next.entity_content.as_deref(),
                    next.entities.get(&id),
                )?;
                Ok((decoded, cursor))
            })();
            match result {
                Ok((decoded, start)) => {
                    let kind = decoded.record.update.kind();
                    if decoded.record.initial.is_some() {
                        next.objects.insert(decoded.record.id, kind);
                        if let Some(profile) = decoded.record.scene_profile() {
                            next.scenes.insert(decoded.record.id, profile.clone());
                        }
                        if let Some(binding) = decoded.record.entity_binding() {
                            next.entities
                                .insert(decoded.record.id, std::sync::Arc::clone(binding));
                        }
                    }
                    boundaries.push(Boundary {
                        id: decoded.record.id,
                        kind,
                        start,
                        initial_end: decoded.initial_bits.map(|bits| start + bits),
                        end: start + decoded.bits,
                    });
                    cursor += decoded.bits;
                    records.push(decoded.record);
                }
                Err(error) => {
                    failure = Some((error, cursor));
                    break;
                }
            }
        }
        Ok(Partial {
            declared: prefix.remaining_records() as usize,
            deleted,
            records,
            boundaries,
            failure,
        })
    }

    pub fn apply(&mut self, section: &Section) -> Result<(), Error> {
        section.encode()?;
        let mut next = self.clone();
        for &id in &section.deleted {
            next.objects.remove(&id).ok_or(Error::UnknownObject)?;
            next.scenes.remove(&id);
            next.entities.remove(&id);
        }
        for record in &section.records {
            let kind = record.update.kind();
            if let Some(initial) = &record.initial {
                if next.objects.insert(record.id, kind).is_some() {
                    return Err(Error::DuplicateObject);
                }
                if let Some(profile) = record.scene_profile() {
                    if let (Some(content), Initial::SubLevel { prefix, .. }) =
                        (&next.content, initial)
                        && content.profile(prefix.content_key)? != profile
                    {
                        return Err(Error::TypeMismatch);
                    }
                    next.scenes.insert(record.id, profile.clone());
                }
                if let Some(binding) = record.entity_binding() {
                    if let Some(content) = &next.entity_content
                        && content.binding(binding.asset())? != **binding
                    {
                        return Err(Error::TypeMismatch);
                    }
                    next.entities
                        .insert(record.id, std::sync::Arc::clone(binding));
                }
            } else {
                if next.get(record.id).ok_or(Error::UnknownObject)? != kind {
                    return Err(Error::TypeMismatch);
                }
                if record.scene_profile() != next.scenes.get(&record.id) {
                    return Err(Error::TypeMismatch);
                }
                if record.entity_binding() != next.entities.get(&record.id) {
                    return Err(Error::TypeMismatch);
                }
            }
        }
        *self = next;
        Ok(())
    }
}

impl Section {
    pub fn encode(&self) -> Result<BitWriter, Error> {
        let count = self
            .deleted
            .len()
            .checked_add(self.records.len())
            .ok_or(Error::Bound)?;
        if count > MAX_SECTION_RECORDS {
            return Err(Error::Bound);
        }
        if self.records.is_empty() != self.setup.is_none() {
            return Err(Error::Shape);
        }
        let mut w = BitWriter::new();
        w.put_bool(self.float_bits.is_some());
        if let Some(value) = self.float_bits {
            if value >= 1 << 31 {
                return Err(Error::Shape);
            }
            w.put(u64::from(value), 31);
        }
        w.put_bool(self.flag).put(count as u64, 13);
        for &id in &self.deleted {
            id_valid(id)?;
            w.put_bool(true).put(u64::from(id), 13);
        }
        if let Some(setup) = self.setup {
            w.put_bool(false);
            encode_setup(&mut w, setup)?;
        }
        for record in &self.records {
            id_valid(record.id)?;
            let wire = record.encode()?;
            if wire.len() > MAX_SECTION_BITS - w.len() {
                return Err(Error::Bound);
            }
            w.put_span(wire.span());
        }
        Ok(w)
    }

    pub fn frame(&self) -> Result<BitWriter, Error> {
        let section = self.encode()?;
        let mut w = BitWriter::new();
        w.put(1 << crate::frame::GHOST, crate::frame::HANDLERS)
            .put_span(section.span());
        w.align();
        Ok(w)
    }
}

pub fn split_records(records: Vec<Record>, max_frame_bits: usize) -> Result<Vec<Section>, Error> {
    if records.len() > MAX_SECTION_RECORDS || max_frame_bits > MAX_SECTION_BITS {
        return Err(Error::Bound);
    }
    let empty = || Section {
        float_bits: None,
        flag: false,
        deleted: Vec::new(),
        setup: Some(Setup::Packed {
            tag: None,
            width: None,
            axes: [None; 3],
        }),
        records: Vec::new(),
    };
    let mut result = Vec::new();
    let mut section = empty();
    for record in records {
        section.records.push(record);
        if section.frame()?.len() > max_frame_bits {
            let last = section.records.pop().ok_or(Error::Shape)?;
            if section.records.is_empty() {
                return Err(Error::Bound);
            }
            result.push(section);
            section = empty();
            section.records.push(last);
            if section.frame()?.len() > max_frame_bits {
                return Err(Error::Bound);
            }
        }
    }
    if !section.records.is_empty() {
        result.push(section);
    }
    Ok(result)
}

fn encode_setup(w: &mut BitWriter, setup: Setup) -> Result<(), Error> {
    match setup {
        Setup::RawEscape(words) => {
            w.put_bool(true).put(0, 3);
            for word in words {
                w.put(u64::from(word), 32);
            }
        }
        Setup::Packed { tag, width, axes } => {
            let first = axes.iter().position(Option::is_some);
            let selected = match (first, tag, width) {
                (None, None, None) => None,
                (Some(index), Some(tag), Some(width))
                    if tag <= 7
                        && !(index == 0 && tag == 0)
                        && width == [1, 3, 5, 7, 9, 11, 14][usize::from(tag.max(1) - 1)] =>
                {
                    Some((tag, width))
                }
                _ => return Err(Error::Shape),
            };
            for (index, axis) in axes.iter().enumerate() {
                w.put_bool(axis.is_some());
                if let Some(value) = axis {
                    let (tag, width) = selected.ok_or(Error::Shape)?;
                    if *value >= 1 << width {
                        return Err(Error::Shape);
                    }
                    if Some(index) == first {
                        w.put(u64::from(tag), 3);
                    }
                    w.put(u64::from(*value), usize::from(width));
                }
            }
        }
        _ => return Err(Error::Unsupported),
    }
    Ok(())
}
