// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ResultEntity's synchronous, all-participant snapshot. The bound collector
//! owns subsequent filtering/listeners; this operation does not persist rewards.
use super::*;
use data::bindings::results::{TRIGGER, Tag};
use nfs_world::scoring::events::Context;

pub const MAX_SCORES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub identity: Identity,
    pub collector: Link,
    pub triggering_participant: Participant,
    pub values: BTreeMap<Participant, BTreeMap<Tag, i32>>,
}

impl Host {
    pub fn result_event(
        &mut self,
        identity: &Identity,
        event: u32,
        context: Option<Context>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Effect>, Error> {
        let mut next = self.clone();
        let actions = next.observers.enqueue([Work::Results {
            identity: identity.clone(),
            event,
            context,
        }])?;
        let effects = next.execute(&actions, callback)?;
        *self = next;
        Ok(effects)
    }
}

impl Observers {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn result_work(
        &mut self,
        plan: &Plan,
        identity: &Identity,
        event: u32,
        context: Option<Context>,
        machine: &dyn nfs_world::participant_states::Membership,
        effects: &mut Vec<Effect>,
        callback: &mut Callback<'_>,
    ) -> Result<Vec<Action>, Error> {
        let definition = plan
            .data
            .bindings
            .results
            .iter()
            .find(|r| &r.identity == identity)
            .ok_or(Error::Invalid)?;
        let Some(Context::Participant(triggering_participant)) = context else {
            return Ok(vec![]);
        };
        if event != TRIGGER {
            return Ok(vec![]);
        }
        let mut values = BTreeMap::<Participant, BTreeMap<Tag, i32>>::new();
        let mut count = 0;
        for (tag, source) in &definition.scores {
            let Some(source) = source else {
                continue;
            };
            for ((key, participant), value) in &self.rankings.values {
                if key != source {
                    continue;
                }
                let row = values.entry(*participant).or_default();
                if row.insert(tag.clone(), *value).is_none() {
                    count += 1;
                }
                if count > MAX_SCORES {
                    return Err(Error::Bound);
                }
            }
        }
        let result = Snapshot {
            identity: definition.identity.clone(),
            collector: definition.collector.clone(),
            triggering_participant,
            values,
        };
        emit(machine, Effect::Results(result), effects, callback)
    }
}
