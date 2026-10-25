// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Native garage composite ownership. Sequence completion and the later story
//! acknowledgement are distinct gates; neither one directly mutates storage.
use crate::sequence_content::authored::{Authored, Instances};
use nfs_content::{
    assets::AssetIndex,
    prologue::{composite, sequences},
};
use nfs_world::{
    logic::Message,
    replication::{Error, Initial, Record, entity, entity::creation::Asset, players::Players},
    sequences::{Completed, Owner, Stop},
};

#[derive(Clone, Debug)]
pub struct Content {
    pub data: composite::Composite,
    sequence: Authored,
    parent: Asset,
    fallback_seconds: f64,
}
impl Content {
    pub fn prepare(
        index: &mut AssetIndex,
        parent: Asset,
        capability: &str,
    ) -> Result<Self, nfs_content::Error> {
        let data = composite::extract(index, capability)?;
        let signals = sequences::direct_stop_signals(index, &data.sequence.blueprint)?;
        if signals.is_empty() {
            return Err(nfs_content::Error::Content(
                "composite stop signal missing".into(),
            ));
        }
        let fallback_seconds = sequences::timing::extract(index, &data.sequence.blueprint)?
            .filter(|s| s.is_finite() && *s > 0. && *s <= 600.)
            .ok_or_else(|| nfs_content::Error::Content("composite fallback timer".into()))?;
        let sequence = Authored::new(
            sequences::extract_referenced(
                index,
                &data.sequence.blueprint,
                &data.sequence.reference,
            )?,
            parent,
            signals,
        )
        .map_err(|e| nfs_content::Error::Content(format!("composite sequence: {e:?}")))?;
        Ok(Self {
            data,
            sequence,
            parent,
            fallback_seconds,
        })
    }
}
#[derive(Clone, Debug)]
pub struct Active {
    definition: composite::Composite,
    owner: Owner,
    parent: u16,
    parent_asset: Asset,
    sequence: Instances,
    seconds: f64,
    deadline: Option<f64>,
    finished: bool,
    queued: bool,
}
impl Active {
    pub fn owner(&self) -> Owner {
        self.owner
    }
    pub fn ghost(&self) -> u16 {
        self.sequence
            .ghost(self.owner.participant)
            .expect("started composite")
    }
    pub fn finished(&self) -> bool {
        self.finished
    }
    pub fn start(
        content: &Content,
        players: &mut Players,
        owner: Owner,
        seconds: f64,
    ) -> Result<(Self, Record), Error> {
        if !seconds.is_finite() || seconds < 0. || !(seconds + content.fallback_seconds).is_finite()
        {
            return Err(Error::Shape);
        }
        let parents = players.objects().iter().filter_map(|(id, o)| match &o.initial {
            Initial::Entity { prefix, fields } if prefix.asset == content.parent && matches!(fields.first(), Some(entity::Initial::Root { reference, .. }) if *reference == owner.participant) => Some(id),
            _ => None,
        }).collect::<Vec<_>>();
        let [parent] = parents.as_slice() else {
            return Err(Error::UnknownObject);
        };
        let mut sequence = Instances::default();
        let record = sequence
            .start(&content.sequence, players, owner)?
            .ok_or(Error::Shape)?;
        Ok((
            Self {
                definition: content.data.clone(),
                owner,
                parent: *parent,
                parent_asset: content.parent,
                sequence,
                seconds,
                deadline: Some(seconds + content.fallback_seconds),
                finished: false,
                queued: false,
            },
            record,
        ))
    }
    fn validate(&self, content: &Content) -> Result<(), Error> {
        if self.definition != content.data {
            return Err(Error::Shape);
        }
        Ok(())
    }
    pub fn signal(
        &mut self,
        content: &Content,
        players: &Players,
        message: &Message,
    ) -> Result<Option<Stop>, Error> {
        self.validate(content)?;
        let stop = self
            .sequence
            .signal(&content.sequence, players, self.owner, message)?;
        if stop.is_some() {
            self.deadline = None;
        }
        Ok(stop)
    }
    pub fn advance(
        &mut self,
        content: &Content,
        players: &Players,
        seconds: f64,
    ) -> Result<Option<Stop>, Error> {
        self.validate(content)?;
        if !seconds.is_finite() || seconds < self.seconds {
            return Err(Error::Shape);
        }
        let stop = if self.deadline.is_some_and(|d| seconds >= d) {
            let stop = self.sequence.stop(&content.sequence, players, self.owner)?;
            self.deadline = None;
            stop
        } else {
            None
        };
        self.seconds = seconds;
        Ok(stop)
    }
    pub fn completed(
        &mut self,
        players: &mut Players,
        call: Completed,
    ) -> Result<Option<u16>, Error> {
        let deleted = self.sequence.complete(players, self.owner, call)?;
        if deleted.is_some() {
            self.finished = true;
            self.deadline = None;
        }
        Ok(deleted)
    }
    /// Claim only the exact owned capability signal after native sequence ACK.
    /// Stage this owner with the durable request and its bounded output queue.
    pub fn completion(
        &mut self,
        players: &Players,
        message: &Message,
    ) -> Result<Option<u64>, Error> {
        let Message::Reached {
            target,
            event,
            player,
        } = message
        else {
            return Ok(None);
        };
        let signal = self.definition.completion;
        if target.ghost != self.parent
            || target.entity != signal.entity
            || *event != signal.event
            || players.local_slot(
                self.owner.connection,
                self.owner.persona,
                self.owner.participant,
            ) != Some(*player)
        {
            return Ok(None);
        }
        if !players.owns_participant(
            self.owner.connection,
            self.owner.persona,
            self.owner.participant,
        ) {
            return Err(Error::UnknownObject);
        }
        if !matches!(players.objects().get(self.parent).map(|o| &o.initial),
            Some(Initial::Entity { prefix, fields }) if prefix.asset == self.parent_asset
                && matches!(fields.first(), Some(entity::Initial::Root { reference, .. }) if *reference == self.owner.participant))
        {
            return Err(Error::UnknownObject);
        }
        if !self.finished || self.queued {
            return Ok(None);
        }
        self.queued = true;
        Ok(Some(self.definition.objective))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world::{logic::EntityRef, scoring::event_sync::Binding};
    fn fixture() -> (Players, Content, Owner, Owner) {
        let (players, sequence, a, b) = crate::sequence_content::authored::tests::fixture();
        let data = composite::Composite {
            objective: 17,
            sequence: nfs_content::prologue::movies::Sequence {
                blueprint: "test".into(),
                reference: nfs_content::scoring::events::Identity {
                    asset: "test".into(),
                    index: 1,
                    instances: vec![],
                },
                state: (7, 2),
            },
            completion: Binding {
                entity: 9,
                event: 71,
                node: 0,
            },
        };
        (
            players,
            Content {
                data,
                sequence,
                parent: Asset {
                    bundle: 4,
                    type_id: 8,
                    local_index: 0,
                },
                fallback_seconds: 300.,
            },
            a,
            b,
        )
    }
    fn signal(ghost: u16, entity: u32, event: u32, player: u8) -> Message {
        Message::Reached {
            event,
            target: EntityRef { ghost, entity },
            player,
        }
    }
    #[test]
    fn native_stop_ack_then_owned_story_signal_are_distinct_retry_safe_gates() {
        let (mut players, content, owner, other) = fixture();
        let (mut active, record) = Active::start(&content, &mut players, owner, 10.).unwrap();
        let completion = signal(active.parent, 9, 71, 0);
        assert_eq!(active.completion(&players, &completion).unwrap(), None);
        let ack = Completed {
            sequence: record.id,
            participant: owner.participant,
        };
        assert!(active.completed(&mut players, ack).is_err());
        assert!(
            active
                .signal(&content, &players, &signal(record.id, 1, 37, 1))
                .unwrap()
                .is_none()
        );
        assert!(
            active
                .signal(&content, &players, &signal(record.id, 1, 37, 0))
                .unwrap()
                .is_some()
        );
        assert!(
            active
                .signal(&content, &players, &signal(record.id, 1, 37, 0))
                .unwrap()
                .is_none()
        );
        assert!(
            active
                .completed(
                    &mut players,
                    Completed {
                        participant: other.participant,
                        ..ack
                    }
                )
                .is_err()
        );
        assert_eq!(
            active.completed(&mut players, ack).unwrap(),
            Some(record.id)
        );
        assert_eq!(active.completed(&mut players, ack).unwrap(), None);
        assert!(active.finished());
        assert!(active.advance(&content, &players, 400.).unwrap().is_none());
        assert_eq!(
            active
                .completion(&players, &signal(active.parent, 9, 71, 1))
                .unwrap(),
            None
        );
        assert_eq!(
            active
                .completion(&players, &signal(active.parent, 9, 72, 0))
                .unwrap(),
            None
        );
        assert_eq!(active.completion(&players, &completion).unwrap(), Some(17));
        assert_eq!(active.completion(&players, &completion).unwrap(), None);
    }
    #[test]
    fn fallback_only_stops_and_still_requires_native_ack_and_story_signal() {
        let (mut players, content, owner, _) = fixture();
        let (mut active, _) = Active::start(&content, &mut players, owner, 10.).unwrap();
        assert!(
            active
                .advance(&content, &players, 309.999)
                .unwrap()
                .is_none()
        );
        assert!(active.advance(&content, &players, 309.).is_err());
        assert!(active.advance(&content, &players, f64::NAN).is_err());
        assert!(active.advance(&content, &players, 310.).unwrap().is_some());
        assert!(!active.finished());
        assert!(active.advance(&content, &players, 311.).unwrap().is_none());
        assert_eq!(
            active
                .completion(&players, &signal(active.parent, 9, 71, 0))
                .unwrap(),
            None
        );
    }
}
