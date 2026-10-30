// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned native movie handshake with authored sequences on both sides.
use crate::sequence_content::authored::{Authored, Instances};
use nfs_content::{
    assets::AssetIndex,
    prologue::{movies, sequences},
};
use nfs_protocol::world::BitSpan;
use nfs_world::{
    fmv,
    participants::{Endpoint, HostRpc},
    replication::{Error, Record, entity::creation::Asset, players::Players},
    sequences::{Completed, Owner},
};

mod prepared;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "prepared::Source", into = "prepared::Source")]
pub struct Content {
    pub data: movies::Movie,
    before: Authored,
    after: Authored,
    before_seconds: f64,
    after_seconds: f64,
}
impl Content {
    pub(crate) fn timed_outro(&self) -> bool {
        self.after_seconds > 0.
    }
    #[cfg(test)]
    pub(crate) fn for_test(data: movies::Movie, sequence: Authored, after_seconds: f64) -> Self {
        Self {
            data,
            before: sequence.clone(),
            after: sequence,
            before_seconds: 0.,
            after_seconds,
        }
    }
    pub fn prepare(
        index: &mut AssetIndex,
        parent: Asset,
        capability: &str,
    ) -> Result<Self, nfs_content::Error> {
        let data = movies::extract(index, capability)?;
        let mut prepare =
            |sequence: &movies::Sequence| -> Result<(Authored, f64), nfs_content::Error> {
                let seconds = sequences::timing::extract(index, &sequence.blueprint)?
                    .filter(|s| s.is_finite() && *s >= 0. && *s <= 300.)
                    .ok_or_else(|| nfs_content::Error::Content("movie sequence timer".into()))?;
                let authored = Authored::new(
                    sequences::extract_referenced(index, &sequence.blueprint, &sequence.reference)?,
                    parent,
                    vec![],
                )
                .map_err(|e| nfs_content::Error::Content(format!("movie sequence: {e:?}")))?;
                Ok((authored, seconds))
            };
        let (before, before_seconds) = prepare(&data.before)?;
        let (after, after_seconds) = prepare(&data.after)?;
        Ok(Self {
            data,
            before,
            after,
            before_seconds,
            after_seconds,
        })
    }
}
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub state_operations: Vec<nfs_world::participants::membership::Operation>,
    pub records: Vec<Record>,
    pub calls: Vec<HostRpc>,
    pub deleted: Vec<u16>,
    pub finished: bool,
}
#[derive(Clone, Debug)]
pub struct Active {
    definition: movies::Movie,
    flow: fmv::Active,
    sequences: Instances,
    seconds: f64,
    deadline: Option<f64>,
}
impl Active {
    pub fn state_bindings(&self) -> fmv::Bindings {
        self.flow.bindings()
    }
    pub fn owner(&self) -> Owner {
        self.flow.owner()
    }
    pub fn objective(&self) -> u64 {
        self.definition.objective
    }
    /// Inject a finite monotonic session clock before input or timer advancement.
    pub fn clock(&mut self, seconds: f64) -> Result<(), Error> {
        if !seconds.is_finite() || seconds < self.seconds {
            return Err(Error::Shape);
        }
        self.seconds = seconds;
        Ok(())
    }
    pub fn advance(
        &mut self,
        content: &Content,
        players: &Players,
        seconds: f64,
    ) -> Result<Output, Error> {
        if content.data != self.definition {
            return Err(Error::Shape);
        }
        let mut staged = self.clone();
        staged.clock(seconds)?;
        let mut out = Output::default();
        if staged.deadline.is_some_and(|at| seconds >= at) {
            let sequence = match staged.phase() {
                fmv::Phase::Before => &content.before,
                fmv::Phase::After => &content.after,
                _ => return Err(Error::Shape),
            };
            let owner = staged.owner();
            let stop = staged
                .sequences
                .stop(sequence, players, owner)?
                .ok_or(Error::Shape)?;
            out.calls.push(HostRpc::SequenceStop(stop));
            staged.deadline = None;
        }
        *self = staged;
        Ok(out)
    }
    pub fn phase(&self) -> fmv::Phase {
        self.flow.phase()
    }
    pub fn sequence(&self) -> Option<u16> {
        self.sequences.ghost(self.owner().participant)
    }
    pub fn start(
        content: &Content,
        players: &mut Players,
        owner: Owner,
    ) -> Result<(Self, Output), Error> {
        Self::start_at(content, players, owner, 0.)
    }
    pub fn start_at(
        content: &Content,
        players: &mut Players,
        owner: Owner,
        seconds: f64,
    ) -> Result<(Self, Output), Error> {
        if !seconds.is_finite() || seconds < 0. {
            return Err(Error::Shape);
        }
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        let endpoint = |(key, serializer)| -> Result<Endpoint, Error> {
            let (scene, selector) = players
                .objects()
                .scene_endpoint(key, serializer)
                .ok_or(Error::UnknownObject)?;
            let call = crate::event_logic::call(
                players.objects(),
                scene,
                selector,
                0,
                Some(owner.participant),
                nfs_world::game_mode::Arguments::None,
            )?;
            Ok(Endpoint {
                scene,
                selector,
                serial: call.serial,
            })
        };
        let (flow, calls) = fmv::Active::start(
            owner,
            fmv::Bindings {
                before: endpoint(content.data.before.state)?,
                playing: endpoint(content.data.state)?,
                after: endpoint(content.data.after.state)?,
                video: content.data.video,
            },
        )?;
        let mut staged = players.clone();
        let mut sequences = Instances::default();
        let (record, deadline) = start_sequence(
            &mut sequences,
            &content.before,
            &mut staged,
            owner,
            seconds,
            content.before_seconds,
        )?;
        *players = staged;
        Ok((
            Self {
                definition: content.data.clone(),
                flow,
                sequences,
                seconds,
                deadline,
            },
            Output {
                records: vec![record],
                calls,
                ..Output::default()
            },
        ))
    }
    /// Scene movie completion creates the after-sequence only once. All state
    /// and object allocation roll back together if binding/construction fails.
    pub fn receive(
        &mut self,
        content: &Content,
        players: &mut Players,
        owner: Owner,
        body: BitSpan<'_>,
    ) -> Result<Option<Output>, Error> {
        self.receive_with(content, players, owner, body, |_| Ok(Vec::new()))
    }
    /// Recreate participant resources after a validated movie acknowledgement,
    /// before constructing its outro. Callback and construction share rollback.
    pub fn receive_with(
        &mut self,
        content: &Content,
        players: &mut Players,
        owner: Owner,
        body: BitSpan<'_>,
        restore: impl FnOnce(&mut Players) -> Result<Vec<Record>, Error>,
    ) -> Result<Option<Output>, Error> {
        if content.data != self.definition {
            return Err(Error::Shape);
        }
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        let mut active = self.clone();
        let before = active.flow.phase();
        let Some(calls) = active.flow.receive(body, owner)? else {
            return Ok(None);
        };
        let mut out = Output {
            calls,
            state_operations: active.flow.take_state_operations()?,
            ..Output::default()
        };
        let mut staged = players.clone();
        if before == fmv::Phase::Playing && active.flow.phase() == fmv::Phase::After {
            out.records.extend(restore(&mut staged)?);
            active.sequences.retire_completed(&staged, owner)?;
            let (record, deadline) = start_sequence(
                &mut active.sequences,
                &content.after,
                &mut staged,
                owner,
                active.seconds,
                content.after_seconds,
            )?;
            active.deadline = deadline;
            out.records.push(record);
        }
        *players = staged;
        *self = active;
        Ok(Some(out))
    }
    pub fn completed(
        &mut self,
        players: &mut Players,
        owner: Owner,
        call: Completed,
    ) -> Result<Output, Error> {
        let mut active = self.clone();
        let mut staged = players.clone();
        let mut out = Output::default();
        if let Some(deleted) = active.sequences.complete(&mut staged, owner, call)? {
            out.deleted.push(deleted);
            out.calls = match active.flow.phase() {
                fmv::Phase::Before => active.flow.before_completed(owner)?,
                fmv::Phase::After => {
                    out.finished = true;
                    active.flow.after_completed(owner)?
                }
                _ => return Err(Error::Shape),
            };
        }
        out.state_operations = active.flow.take_state_operations()?;
        *players = staged;
        *self = active;
        Ok(out)
    }
}

fn start_sequence(
    instances: &mut Instances,
    content: &Authored,
    players: &mut Players,
    owner: Owner,
    seconds: f64,
    duration: f64,
) -> Result<(Record, Option<f64>), Error> {
    if !duration.is_finite()
        || !(0. ..=300.).contains(&duration)
        || !(seconds + duration).is_finite()
    {
        return Err(Error::Shape);
    }
    let record = if duration == 0. {
        instances.start_stopped(content, players, owner)?
    } else {
        instances.start(content, players, owner)?
    }
    .ok_or(Error::Shape)?;
    Ok((record, (duration > 0.).then_some(seconds + duration)))
}

#[cfg(test)]
mod tests;
