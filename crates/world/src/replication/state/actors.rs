// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

impl World {
    pub(in crate::replication) fn bind_actor(
        &mut self,
        gameplay_key: u32,
        participant: u16,
        actor: u16,
    ) -> Result<Option<crate::actors::Binding>, Error> {
        if !matches!(
            self.get(participant).map(|o| &o.initial),
            Some(Initial::Participant(_))
        ) || !matches!(self.get(actor).map(|o| &o.initial), Some(Initial::Actor(a)) if a.participant == participant)
        {
            return Err(Error::UnknownObject);
        }
        let id = self.scene(gameplay_key).ok_or(Error::UnknownObject)?;
        let (mut creation, content) = scene(self.get(id).ok_or(Error::UnknownObject)?)?;
        let Some(sublevel::Initial::RpcPairs { rpc, value }) =
            creation.body.initial.as_mut().and_then(|f| f.get_mut(24))
        else {
            return Err(Error::TypeMismatch);
        };
        if let Some((_, old)) = value.iter().find(|(p, _)| *p == participant) {
            return if *old == actor {
                Ok(None)
            } else {
                Err(Error::Unsupported)
            };
        }
        if value.len() >= 128 {
            return Err(Error::Bound);
        }
        let reply = crate::actors::Binding {
            endpoint: crate::participants::Endpoint {
                scene: id,
                selector: rpc.selector,
                serial: rpc.serial,
            },
            participant,
            actor,
        };
        reply.encode()?;
        value.push((participant, actor));
        let bindings = sublevel::state::Bindings {
            ghost: &|id| self.objects.contains_key(&id),
            level: &|id| self.levels.contains(&id),
        };
        let state = sublevel::state::State::new(creation, &content, bindings)?;
        let current = Record::from_scene(id, state.snapshot(bindings)?, &content)?;
        self.objects.insert(
            id,
            Object {
                initial: current.initial.ok_or(Error::Shape)?,
                current: current.update,
            },
        );
        Ok(Some(reply))
    }
}
