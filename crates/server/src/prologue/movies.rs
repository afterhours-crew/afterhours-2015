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

#[derive(Clone, Debug)]
pub struct Content {
    pub data: movies::Movie,
    before: Authored,
    after: Authored,
}
impl Content {
    #[cfg(test)]
    pub(crate) fn for_test(data: movies::Movie, sequence: Authored) -> Self {
        Self {
            data,
            before: sequence.clone(),
            after: sequence,
        }
    }
    pub fn prepare(
        index: &mut AssetIndex,
        parent: Asset,
        capability: &str,
    ) -> Result<Self, nfs_content::Error> {
        let data = movies::extract(index, capability)?;
        let mut prepare = |sequence: &movies::Sequence| -> Result<Authored, nfs_content::Error> {
            if sequences::timing::extract(index, &sequence.blueprint)? != Some(0.) {
                return Err(nfs_content::Error::Content(
                    "movie sequence requires an owned timer".into(),
                ));
            }
            Authored::new(
                sequences::extract_referenced(index, &sequence.blueprint, &sequence.reference)?,
                parent,
                vec![],
            )
            .map_err(|e| nfs_content::Error::Content(format!("movie sequence: {e:?}")))
        };
        let before = prepare(&data.before)?;
        let after = prepare(&data.after)?;
        Ok(Self {
            data,
            before,
            after,
        })
    }
}
#[derive(Clone, Debug, Default)]
pub struct Output {
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
}
impl Active {
    pub fn owner(&self) -> Owner {
        self.flow.owner()
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
        let record = sequences
            .start_stopped(&content.before, &mut staged, owner)?
            .ok_or(Error::Shape)?;
        *players = staged;
        Ok((
            Self {
                definition: content.data.clone(),
                flow,
                sequences,
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
            ..Output::default()
        };
        let mut staged = players.clone();
        if before == fmv::Phase::Playing && active.flow.phase() == fmv::Phase::After {
            out.records.extend(restore(&mut staged)?);
            active.sequences.retire_completed(&staged, owner)?;
            out.records.push(
                active
                    .sequences
                    .start_stopped(&content.after, &mut staged, owner)?
                    .ok_or(Error::Shape)?,
            );
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
        *players = staged;
        *self = active;
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
