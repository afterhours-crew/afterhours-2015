// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-owned playback of an installed video asset. Native completion
//! carries no movie ID; the current state and connection owner gate acceptance.
use crate::{
    bits::BitWriter,
    participants::{Call, Endpoint, HostRpc, Notification},
    replication::Error,
    sequences::Owner,
};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Video {
    pub bundle: u16,
    pub index: u16,
    pub count: u16,
}
impl Video {
    pub fn validate(self) -> Result<(), Error> {
        if self.bundle == 0
            || self.bundle >= 2048
            || self.bundle == 2003
            || self.count == 0
            || self.index >= self.count
        {
            return Err(Error::Bound);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Play {
    pub endpoint: Endpoint,
    pub participant: u16,
    pub video: Video,
}
impl Play {
    pub fn encode(self) -> Result<BitWriter, Error> {
        self.video.validate()?;
        Notification {
            endpoint: self.endpoint,
            participant: self.participant,
            call: Call::Enter,
        }
        .encode()?;
        let mut payload = BitWriter::new();
        payload
            .put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(2, 32)
            .put(2, 2)
            .put(self.video.bundle.into(), 11)
            .put(
                self.video.index.into(),
                (16 - self.video.count.leading_zeros()) as usize,
            )
            .align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(2, 8)
            .put(self.endpoint.scene.into(), 13)
            .put(self.participant.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bindings {
    pub before: Endpoint,
    pub playing: Endpoint,
    pub after: Endpoint,
    pub video: Video,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Before,
    Playing,
    After,
    Finished,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Active {
    owner: Owner,
    bindings: Bindings,
    phase: Phase,
}
impl Active {
    pub fn start(owner: Owner, bindings: Bindings) -> Result<(Self, Vec<HostRpc>), Error> {
        bindings.video.validate()?;
        if [bindings.before, bindings.after].contains(&bindings.playing)
            || bindings.before == bindings.after
        {
            return Err(Error::Shape);
        }
        let active = Self {
            owner,
            bindings,
            phase: Phase::Before,
        };
        let calls = vec![active.call(bindings.before, Call::Enter)];
        for endpoint in [bindings.before, bindings.playing, bindings.after] {
            active.call(endpoint, Call::Enter).encode()?;
        }
        Ok((active, calls))
    }
    pub fn owner(&self) -> Owner {
        self.owner
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    fn call(&self, endpoint: Endpoint, call: Call) -> HostRpc {
        HostRpc::Participant(Notification {
            endpoint,
            participant: self.owner.participant,
            call,
        })
    }
    /// The caller validates completion of this participant's before sequence.
    pub fn before_completed(&mut self, owner: Owner) -> Result<Vec<HostRpc>, Error> {
        if owner != self.owner {
            return Err(Error::UnknownObject);
        }
        if self.phase != Phase::Before {
            return Ok(vec![]);
        }
        let calls = vec![
            self.call(self.bindings.before, Call::Leave),
            self.call(self.bindings.playing, Call::Enter),
            HostRpc::Movie(Play {
                endpoint: self.bindings.playing,
                participant: owner.participant,
                video: self.bindings.video,
            }),
        ];
        self.phase = Phase::Playing;
        Ok(calls)
    }
    pub fn receive(
        &mut self,
        body: BitSpan<'_>,
        owner: Owner,
    ) -> Result<Option<Vec<HostRpc>>, Error> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 2,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if envelope.references().first() != Some(&self.bindings.playing.scene)
            || route.selector() != self.bindings.playing.selector
            || route.method_index() != 3
        {
            return Ok(None);
        }
        if owner != self.owner
            || envelope.references() != [self.bindings.playing.scene, owner.participant]
        {
            return Err(Error::UnknownObject);
        }
        // These seven alignment bits are unspecified and are nonzero in the
        // native client. Validate their length, not their incidental values.
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || route.arguments().len() != 7
        {
            return Err(Error::Shape);
        }
        match self.phase {
            Phase::Before => Err(Error::Shape),
            Phase::After | Phase::Finished => Ok(Some(vec![])),
            Phase::Playing => {
                self.phase = Phase::After;
                Ok(Some(vec![
                    self.call(self.bindings.playing, Call::Leave),
                    self.call(self.bindings.after, Call::Enter),
                ]))
            }
        }
    }
    /// The caller validates completion of this participant's after sequence.
    pub fn after_completed(&mut self, owner: Owner) -> Result<Vec<HostRpc>, Error> {
        if owner != self.owner {
            return Err(Error::UnknownObject);
        }
        match self.phase {
            Phase::Finished => Ok(vec![]),
            Phase::After => {
                self.phase = Phase::Finished;
                Ok(vec![self.call(self.bindings.after, Call::Leave)])
            }
            _ => Err(Error::Shape),
        }
    }
}

#[cfg(test)]
mod tests;
