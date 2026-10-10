// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use std::collections::{BTreeMap, BTreeSet};

mod actors;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Object {
    pub initial: Initial,
    pub current: Update,
}

#[derive(Clone, Debug)]
pub struct World {
    objects: BTreeMap<u16, Object>,
    levels: BTreeSet<u16>,
    next: usize,
}
impl Default for World {
    fn default() -> Self {
        Self {
            objects: BTreeMap::new(),
            levels: BTreeSet::new(),
            next: 1,
        }
    }
}
impl World {
    pub fn len(&self) -> usize {
        self.objects.len()
    }
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
    pub fn get(&self, id: u16) -> Option<&Object> {
        self.objects.get(&id)
    }
    pub fn iter(&self) -> impl Iterator<Item = (u16, &Object)> {
        self.objects.iter().map(|(id, object)| (*id, object))
    }
    pub fn scene(&self, content_key: u32) -> Option<u16> {
        let mut found = self
            .objects
            .iter()
            .filter_map(|(id, object)| match &object.initial {
                Initial::SubLevel { prefix, .. } if prefix.content_key == content_key => Some(*id),
                _ => None,
            });
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }
    pub fn scene_endpoint(&self, content_key: u32, index: usize) -> Option<(u16, u16)> {
        let id = self.scene(content_key)?;
        let Initial::SubLevel { fields, .. } = &self.objects.get(&id)?.initial else {
            return None;
        };
        fields
            .get(index)
            .and_then(sublevel::Initial::rpc)
            .map(|rpc| (id, rpc.selector))
    }
    pub fn spawn(&mut self, initial: Initial, current: Update) -> Result<Record, Error> {
        if matches!(initial.kind(), Kind::SubLevel | Kind::Entity)
            || matches!(current.kind(), Kind::SubLevel | Kind::Entity)
        {
            return Err(Error::Unsupported);
        }
        if self.next > MAX_OBJECTS {
            return Err(Error::Bound);
        }
        let record = Record {
            id: self.next as u16,
            initial: Some(initial.clone()),
            update: current.clone(),
        };
        record.encode()?;
        self.objects.insert(record.id, Object { initial, current });
        self.next += 1;
        Ok(record)
    }
    pub fn change(&mut self, id: u16, delta: Update) -> Result<Option<Record>, Error> {
        let object = self.objects.get(&id).ok_or(Error::UnknownObject)?;
        let record = Record {
            id,
            initial: None,
            update: delta,
        };
        record.encode()?;
        if let Update::Vehicle { binding, fields } = &record.update {
            let (creation, content) = vehicle(object)?;
            if object.current.entity_binding() != Some(binding) {
                return Err(Error::TypeMismatch);
            }
            let known = |id| self.objects.contains_key(&id);
            creation.validate_references(known)?;
            let mut state = vehicle::state::State::new(
                binding.vehicle_profile()?.clone(),
                creation.body,
                known,
            )?;
            let before = state.clone();
            state.apply(fields.clone(), known)?;
            if state == before {
                return Ok(None);
            }
            let current = Record::from_vehicle(
                id,
                vehicle::EntityCreation {
                    prefix: creation.prefix,
                    body: state.snapshot(),
                },
                &content,
            )?;
            self.objects.insert(
                id,
                Object {
                    initial: current.initial.ok_or(Error::Shape)?,
                    current: current.update,
                },
            );
            return Ok(Some(record));
        }
        if let Update::Entity { binding, fields } = &record.update {
            let (creation, content) = entity(object)?;
            if object.current.entity_binding() != Some(binding) {
                return Err(Error::TypeMismatch);
            }
            let mut state =
                entity::state::State::new(creation, &content, |id| self.objects.contains_key(&id))?;
            let before = state.clone();
            state.apply(fields.clone())?;
            if state == before {
                return Ok(None);
            }
            let current = Record::from_entity(id, state.snapshot(), &content)?;
            self.objects.insert(
                id,
                Object {
                    initial: current.initial.ok_or(Error::Shape)?,
                    current: current.update,
                },
            );
            return Ok(Some(record));
        }
        if let Update::SubLevel { profile, fields } = &record.update {
            let (creation, content) = scene(object)?;
            if content.profile(creation.prefix.content_key)? != profile {
                return Err(Error::TypeMismatch);
            }
            let known = |value| self.objects.contains_key(&value);
            let level = |value| self.levels.contains(&value);
            let bindings = sublevel::state::Bindings {
                ghost: &known,
                level: &level,
            };
            let mut state = sublevel::state::State::new(creation, &content, bindings)?;
            if !state.apply(fields.clone(), bindings)? {
                return Ok(None);
            }
            let current = Record::from_scene(id, state.snapshot(bindings)?, &content)?;
            self.objects.insert(
                id,
                Object {
                    initial: current.initial.ok_or(Error::Shape)?,
                    current: current.update,
                },
            );
            return Ok(Some(record));
        }
        let mut current = object.current.clone();
        current.merge(&record.update)?;
        if current == object.current {
            return Ok(None);
        }
        self.objects
            .get_mut(&id)
            .ok_or(Error::UnknownObject)?
            .current = current;
        Ok(Some(record))
    }
    pub fn remove(&mut self, id: u16) -> Result<Object, Error> {
        if !self.objects.contains_key(&id) {
            return Err(Error::UnknownObject);
        }
        self.validate_scenes(Some(id), None)?;
        self.validate_entities(Some(id))?;
        self.objects.remove(&id).ok_or(Error::UnknownObject)
    }
    pub fn snapshot(&self) -> Vec<Record> {
        self.objects
            .iter()
            .map(|(id, o)| Record {
                id: *id,
                initial: Some(o.initial.clone()),
                update: o.current.clone(),
            })
            .collect()
    }
}

impl Update {
    fn entity_binding(&self) -> Option<&std::sync::Arc<entity::creation::Binding>> {
        match self {
            Self::Entity { binding, .. } | Self::Vehicle { binding, .. } => Some(binding),
            _ => None,
        }
    }
}

fn entity(
    object: &Object,
) -> Result<(entity::creation::Creation, entity::creation::Content), Error> {
    let (
        Initial::Entity {
            prefix,
            fields: initial,
        },
        Update::Entity { binding, fields },
    ) = (&object.initial, &object.current)
    else {
        return Err(Error::TypeMismatch);
    };
    Ok((
        entity::creation::Creation {
            prefix: prefix.clone(),
            body: entity::Body {
                initial: Some(initial.clone()),
                updates: fields.clone(),
            },
        },
        binding.content(),
    ))
}

fn scene(object: &Object) -> Result<(sublevel::Creation, sublevel::Content), Error> {
    let (
        Initial::SubLevel {
            prefix,
            fields: initial,
        },
        Update::SubLevel { profile, fields },
    ) = (&object.initial, &object.current)
    else {
        return Err(Error::TypeMismatch);
    };
    Ok((
        sublevel::Creation {
            prefix: prefix.clone(),
            body: sublevel::Body {
                initial: Some(initial.clone()),
                updates: fields.clone(),
            },
        },
        sublevel::Content::new(vec![(prefix.content_key, profile.clone())])?,
    ))
}

fn vehicle(object: &Object) -> Result<(vehicle::EntityCreation, entity::creation::Content), Error> {
    let (Initial::Vehicle { prefix, creation }, Update::Vehicle { binding, fields }) =
        (&object.initial, &object.current)
    else {
        return Err(Error::TypeMismatch);
    };
    Ok((
        vehicle::EntityCreation {
            prefix: prefix.clone(),
            body: vehicle::Body {
                creation: Some(creation.clone()),
                updates: fields.clone(),
            },
        },
        binding.content(),
    ))
}

impl World {
    pub fn spawn_vehicle(
        &mut self,
        creation: vehicle::EntityCreation,
        content: &entity::creation::Content,
    ) -> Result<Record, Error> {
        if self.next > MAX_OBJECTS {
            return Err(Error::Bound);
        }
        creation.encode(content)?;
        let known = |id| self.objects.contains_key(&id);
        creation.validate_references(known)?;
        let profile = content.vehicle_profile(creation.prefix.asset)?.clone();
        let state = vehicle::state::State::new(profile, creation.body, known)?;
        let record = Record::from_vehicle(
            self.next as u16,
            vehicle::EntityCreation {
                prefix: creation.prefix,
                body: state.snapshot(),
            },
            content,
        )?;
        self.objects.insert(
            record.id,
            Object {
                initial: record.initial.clone().ok_or(Error::Shape)?,
                current: record.update.clone(),
            },
        );
        self.next += 1;
        Ok(record)
    }
    pub fn spawn_entity(
        &mut self,
        creation: entity::creation::Creation,
        content: &entity::creation::Content,
    ) -> Result<Record, Error> {
        if self.next > MAX_OBJECTS {
            return Err(Error::Bound);
        }
        let state =
            entity::state::State::new(creation, content, |id| self.objects.contains_key(&id))?;
        let record = Record::from_entity(self.next as u16, state.snapshot(), content)?;
        self.objects.insert(
            record.id,
            Object {
                initial: record.initial.clone().ok_or(Error::Shape)?,
                current: record.update.clone(),
            },
        );
        self.next += 1;
        Ok(record)
    }

    fn validate_entities(&self, removed: Option<u16>) -> Result<(), Error> {
        for (id, object) in &self.objects {
            if Some(*id) != removed && object.initial.kind() == Kind::Entity {
                if matches!(object.initial, Initial::Vehicle { .. }) {
                    let (creation, content) = vehicle(object)?;
                    let known = |id| removed != Some(id) && self.objects.contains_key(&id);
                    creation.validate_references(known)?;
                    vehicle::state::State::new(
                        content.vehicle_profile(creation.prefix.asset)?.clone(),
                        creation.body,
                        known,
                    )?;
                    continue;
                }
                let (creation, content) = entity(object)?;
                entity::state::State::new(creation, &content, |id| {
                    removed != Some(id) && self.objects.contains_key(&id)
                })?;
            }
        }
        Ok(())
    }

    pub fn register_levels(&mut self, levels: &[u16]) -> Result<(), Error> {
        if levels.len() > sublevel::MAX_PROFILES {
            return Err(Error::Bound);
        }
        let mut next = self.levels.clone();
        for &level in levels {
            if level == u16::MAX {
                return Err(Error::Shape);
            }
            next.insert(level);
        }
        if next.len() > sublevel::MAX_PROFILES {
            return Err(Error::Bound);
        }
        self.levels = next;
        Ok(())
    }

    pub fn unregister_level(&mut self, level: u16) -> Result<(), Error> {
        if !self.levels.contains(&level) {
            return Err(Error::UnknownObject);
        }
        self.validate_scenes(None, Some(level))?;
        self.levels.remove(&level);
        Ok(())
    }

    pub fn spawn_scene(
        &mut self,
        creation: sublevel::Creation,
        content: &sublevel::Content,
    ) -> Result<Record, Error> {
        if self.next > MAX_OBJECTS {
            return Err(Error::Bound);
        }
        if self.objects.values().any(|object| matches!(&object.initial, Initial::SubLevel { prefix, .. } if prefix.level_id == creation.prefix.level_id)) {
            return Err(Error::DuplicateObject);
        }
        let id = self.next as u16;
        let known = |value| self.objects.contains_key(&value);
        let level = |value| self.levels.contains(&value);
        let bindings = sublevel::state::Bindings {
            ghost: &known,
            level: &level,
        };
        let state = sublevel::state::State::new(creation, content, bindings)?;
        let record = Record::from_scene(id, state.snapshot(bindings)?, content)?;
        self.objects.insert(
            id,
            Object {
                initial: record.initial.clone().ok_or(Error::Shape)?,
                current: record.update.clone(),
            },
        );
        self.next += 1;
        Ok(record)
    }

    fn validate_scenes(
        &self,
        removed: Option<u16>,
        level_removed: Option<u16>,
    ) -> Result<(), Error> {
        let known = |id| removed != Some(id) && self.objects.contains_key(&id);
        let level = |id| level_removed != Some(id) && self.levels.contains(&id);
        let bindings = sublevel::state::Bindings {
            ghost: &known,
            level: &level,
        };
        for (id, object) in &self.objects {
            if Some(*id) != removed && object.initial.kind() == Kind::SubLevel {
                let (creation, content) = scene(object)?;
                sublevel::state::State::new(creation, &content, bindings)?;
            }
        }
        Ok(())
    }
}

fn replace<T: Clone>(old: &mut Option<T>, new: &Option<T>) {
    if let Some(v) = new {
        *old = Some(v.clone());
    }
}
impl Update {
    fn merge(&mut self, new: &Self) -> Result<(), Error> {
        match (self, new) {
            (Self::Participant(a), Self::Participant(b)) => {
                replace(&mut a.player, &b.player);
                replace(&mut a.entity, &b.entity);
                replace(&mut a.identity, &b.identity);
            }
            (Self::Actor(a), Self::Actor(b)) => {
                replace(&mut a.flag_a, &b.flag_a);
                replace(&mut a.flag_b, &b.flag_b);
                replace(&mut a.word_pair, &b.word_pair);
                replace(&mut a.word_c, &b.word_c);
                replace(&mut a.references_a, &b.references_a);
                replace(&mut a.references_b, &b.references_b);
            }
            (Self::Player(a), Self::Player(b)) => {
                if let Some(b) = &b.base {
                    let a = a.base.get_or_insert_with(PlayerBaseUpdate::default);
                    replace(&mut a.value16, &b.value16);
                    replace(&mut a.name_identity, &b.name_identity);
                    replace(&mut a.identity_a, &b.identity_a);
                    replace(&mut a.value8, &b.value8);
                    replace(&mut a.configured_pair, &b.configured_pair);
                    a.empty_assets |= b.empty_assets;
                    a.empty_structured |= b.empty_structured;
                    replace(&mut a.asset, &b.asset);
                    a.empty_local |= b.empty_local;
                    replace(&mut a.identity_b, &b.identity_b);
                }
                a.component_present |= b.component_present;
            }
            _ => return Err(Error::TypeMismatch),
        }
        Ok(())
    }
}
