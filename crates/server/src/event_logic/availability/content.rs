// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::copies;
use crate::{prepared_content::Cache, progression::Catalog};
use nfs_content::{
    Error,
    assets::AssetIndex,
    events::discovery::{
        Availability, Endpoint, Entry, MAX_EVENTS,
        membership::{Fact, IntegerRange, Program},
    },
    progression::conditions::{Condition, Fact as Progress, owners::StateOwners},
    scoring::events::Identity,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    availability: Availability,
    conditions: BTreeMap<String, Condition>,
    auxiliary: StateOwners,
}
#[derive(Clone, Debug)]
pub struct Content {
    source: Source,
    facts: BTreeSet<Progress>,
    inputs: BTreeMap<Identity, Program>,
    ranges: BTreeMap<Identity, IntegerRange>,
    player_manager: Identity,
}
fn bad(message: &str) -> Error {
    Error::Content(format!("map availability: {message}"))
}
impl Content {
    pub fn prepare(
        index: &mut AssetIndex,
        progression: &Catalog,
        cache: Option<&Cache>,
    ) -> Result<Self, Error> {
        let known = progression.owned_condition_facts();
        let mut build = || -> Result<Source, Error> {
            let availability = index.event_availability()?;
            let ids = availability
                .memberships
                .iter()
                .map(|b| &b.program)
                .chain(availability.copies.values().map(|c| &c.input))
                .flat_map(Program::facts)
                .filter_map(|f| {
                    if let Fact::Boolean(id) = f {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect();
            let conditions = index.scene_mission_conditions(&ids)?;
            let unknown = conditions
                .values()
                .flat_map(Condition::facts)
                .filter(|f| !known.contains(f) && !matches!(f, Progress::MessageRead { .. }))
                .collect();
            let auxiliary = index.condition_state_owners(&unknown)?;
            Ok(Source {
                availability,
                conditions,
                auxiliary,
            })
        };
        match cache {
            Some(cache) => cache.prepare(
                "map-availability",
                &serde_json::to_vec(&known).map_err(|_| bad("input identity"))?,
                |bytes| {
                    Self::new(
                        serde_json::from_slice(bytes).map_err(|_| bad("cached definition"))?,
                        &known,
                    )
                },
                || serde_json::to_vec(&build()?).map_err(|_| bad("serialization")),
            ),
            None => Self::new(build()?, &known),
        }
    }
    fn new(source: Source, known: &BTreeSet<Progress>) -> Result<Self, Error> {
        let a = &source.availability;
        if a.entries.is_empty()
            || a.entries.len() > MAX_EVENTS
            || a.memberships.len() > MAX_EVENTS * 4
            || source.conditions.len() > 256
            || source.auxiliary.objectives.len() + source.auxiliary.integers.len() > 2048
        {
            return Err(bad("definition bound"));
        }
        let mut entries = BTreeSet::new();
        for entry in &a.entries {
            if entry.id.is_empty()
                || entry.id.len() > 512
                || entry.scene.is_empty()
                || entry.game_mode.is_empty()
                || entry.volume.scene_key == 0
                || entry.notifications.len() > 16
                || !entries.insert(entry.id.clone())
            {
                return Err(bad("entry identity"));
            }
        }
        if entries != a.visibility.keys().cloned().collect() {
            return Err(bad("variant visibility coverage"));
        }
        let mut notifications = BTreeSet::new();
        for b in &a.memberships {
            if b.notification.scene_key == 0 || !notifications.insert(b.notification) {
                return Err(bad("notification identity"));
            }
        }
        for rule in a.visibility.values() {
            rule.evaluate(|site| notifications.contains(&site).then_some(false))?;
        }
        if a.entries
            .iter()
            .any(|e| e.notifications.iter().any(|s| !notifications.contains(s)))
        {
            return Err(bad("entry notification missing"));
        }
        let controls = a
            .copies
            .iter()
            .map(|(id, c)| (id.clone(), copies::Definition::from(c)))
            .collect();
        copies::State::new(&controls, &a.channels)
            .map_err(|e| bad(&format!("copy controller {e:?}")))?;
        let mut inputs = BTreeMap::new();
        for control in a.copies.values() {
            if control.input.facts().any(|f| matches!(f, Fact::Copy(_)))
                || inputs
                    .insert(control.input_owner.clone(), control.input.clone())
                    .is_some_and(|old| old != control.input)
            {
                return Err(bad("copy input dependency or conflicting definition"));
            }
        }
        let mut booleans = BTreeSet::new();
        let mut player_managers = BTreeSet::new();
        let mut ranges = BTreeMap::new();
        for program in a
            .memberships
            .iter()
            .map(|b| &b.program)
            .chain(inputs.values())
        {
            for fact in program.facts() {
                match fact {
                    Fact::Boolean(id) => {
                        booleans.insert(id.clone());
                    }
                    Fact::Player(id) => {
                        player_managers.insert(id.clone());
                    }
                    Fact::Copy(id) if !a.copies.contains_key(id) => {
                        return Err(bad("copy owner missing"));
                    }
                    Fact::Mutable(id) if !program.empty_sets().contains(id) => {
                        return Err(bad("mutable owner missing"));
                    }
                    _ => {}
                }
            }
            for (id, range) in program.ranges() {
                if !a.integers.contains_key(&range.input)
                    || ranges
                        .insert(id.clone(), range.clone())
                        .is_some_and(|old| old != *range)
                {
                    return Err(bad("range owner missing or inconsistent"));
                }
            }
        }
        if booleans != source.conditions.keys().cloned().collect() {
            return Err(bad("Boolean owner coverage"));
        }
        let [player_manager] = Vec::from_iter(player_managers)
            .try_into()
            .map_err(|_| bad("requires one exact world player manager"))?;
        if a.integers
            .values()
            .any(|s| s.player_manager != player_manager)
        {
            return Err(bad("integer store belongs to another player manager"));
        }
        let facts: BTreeSet<_> = source
            .conditions
            .values()
            .flat_map(Condition::facts)
            .collect();
        let auxiliary: BTreeSet<_> = source
            .auxiliary
            .objectives
            .keys()
            .flat_map(|id| [Progress::Active(*id), Progress::Completed(*id)])
            .chain(
                source
                    .auxiliary
                    .integers
                    .iter()
                    .copied()
                    .map(Progress::PositiveInt),
            )
            .collect();
        if facts.len() > 4096
            || !auxiliary.is_disjoint(known)
            || auxiliary.iter().any(|f| {
                matches!(
                    f,
                    Progress::Active(0) | Progress::Completed(0) | Progress::PositiveInt(0)
                )
            })
            || facts.iter().any(|f| {
                !known.contains(f)
                    && !auxiliary.contains(f)
                    && !matches!(f, Progress::MessageRead { .. })
            })
        {
            return Err(bad("progression owner coverage"));
        }
        Ok(Self {
            source,
            facts,
            inputs,
            ranges,
            player_manager,
        })
    }
    pub fn entries(&self) -> &[Entry] {
        &self.source.availability.entries
    }
    pub fn facts(&self) -> &BTreeSet<Progress> {
        &self.facts
    }
    pub fn player_manager(&self) -> &Identity {
        &self.player_manager
    }
    pub fn auxiliary(&self) -> &StateOwners {
        &self.source.auxiliary
    }
    pub fn conditions(&self) -> &BTreeMap<String, Condition> {
        &self.source.conditions
    }
    pub fn integer(
        &self,
        id: &Identity,
    ) -> Option<(
        &IntegerRange,
        &nfs_content::events::discovery::integers::Store,
    )> {
        let range = self.ranges.get(id)?;
        Some((range, self.source.availability.integers.get(&range.input)?))
    }
    pub(super) fn definitions(&self) -> &Availability {
        &self.source.availability
    }
    pub(super) fn inputs(&self) -> &BTreeMap<Identity, Program> {
        &self.inputs
    }
    pub fn notifications(&self) -> BTreeSet<Endpoint> {
        self.source
            .availability
            .memberships
            .iter()
            .map(|b| b.notification)
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests;
