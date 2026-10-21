// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use crate::{
    participants::{Call, Endpoint, Notification},
    replication::{Error, Initial, players::Players, sublevel},
};

/// The reset branch of the installed startup participant state machine.
/// Endpoints retain the allocated scene identity and serializer lifetime.
#[derive(Clone, Copy, Debug)]
pub struct ResetBindings {
    pub cinematic: Endpoint,
    pub recover: Endpoint,
    pub teleport: Endpoint,
    pub rendering: Endpoint,
    pub arrive: Endpoint,
    pub alive: Endpoint,
}
impl ResetBindings {
    pub fn from_scene(players: &Players, scene: u16) -> Result<Self, Error> {
        let object = players.objects().get(scene).ok_or(Error::UnknownObject)?;
        let Initial::SubLevel { fields, .. } = &object.initial else {
            return Err(Error::TypeMismatch);
        };
        let endpoint = |index: usize| {
            let rpc = fields
                .get(index)
                .and_then(sublevel::Initial::rpc)
                .ok_or(Error::TypeMismatch)?;
            Ok(Endpoint {
                scene,
                selector: rpc.selector,
                serial: rpc.serial,
            })
        };
        Ok(Self {
            cinematic: endpoint(106)?,
            recover: endpoint(103)?,
            teleport: endpoint(109)?,
            rendering: endpoint(110)?,
            arrive: endpoint(112)?,
            alive: endpoint(2)?,
        })
    }

    pub fn begin(self, participant: u16) -> Vec<Notification> {
        [
            (self.cinematic, Call::Leave),
            (self.recover, Call::Enter),
            (self.recover, Call::Leave),
            (self.teleport, Call::Enter),
        ]
        .map(|(endpoint, call)| Notification {
            endpoint,
            participant,
            call,
        })
        .to_vec()
    }

    pub fn finish(self, participant: u16) -> Vec<Notification> {
        [
            (self.teleport, Call::Leave),
            (self.rendering, Call::Enter),
            (self.rendering, Call::Leave),
            (self.arrive, Call::Enter),
            (self.arrive, Call::Leave),
            (self.alive, Call::Enter),
        ]
        .map(|(endpoint, call)| Notification {
            endpoint,
            participant,
            call,
        })
        .to_vec()
    }
}
