// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

#[derive(Clone, Debug)]
pub(super) struct Presentation {
    pub properties: BTreeMap<nfs_content::scoring::participant_properties::Property, i32>,
    pub awards: Option<nfs_content::events::awards::ResultValues>,
}

impl PlayerListener {
    /// Finish rank, clock and rewards commit at the crossing. Only presentation
    /// waits when crash recovery already owns this participant's sequence. This
    /// prevents a late crash completion from restoring driving over the outro.
    pub(super) fn event_finish_presentation(
        &self,
        content: &event_logic::Content,
        players: &mut Players,
        state: &mut State,
        participant: u16,
        now_ms: u64,
        out: &mut Out,
    ) -> Result<(), replication::Error> {
        let race = state
            .entries
            .get_mut(&participant)
            .and_then(|e| e.race.as_mut())
            .ok_or(replication::Error::Shape)?;
        let Some(snapshot) = race.pending_outro.take() else {
            return Ok(());
        };
        let finish = race.finish.ok_or(replication::Error::Shape)?;
        let game_mode = race.game_mode;
        let mut creations = Vec::new();
        for logic in [Logic::AddSet(1), Logic::Outro, Logic::Awards] {
            let (name, occurrence) = logic.name();
            let layout = content.logic(Owner::GameMode, name, occurrence)?;
            creations.push(content.game_mode_logic(
                layout,
                players.objects(),
                participant,
                game_mode,
                Some(event_logic::Results {
                    finish,
                    properties: &snapshot.properties,
                    awards: snapshot.awards.as_ref(),
                }),
            )?);
        }
        let mut records = players.spawn_entities(creations, content.creation_content())?;
        for (logic, record) in [Logic::AddSet(1), Logic::Outro, Logic::Awards]
            .into_iter()
            .zip(&records)
        {
            race.logics.insert(logic, record.id);
        }
        let outro = content.sequence_creation(
            names::OUTRO_SEQUENCE,
            players.objects(),
            participant,
            game_mode,
        )?;
        let outro = players
            .spawn_entities(vec![outro], content.creation_content())?
            .remove(0);
        race.sequence = Some((Sequence::Outro, outro.id, false));
        records.push(outro);
        // The results screen drives the car (E1280: at the human's finish frame
        // the official host creates the outro's authored wander and speed-limit
        // goals and sets the Actor's first flag with them; both go at the outro's
        // end). Goals the intro left on the Actor are released first; a refused
        // assignment keeps the finish and is logged.
        let held = std::mem::take(&mut race.goals);
        release_goals(players, self.persona, participant, held, out);
        if content.sequence(names::OUTRO_SEQUENCE)?.wander.is_some() {
            match players.start_controlled_goals(
                HOST_SELECTOR as u8,
                self.persona,
                participant,
                content.outro_goals()?,
            ) {
                Ok(assigned) => {
                    race.goals = assigned[..assigned.len() - 1]
                        .iter()
                        .map(|r| r.id)
                        .collect();
                    tracing::info!(
                        participant,
                        goals = ?race.goals,
                        "owned event outro goals assigned; the Actor drives the results screen"
                    );
                    records.extend(assigned);
                }
                Err(error) => tracing::warn!(
                    participant,
                    ?error,
                    "owned event outro goals not assigned; the results screen keeps manual control"
                ),
            }
        }
        let mut gone = Vec::new();
        for logic in [
            Logic::Restart,
            Logic::Leave,
            Logic::AddSet(0),
            Logic::OngoingScore,
            Logic::ResultTracking,
            Logic::DrivingStyle,
        ] {
            gone.extend(race.logics.remove(&logic));
        }
        players.remove_entities(&gone)?;
        state
            .steps
            .push_back((now_ms + FINISH_FOLLOW_MS, participant, Step::Finished));
        out.records.extend(records);
        out.deleted.extend(gone);
        Ok(())
    }
}
