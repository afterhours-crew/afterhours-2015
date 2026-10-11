// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Body, Error, Initial, Profile, Update};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    profile: Profile,
    body: Body,
}
fn refs(body: &Body, known: impl Fn(u16) -> bool) -> Result<(), Error> {
    let valid = |id: u16| id == 0 || known(id);
    if body.creation.as_ref().is_some_and(|c| {
        c.fields
            .iter()
            .any(|v| matches!(v,Initial::Root(root) if root.reference.is_some_and(|id|!valid(id))))
    }) || body.updates.iter().any(
        |v| matches!(v,Some(Update::Tagged(Some(t))) if t.reference.is_some_and(|id|!valid(id))),
    ) {
        return Err(Error::UnknownObject);
    }
    Ok(())
}
fn field<T>(old: &mut Option<T>, new: Option<T>) {
    if new.is_some() {
        *old = new;
    }
}
fn merge(old: &mut Option<Update>, new: Option<Update>) {
    let Some(new) = new else { return };
    let Some(old) = old else {
        *old = Some(new);
        return;
    };
    match (old, new) {
        (Update::Root(a), Update::Root(b)) => field(a, b),
        (Update::Part(a), Update::Part(b)) => field(a, b),
        (Update::TripleNibbles(a), Update::TripleNibbles(b)) => field(a, b),
        (Update::Bool(a), Update::Bool(b)) | (Update::Wheel(a), Update::Wheel(b)) => field(a, b),
        (Update::Tuning(a), Update::Tuning(b)) => field(a, b),
        (Update::Tagged(a), Update::Tagged(b)) => field(a, b),
        (Update::NibblesGuid(a), Update::NibblesGuid(b)) => field(a, b),
        (Update::Mesh(a), Update::Mesh(b)) => field(a, b),
        (Update::Index { index: a, value: b }, Update::Index { index: c, value: d }) => {
            field(a, c);
            field(b, d);
        }
        (Update::GuidFloat { guid: a, value: b }, Update::GuidFloat { guid: c, value: d }) => {
            field(a, c);
            field(b, d);
        }
        (Update::Chassis(a), Update::Chassis(b)) => {
            field(&mut a.physics, b.physics);
            field(&mut a.flag, b.flag);
            field(&mut a.custom, b.custom);
            field(&mut a.pair, b.pair);
        }
        (Update::Appearance(a), Update::Appearance(b)) => {
            field(&mut a.paint, b.paint);
            field(&mut a.palette1, b.palette1);
            field(&mut a.palette2, b.palette2);
            field(&mut a.wrap, b.wrap);
            field(&mut a.index, b.index);
            field(&mut a.indices, b.indices);
        }
        _ => (),
    }
}
impl State {
    pub fn new(profile: Profile, body: Body, known: impl Fn(u16) -> bool) -> Result<Self, Error> {
        if body.creation.is_none() {
            return Err(Error::Shape);
        }
        body.encode(&profile)?;
        refs(&body, known)?;
        Ok(Self { profile, body })
    }
    pub fn apply(
        &mut self,
        updates: Vec<Option<Update>>,
        known: impl Fn(u16) -> bool,
    ) -> Result<(), Error> {
        let delta = Body {
            creation: None,
            updates,
        };
        delta.encode(&self.profile)?;
        refs(&delta, known)?;
        let mut next = self.body.clone();
        for (old, new) in next.updates.iter_mut().zip(delta.updates) {
            merge(old, new);
        }
        next.encode(&self.profile)?;
        self.body = next;
        Ok(())
    }
    pub fn snapshot(&self) -> Body {
        self.body.clone()
    }
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
}
