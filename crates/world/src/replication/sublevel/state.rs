// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Body, Creation, Error, Initial, MAX_RECORD_BITS, Prefix, Profile, Update};

#[derive(Clone, Copy)]
pub struct Bindings<'a> {
    pub ghost: &'a dyn Fn(u16) -> bool,
    pub level: &'a dyn Fn(u16) -> bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    prefix: Prefix,
    profile: Profile,
    initial: Vec<Initial>,
    current: Vec<Option<Update>>,
}
impl State {
    pub fn new(
        creation: Creation,
        content: &super::Content,
        bindings: Bindings<'_>,
    ) -> Result<Self, Error> {
        creation.encode(content)?;
        references(&creation, bindings)?;
        let profile = content.profile(creation.prefix.content_key)?.clone();
        let mut current = vec![None; profile.kinds().len()];
        if current.len() == 1 {
            current[0] = Some(Update::Noop);
        }
        let mut result = Self {
            prefix: creation.prefix,
            profile,
            initial: creation.body.initial.ok_or(Error::Shape)?,
            current,
        };
        result.apply(creation.body.updates, bindings)?;
        Ok(result)
    }

    pub fn apply(
        &mut self,
        updates: Vec<Option<Update>>,
        bindings: Bindings<'_>,
    ) -> Result<bool, Error> {
        let delta = Body {
            initial: None,
            updates,
        };
        delta.encode(&self.profile)?;
        let mut next = self.clone();
        for (index, update) in delta.updates.into_iter().enumerate() {
            match (&mut next.initial[index], update) {
                (Initial::BoolProperty(old), Some(Update::BoolProperty(Some(value)))) => {
                    *old = Some(value)
                }
                (Initial::FloatProperty(old), Some(Update::FloatProperty(Some(value)))) => {
                    *old = Some(value)
                }
                (Initial::I32Property(old), Some(Update::I32Property(Some(value)))) => {
                    *old = Some(value)
                }
                (Initial::RpcMap { value: old, .. }, Some(Update::ReferenceMap(Some(value)))) => {
                    *old = value
                }
                (_, Some(Update::LevelReference(Some(value)))) => {
                    next.current[index] = Some(Update::LevelReference(Some(value)));
                }
                (_, Some(Update::FourReferences(values))) if values.iter().any(Option::is_some) => {
                    let old = next.current[index].get_or_insert(Update::FourReferences([None; 4]));
                    let Update::FourReferences(old) = old else {
                        return Err(Error::TypeMismatch);
                    };
                    for (old, value) in old.iter_mut().zip(values) {
                        if value.is_some() {
                            *old = value;
                        }
                    }
                }
                (_, Some(Update::Set64(Some(value)))) => {
                    next.current[index] = Some(Update::Set64(Some(value)))
                }
                (_, Some(Update::PursuitMaps(values))) if values.iter().any(Option::is_some) => {
                    let old = next.current[index].get_or_insert(Update::PursuitMaps([None, None]));
                    let Update::PursuitMaps(old) = old else {
                        return Err(Error::TypeMismatch);
                    };
                    for (old, value) in old.iter_mut().zip(values) {
                        if value.is_some() {
                            *old = value;
                        }
                    }
                }
                _ => (),
            }
        }
        next.snapshot(bindings)?;
        let changed = *self != next;
        *self = next;
        Ok(changed)
    }

    pub fn snapshot(&self, bindings: Bindings<'_>) -> Result<Creation, Error> {
        let creation = Creation {
            prefix: self.prefix.clone(),
            body: Body {
                initial: Some(self.initial.clone()),
                updates: self.current.clone(),
            },
        };
        if creation.prefix.encode()?.len() + creation.body.encode(&self.profile)?.len()
            > MAX_RECORD_BITS
        {
            return Err(Error::Bound);
        }
        references(&creation, bindings)?;
        Ok(creation)
    }
}

fn references(creation: &Creation, bindings: Bindings<'_>) -> Result<(), Error> {
    let ghost = |id| id == 0 || (bindings.ghost)(id);
    let level = |id| id == u16::MAX || (bindings.level)(id);
    if !(bindings.level)(creation.prefix.level_id)
        || creation
            .prefix
            .blueprint
            .as_ref()
            .is_some_and(|v| !ghost(v.ghost))
    {
        return Err(Error::UnknownObject);
    }
    for value in creation.body.initial.as_ref().ok_or(Error::Shape)? {
        let valid = match value {
            Initial::RpcReferences { value, .. }
            | Initial::RpcSpawnInactive {
                references: value, ..
            } => value.iter().all(|v| ghost(*v)),
            Initial::RpcPairs { value, .. } => value.iter().all(|(a, b)| ghost(*a) && ghost(*b)),
            Initial::RpcTagged { value, .. } => value.iter().all(|(id, _)| ghost(*id)),
            Initial::RpcMap { value, .. } => value.iter().all(|(id, _)| ghost(*id)),
            _ => true,
        };
        if !valid {
            return Err(Error::UnknownObject);
        }
    }
    for value in creation.body.updates.iter().flatten() {
        let valid = match value {
            Update::LevelReference(value) => value.is_none_or(level),
            Update::ReferenceMap(value) => value
                .as_ref()
                .is_none_or(|v| v.iter().all(|(id, _)| ghost(*id))),
            Update::FourReferences(value) => value.iter().flatten().all(|id| ghost(*id)),
            Update::PursuitMaps(value) => {
                value.iter().flatten().flatten().all(|(_, id)| ghost(*id))
            }
            _ => true,
        };
        if !valid {
            return Err(Error::UnknownObject);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
