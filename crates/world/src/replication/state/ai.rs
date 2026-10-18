// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use players::ai::{Bound, Managers};

impl World {
    pub(in crate::replication) fn bind_ai_managers(
        &mut self,
        managers: Managers,
        participant: u16,
        actor: u16,
    ) -> Result<Bound, Error> {
        if !matches!(self.get(participant).map(|o| &o.initial),
            Some(Initial::Participant(p)) if p.player == 0)
            || !matches!(self.get(actor).map(|o| &o.initial),
                Some(Initial::Actor(a)) if a.participant == participant)
        {
            return Err(Error::UnknownObject);
        }
        let id = self.scene(managers.scene).ok_or(Error::UnknownObject)?;
        let (mut creation, content) = scene(self.get(id).ok_or(Error::UnknownObject)?)?;
        let fields = creation.body.initial.as_mut().ok_or(Error::Shape)?;
        let Some(sublevel::Initial::RpcReferences { rpc, value }) =
            fields.get_mut(managers.participants)
        else {
            return Err(Error::TypeMismatch);
        };
        let add = if value.contains(&participant) {
            None
        } else {
            if value.len() >= players::ai::MAX_AI {
                return Err(Error::Bound);
            }
            let call = crate::participants::Notification {
                endpoint: crate::participants::Endpoint {
                    scene: id,
                    selector: rpc.selector,
                    serial: rpc.serial,
                },
                participant,
                call: crate::participants::Call::Add,
            };
            call.encode()?;
            value.push(participant);
            Some(call)
        };
        let Some(sublevel::Initial::RpcPairs { rpc, value }) = fields.get_mut(managers.actors)
        else {
            return Err(Error::TypeMismatch);
        };
        let bind = if let Some((_, old)) = value.iter().find(|(p, _)| *p == participant) {
            if *old != actor {
                return Err(Error::DuplicateObject);
            }
            None
        } else {
            if value.len() >= players::ai::MAX_AI {
                return Err(Error::Bound);
            }
            let call = crate::actors::Binding {
                endpoint: crate::participants::Endpoint {
                    scene: id,
                    selector: rpc.selector,
                    serial: rpc.serial,
                },
                participant,
                actor,
            };
            call.encode()?;
            value.push((participant, actor));
            Some(call)
        };
        self.replace_ai_scene(id, creation, &content)?;
        Ok(Bound { add, bind })
    }

    /// Client manager listeners remove these entries when the actor/participant
    /// is deleted. Update retained creation state in the same deletion transaction;
    /// there is no separate remove RPC on this participant manager.
    pub(in crate::replication) fn release_ai_managers(
        &mut self,
        managers: Managers,
        participant: u16,
        actor: u16,
    ) -> Result<(), Error> {
        let id = self.scene(managers.scene).ok_or(Error::UnknownObject)?;
        let (mut creation, content) = scene(self.get(id).ok_or(Error::UnknownObject)?)?;
        let fields = creation.body.initial.as_mut().ok_or(Error::Shape)?;
        let Some(sublevel::Initial::RpcReferences { value, .. }) =
            fields.get_mut(managers.participants)
        else {
            return Err(Error::TypeMismatch);
        };
        value.retain(|p| *p != participant);
        let Some(sublevel::Initial::RpcPairs { value, .. }) = fields.get_mut(managers.actors)
        else {
            return Err(Error::TypeMismatch);
        };
        if value.iter().any(|(p, a)| *p == participant && *a != actor) {
            return Err(Error::UnknownObject);
        }
        value.retain(|(p, a)| (*p, *a) != (participant, actor));
        self.replace_ai_scene(id, creation, &content)
    }

    fn replace_ai_scene(
        &mut self,
        id: u16,
        creation: sublevel::Creation,
        content: &sublevel::Content,
    ) -> Result<(), Error> {
        let bindings = sublevel::state::Bindings {
            ghost: &|id| self.objects.contains_key(&id),
            level: &|id| self.levels.contains(&id),
        };
        let state = sublevel::state::State::new(creation, content, bindings)?;
        let current = Record::from_scene(id, state.snapshot(bindings)?, content)?;
        self.objects.insert(
            id,
            Object {
                initial: current.initial.ok_or(Error::Shape)?,
                current: current.update,
            },
        );
        Ok(())
    }
}
