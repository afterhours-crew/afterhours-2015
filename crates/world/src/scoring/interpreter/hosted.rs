// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Execute the same interpreter with synchronous calls to the enclosing event
//! host. The caller commits its host and all affected graph instances together.
use super::*;
use events::native::{Operation, Scope};

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Start,
    Tick(f32),
    Report { node: u16, report: Report },
    Retire,
    Graph(Operation),
}

impl Session {
    pub fn hosted(
        &mut self,
        action: Action,
        inputs: &Inputs,
        scope: Scope<'_>,
    ) -> Result<Output, Error> {
        let starting = action == Action::Start;
        if self.state.retired || starting == self.started {
            return Err(Error::InvalidGraph);
        }
        let mut candidate = Candidate {
            program: &self.program,
            inputs,
            state: self.state.clone(),
        };
        let mut failed = None;
        // Reborrow the host and ancestor stack for this graph transaction only.
        let Scope {
            instance,
            host,
            mut upstream,
        } = scope;
        let result = self.graph.execute_hosted(
            &mut candidate,
            Scope {
                instance,
                host: &mut *host,
                upstream: upstream
                    .as_mut()
                    .map(|p| &mut **p as &mut dyn events::native::Callbacks),
            },
            |c, d| match action {
                Action::Start => c.startup(&mut failed, d),
                Action::Tick(delta) => c.tick_owned(delta, d),
                Action::Report { node, report } => c.report_owned(node, &report, d),
                Action::Retire | Action::Graph(Operation::Retire) => c.retire_owned(d),
                Action::Graph(Operation::Update { node }) => c.native_update(node, d),
                Action::Graph(Operation::Import(value)) => c.import(value, d),
                Action::Graph(Operation::Publish { port, value }) => d.publish(c, port, value),
                Action::Graph(Operation::Emit { source, context }) => {
                    c.emit_with(source, context, d)
                }
                Action::Graph(Operation::Deliver { target, context }) => {
                    c.deliver(target, context, d)
                }
            },
        );
        if starting {
            self.failed_step = failed;
        }
        let remote = result?;
        let output = self.commit_output(candidate.state, remote)?;
        if starting {
            self.started = true;
        }
        Ok(output)
    }
}

impl Candidate<'_> {
    pub(super) fn transition_request(
        &mut self,
        request: super::super::transitions::Request,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        if dispatch.is_hosted() {
            return dispatch.native(
                self,
                request.node,
                events::native::Call::Transition(request),
            );
        }
        if self.state.transitions.len() >= MAX_OUTPUT {
            return Err(Error::Bound);
        }
        self.state.transitions.push(request);
        Ok(())
    }

    pub(super) fn tick_owned(
        &mut self,
        delta: f32,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        for node in self.state.scheduled.clone() {
            if self.state.retired {
                break;
            }
            match &self.program.node(node)?.behavior {
                Behavior::Burnout(config) => self.burnout(node, *config, delta, dispatch)?,
                Behavior::VehicleQuery(_) => dispatch.vehicle_query(self, node)?,
                Behavior::Road { .. } => {
                    let frame = road::Frame {
                        now: self.inputs.now,
                        elapsed: delta,
                    };
                    dispatch.road(self, node, frame, self.inputs.road)?;
                }
                Behavior::Proximity => self.proximity(node, dispatch)?,
                Behavior::Stats => self.stats(node, dispatch)?,
                Behavior::RaceTime => self.race_time(node, dispatch)?,
                Behavior::Nos => self.nos(node, dispatch)?,
                Behavior::NosBuildup => self.buildup(node)?,
                Behavior::Donut(configuration) => {
                    self.donut(node, *configuration, delta, dispatch)?
                }
                _ => return Err(Error::InvalidGraph),
            }
        }
        dispatch.update_stopwatches(self, delta)?;
        dispatch.update_delays(self, delta)
    }

    pub(super) fn report_owned(
        &mut self,
        node: u16,
        report: &Report,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        let publication = report.publication();
        match (&self.program.node(node)?.behavior, report) {
            (Behavior::Collision(_), Report::Collision { value, reference }) => dispatch.collision(
                self,
                node,
                collision::Action::Report {
                    value: *value,
                    context: EntityContext { object: *reference },
                },
            ),
            (Behavior::NearMiss, Report::NearMiss(_))
            | (Behavior::AccumulatedCollisions, Report::CollisionWeight(_)) => {
                for (field, value) in publication.properties {
                    let port = Port { node, field };
                    if self.state.store.read(port).is_ok() {
                        dispatch.publish(self, port, value)?;
                    }
                }
                dispatch.emit(
                    self,
                    Endpoint {
                        node,
                        event: publication.event,
                    },
                )
            }
            _ => Err(Error::InvalidGraph),
        }
    }

    pub(super) fn retire_owned(&mut self, dispatch: &mut Dispatch<'_>) -> Result<(), Error> {
        if self.state.retired {
            return Ok(());
        }
        self.state.retired = true;
        // Native child contexts prepend entities (03a75830 -> 03760870 ->
        // 0379fff0). Deactivation03759e70 walks that list forward. Keep each
        // entity's bindings alive through its own deactivation callback.
        for (id, node) in self.program.nodes.iter().enumerate().rev() {
            if !node.created {
                continue;
            }
            self.ai_retire_node(id as u16)?;
            if let Some(state) = self.state.burnout_overrides.get_mut(&(id as u16))
                && let Some(change) = state.release()
            {
                if self.state.burnout_changes.len() >= MAX_OUTPUT {
                    return Err(Error::Bound);
                }
                self.state.burnout_changes.push(change);
            }
            if node.created
                && self.state.set_observers.contains(&(id as u16))
                && matches!(node.behavior, Behavior::ParticipantSetAccessor { .. })
            {
                dispatch.native(self, id as u16, events::native::Call::SetUnobserve)?;
            }
            if node.created
                && matches!(node.behavior, Behavior::ParticipantProperty { .. })
                && self
                    .state
                    .bound_accessors
                    .iter()
                    .any(|(n, _)| usize::from(*n) == id)
            {
                dispatch.native(self, id as u16, events::native::Call::ParticipantUnobserve)?;
            }
            if node.created && node.behavior == Behavior::Native {
                dispatch.native(self, id as u16, events::native::Call::Retire)?;
            }
            self.state.deactivated.insert(id as u16);
        }
        self.state.scheduled.clear();
        Ok(())
    }

    pub(super) fn import_owned(
        &mut self,
        value: events::native::Import,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        use events::native::Import;
        match value {
            Import::SetAccessor { node, change } => {
                change.set.validate()?;
                let Behavior::ParticipantSetAccessor {
                    set: Some(set),
                    provider,
                } = &self.program.node(node)?.behavior
                else {
                    return Err(Error::InvalidGraph);
                };
                if *set != change.set {
                    return Err(Error::InvalidGraph);
                }
                if self.state.set_observers.contains(&node)
                    && provider.and_then(|p| self.state.participant_providers.get(&p))
                        == Some(&change.participant)
                {
                    self.set_membership_output(node, change.member, dispatch)?;
                }
                Ok(())
            }
            Import::Accessor { node, write } => {
                write.property.validate()?;
                let kind = participant_properties::kind(&write.value)?;
                if !matches!(&self.program.node(node)?.behavior,
                    Behavior::ParticipantProperty { property, kind: k, .. }
                        if *property == write.property && *k == kind)
                {
                    return Err(Error::InvalidGraph);
                }
                if !self
                    .state
                    .bound_accessors
                    .contains(&(node, write.participant))
                {
                    return Ok(());
                }
                let key = (write.participant, write.property, kind);
                if !self.state.participant_values.contains_key(&key)
                    && self.state.participant_values.len() >= participant_properties::MAX_VALUES
                {
                    return Err(Error::Bound);
                }
                self.state
                    .participant_values
                    .insert(key, write.value.clone());
                // Callback payloads are not equality-filtered here. The host
                // store suppresses unchanged writes before visiting listeners.
                self.publish_bound(node, nodes::OUTPUT_VALUE, write.value, dispatch)
            }
            Import::Participant(write) => {
                write.property.validate()?;
                self.write_participant(
                    write.participant,
                    &write.property,
                    write.value,
                    None,
                    dispatch,
                )
            }
            Import::Set(change) => {
                change.set.validate()?;
                self.change_set(&change, None, dispatch)
            }
            Import::RankTable { sorter, rows } => {
                sorter.validate()?;
                super::super::rank_query::validate(&rows)?;
                if !self.state.rank_tables.contains_key(&sorter)
                    && self.state.rank_tables.len() >= super::super::rank_query::MAX_TABLES
                {
                    return Err(Error::Bound);
                }
                self.state.rank_tables.insert(sorter.clone(), rows);
                for node in self.state.rank_observers.clone() {
                    if matches!(&self.program.node(node)?.behavior,
                        Behavior::ParticipantRankQuery { sorter: key } if *key == sorter)
                    {
                        self.rank_query(node, &sorter)?;
                    }
                }
                Ok(())
            }
            Import::Context {
                source,
                participant,
                categories,
            } => {
                source.validate()?;
                if categories.len() > 128 {
                    return Err(Error::Bound);
                }
                let key = (source.clone(), participant);
                if !self.state.context_categories.contains_key(&key)
                    && self.state.context_categories.len() >= 256
                {
                    return Err(Error::Bound);
                }
                self.state
                    .context_categories
                    .insert(key, categories.into_iter().collect());
                for id in self.state.context_observers.clone() {
                    if let Behavior::ContextReporter {
                        source: key,
                        provider,
                        ..
                    } = &self.program.node(id)?.behavior
                        && *key == source
                        && self.state.participant_providers.get(provider) == Some(&participant)
                        && self.state.context_active.contains(&id)
                    {
                        self.context_match(id, false, dispatch)?;
                    }
                }
                Ok(())
            }
        }
    }

    pub(super) fn startup(
        &mut self,
        failed: &mut Option<usize>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), Error> {
        for (id, node) in self.program.nodes.iter().enumerate() {
            if node.created && node.behavior == Behavior::ParticipantProvider {
                self.state
                    .participant_providers
                    .insert(id as u16, self.inputs.participant);
            }
        }
        for (index, step) in self.program.startup.iter().enumerate() {
            *failed = Some(index);
            match step {
                Step::Bind(node) => self.bind(*node, dispatch)?,
                Step::Register(registration) => dispatch.register(self, *registration)?,
                Step::CopyReady { source, target } => {
                    if let Some(value) = self.state.store.read(*source)? {
                        dispatch.initialize(
                            self,
                            *target,
                            value,
                            super::super::properties::DefaultMode::Override,
                        )?;
                    }
                }
                Step::Default { port, value, mode } => {
                    dispatch.initialize(self, *port, value.clone(), *mode)?;
                }
                Step::Activate(node) => self.activate(*node, dispatch)?,
            }
        }
        *failed = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
