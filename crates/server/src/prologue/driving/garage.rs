// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scene membership feeds the mode's imported state aliases. The scene owner
//! sends its RPCs; the mode only publishes effects of its own observers.
use super::*;

impl Active {
    pub fn scene_notification(
        &mut self,
        content: &Mode,
        players: &mut Players,
        notification: nfs_world::participants::Notification,
    ) -> Result<Output, Error> {
        use nfs_world::participants::Call;
        if notification.participant != self.owner.participant {
            return Err(Error::UnknownObject);
        }
        let entered = match notification.call {
            Call::Enter => true,
            Call::Leave | Call::LeaveTagged(_) => false,
            _ => return Ok(Output::default()),
        };
        let sources = content
            .plan
            .data()
            .states
            .iter()
            .filter_map(|s| content.plan.external_state(s.serializer as u16))
            .filter(|s| {
                players
                    .objects()
                    .scene_endpoint(s.scene_content_key, s.state.serializer)
                    == Some((notification.endpoint.scene, notification.endpoint.selector))
            })
            .map(|s| (s.scene_content_key, s.state.serializer))
            .collect::<BTreeSet<_>>();
        let mut out = Output::default();
        for (key, serializer) in sources {
            let value = self.scene_state(content, players, key, serializer, entered)?;
            out.records.extend(value.records);
            out.departures.extend(value.departures);
            out.deleted.extend(value.deleted);
            out.calls.extend(value.calls);
        }
        Ok(out)
    }
    pub fn destination_reached(&self) -> bool {
        self.progress
            .as_ref()
            .and_then(|(_, r)| r.participant(self.owner.participant))
            .is_some_and(|p| p.finished.is_some())
    }
    pub fn has_left(&self) -> bool {
        self.left
    }

    pub fn scene_state(
        &mut self,
        content: &Mode,
        players: &mut Players,
        key: u32,
        serializer: usize,
        entered: bool,
    ) -> Result<Output, Error> {
        if self.left {
            return Ok(Output::default());
        }
        let actions = content
            .plan
            .data()
            .states
            .iter()
            .filter_map(|s| {
                let imported = content.plan.external_state(s.serializer as u16)?;
                (imported.scene_content_key == key && imported.state.serializer == serializer)
                    .then_some(if entered {
                        Action::Enter {
                            state: s.serializer as u16,
                            participant: self.owner.participant.into(),
                        }
                    } else {
                        Action::Remove {
                            state: s.serializer as u16,
                            participant: self.owner.participant.into(),
                        }
                    })
            })
            .collect::<Vec<_>>();
        if actions.is_empty() {
            return Ok(Output::default());
        }
        let mut next = self.clone();
        let mut staged = players.clone();
        let mut out = Output::default();
        let effects = next.dispatch(actions.into_iter().map(Command::State).collect())?;
        next.effects(content, &mut staged, &effects, false, &mut out)?;
        *self = next;
        *players = staged;
        Ok(out)
    }
    pub(super) fn finish_if_final(
        &mut self,
        content: &Mode,
        players: &Players,
        out: &mut Output,
    ) -> Result<(), Error> {
        if !self.ending
            && self
                .host
                .machine()
                .members(content.data.game_mode.lifecycle.final_state as u16)
                .map_err(invalid)?
                .contains(&u64::from(self.owner.participant))
        {
            for method in [1, 7] {
                out.calls.push(HostRpc::GameMode(event_logic::call(
                    players.objects(),
                    self.ghost,
                    0,
                    method,
                    None,
                    Arguments::None,
                )?));
            }
            self.ending = true;
        }
        Ok(())
    }

    /// Only the owner's acknowledgement after final-state publication retires
    /// this mode. The event canceller has a separate, longer lifetime.
    pub fn leave(
        &mut self,
        content: &Mode,
        players: &mut Players,
        participant: u16,
    ) -> Result<Output, Error> {
        if participant != self.owner.participant {
            return Err(Error::UnknownObject);
        }
        let mut out = Output::default();
        if self.left {
            return Ok(out);
        }
        if !self.ending
            || !self.goals.is_empty()
            || self
                .logics
                .keys()
                .any(|(_, p)| *p == self.owner.participant)
            || self.routes.values().any(|(_, goal)| goal.is_some())
            || self
                .pacing
                .as_ref()
                .is_some_and(|(_, tokens, _)| tokens.contains_key(&self.owner.participant))
        {
            return Err(Error::Shape);
        }
        let mut staged = players.clone();
        let mut next = self.clone();
        if let Some((_, _, command)) = &mut next.pacing {
            let changes = command.clear(&mut staged)?;
            out.records.extend(changes.records);
            out.deleted.extend(changes.deleted);
        }
        next.pacing = None;
        for ghost in next.logics.values().copied() {
            staged.remove_entities(&[ghost])?;
            out.deleted.push(ghost);
        }
        next.logics.clear();
        if let Some(racers) = &mut next.opponents {
            let changes = content.opponents().ok_or(Error::Unsupported)?.shutdown(
                racers,
                &mut staged,
                self.owner.connection,
                self.owner.persona,
            )?;
            out.departures
                .extend(changes.leave.into_iter().map(HostRpc::Participant));
            out.departures
                .extend(changes.untracked.into_iter().map(HostRpc::WorldActivation));
            out.records.extend(changes.records);
            out.deleted.extend(changes.deleted);
        }
        next.opponents = None;
        next.opponent_deadlines.clear();
        next.contexts.clear();
        next.booleans.clear();
        staged.remove_entities(&[self.ghost])?;
        out.deleted.push(self.ghost);
        for &(key, slot) in &content.data.game_mode.checkpoints {
            let (target, selector) = staged
                .objects()
                .scene_endpoint(key, slot)
                .ok_or(Error::UnknownObject)?;
            out.calls.push(HostRpc::GameMode(event_logic::call(
                staged.objects(),
                target,
                selector,
                1,
                None,
                Arguments::None,
            )?));
        }
        *players = staged;
        next.left = true;
        *self = next;
        Ok(out)
    }
}
