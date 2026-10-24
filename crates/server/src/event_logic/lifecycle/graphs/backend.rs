// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! The graph executor is shared by isolated authored-plan tests and the session
//! WorldHost. Both backends yield at the same synchronous callback boundaries.
use super::super::{self as lifecycle, Host, commands::Command, execution, values::Values, world};
use nfs_world::participant_states::{Error, Machine, Membership, Participant, StateId};
use std::{collections::BTreeSet, fmt::Debug};

pub enum Yield<E> {
    Effect(E),
    Returned(u64),
}

pub enum View<'a> {
    Single(&'a Machine),
    World(world::View<'a>),
}
impl Membership for View<'_> {
    fn members(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error> {
        match self {
            Self::Single(m) => m.members(state),
            Self::World(m) => m.members(state),
        }
    }
    fn completed(&self, state: StateId) -> Result<&BTreeSet<Participant>, Error> {
        match self {
            Self::Single(m) => m.completed(state),
            Self::World(m) => m.completed(state),
        }
    }
}

pub trait Backend: Clone + Debug + Sized {
    type Command: Clone + Debug;
    type Effect: Clone + Debug + PartialEq;
    type Execution: Running<Self>;
    fn begin(&self, commands: Vec<Self::Command>) -> Result<Self::Execution, Error>;
    fn values(&self, scope: u64) -> Result<Values<'_>, Error>;
}
pub trait Running<H: Backend> {
    fn advance(&mut self) -> Result<Option<Yield<H::Effect>>, Error>;
    fn resume(&mut self, commands: Vec<H::Command>) -> Result<(), Error>;
    fn call(&mut self, commands: Vec<H::Command>) -> Result<u64, Error>;
    fn finish(self) -> Result<H, Error>;
    fn values(&self, scope: u64) -> Result<Values<'_>, Error>;
    fn view(&self, effect: &H::Effect) -> Result<View<'_>, Error>;
    fn effect_scope(effect: &H::Effect) -> u64;
}

impl Backend for Host {
    type Command = Command;
    type Effect = lifecycle::Effect;
    type Execution = execution::Execution;
    fn begin(&self, commands: Vec<Command>) -> Result<Self::Execution, Error> {
        Host::begin(self, commands)
    }
    fn values(&self, _: u64) -> Result<Values<'_>, Error> {
        Ok(Host::values(self))
    }
}
impl Running<Host> for execution::Execution {
    fn advance(&mut self) -> Result<Option<Yield<lifecycle::Effect>>, Error> {
        Ok(self.advance()?.map(|y| match y {
            execution::Yield::Effect(e) => Yield::Effect(e),
            execution::Yield::Returned(t) => Yield::Returned(t),
        }))
    }
    fn resume(&mut self, c: Vec<Command>) -> Result<(), Error> {
        self.resume(c)
    }
    fn call(&mut self, c: Vec<Command>) -> Result<u64, Error> {
        self.call(c)
    }
    fn finish(self) -> Result<Host, Error> {
        Ok(self.finish()?.0)
    }
    fn values(&self, _: u64) -> Result<Values<'_>, Error> {
        Ok(self.values())
    }
    fn view(&self, _: &lifecycle::Effect) -> Result<View<'_>, Error> {
        Ok(View::Single(self.machine()))
    }
    fn effect_scope(_: &lifecycle::Effect) -> u64 {
        0
    }
}

impl Backend for world::WorldHost {
    type Command = world::Scoped<Command>;
    type Effect = world::Publication;
    type Execution = world::Execution;
    fn begin(&self, c: Vec<Self::Command>) -> Result<Self::Execution, Error> {
        self.begin(c)
    }
    fn values(&self, scope: u64) -> Result<Values<'_>, Error> {
        self.values(scope)
    }
}
impl Running<world::WorldHost> for world::Execution {
    fn advance(&mut self) -> Result<Option<Yield<world::Publication>>, Error> {
        Ok(self.advance()?.map(|y| match y {
            world::Yield::Effect(e) => Yield::Effect(*e),
            world::Yield::Returned(t) => Yield::Returned(t),
        }))
    }
    fn resume(&mut self, c: Vec<world::Scoped<Command>>) -> Result<(), Error> {
        self.resume(c)
    }
    fn call(&mut self, c: Vec<world::Scoped<Command>>) -> Result<u64, Error> {
        self.call(c)
    }
    fn finish(self) -> Result<world::WorldHost, Error> {
        Ok(self.finish()?.0)
    }
    fn values(&self, scope: u64) -> Result<Values<'_>, Error> {
        self.values(scope)
    }
    fn view(&self, effect: &world::Publication) -> Result<View<'_>, Error> {
        Ok(View::World(self.view(effect.instance)?))
    }
    fn effect_scope(effect: &world::Publication) -> u64 {
        effect.instance.value()
    }
}
