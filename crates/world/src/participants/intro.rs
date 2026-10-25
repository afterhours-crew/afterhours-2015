// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! First-drive startup inside the participant's existing lifecycle. The caller
//! owns the temporary car, teleport command, opening sequence and first mode.
//! These gates never select or grant an inventory item or start a timer.
use super::*;

/// Transient flow position. The session restores it from durable story state;
/// it is never itself proof that an objective has completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Chapter {
    FirstDrive,
    FirstRace,
    SecondRace,
    ReturnDrive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bindings {
    pub parameters: Endpoint,
    pub spawning: Endpoint,
    pub teleport: Endpoint,
    pub check_player: Endpoint,
    pub render_wait: Endpoint,
    pub render_ready: Endpoint,
    pub opening: Endpoint,
    pub alive: Endpoint,
    pub pending_items: Endpoint,
    pub relocating: Endpoint,
}
impl Bindings {
    pub fn from_records(records: &[Record], startup_key: u32) -> Result<Option<Self>, Error> {
        let mut found = records.iter().filter_map(|record| match &record.initial {
            Some(Initial::SubLevel { prefix, fields }) if prefix.content_key == startup_key => {
                Some((record.id, fields))
            }
            _ => None,
        });
        let Some((scene, fields)) = found.next() else {
            return Ok(None);
        };
        if found.next().is_some() {
            return Err(Error::DuplicateObject);
        }
        let endpoint = |index| {
            let rpc = match fields.get(index) {
                Some(
                    sublevel::Initial::Rpc(rpc) | sublevel::Initial::RpcReferences { rpc, .. },
                ) => rpc,
                _ => return Err(Error::TypeMismatch),
            };
            Ok(Endpoint {
                scene,
                selector: rpc.selector,
                serial: rpc.serial,
            })
        };
        Ok(Some(Self {
            parameters: endpoint(4)?,
            spawning: endpoint(3)?,
            teleport: endpoint(77)?,
            check_player: endpoint(83)?,
            render_wait: endpoint(78)?,
            render_ready: endpoint(81)?,
            opening: endpoint(79)?,
            alive: endpoint(2)?,
            pending_items: endpoint(94)?,
            relocating: endpoint(85)?,
        }))
    }
}
fn notifications(participant: u16, chain: &[(Endpoint, Call)]) -> Vec<Notification> {
    chain
        .iter()
        .map(|&(endpoint, call)| Notification {
            endpoint,
            participant,
            call,
        })
        .collect()
}
impl Lifecycle {
    /// The server owner verified durable completion and retired both modes.
    /// Ordinary garage exit now owns subsequent world entry.
    pub fn finish_intro(&mut self, participant: u16) -> Outcome {
        if !self.intro_started.contains_key(&participant) {
            return Outcome::Repeated;
        }
        if self.intro_started.get(&participant) != Some(&Chapter::ReturnDrive)
            || self.stages.get(&participant) != Some(&Stage::Customization)
        {
            return Outcome::Unsupported;
        }
        self.intro_started.remove(&participant);
        Outcome::Advanced(vec![])
    }
    pub fn intro_final_garage_ready(&mut self, participant: u16, composite: Endpoint) -> Outcome {
        let Some(b) = self.bindings else {
            return Outcome::Unsupported;
        };
        if composite != b.composite
            || self.intro_started.get(&participant) != Some(&Chapter::ReturnDrive)
        {
            return Outcome::Unsupported;
        }
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::IntroLoadingGarage) => {
                *stage = Stage::IntroComposite;
                Outcome::Advanced(notifications(
                    participant,
                    &[(b.loading_garage, Call::Leave), (b.composite, Call::Enter)],
                ))
            }
            Some(Stage::IntroComposite | Stage::Customization) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }
    pub fn intro_final_composite_complete(&mut self, participant: u16) -> Outcome {
        let Some(b) = self.bindings else {
            return Outcome::Unsupported;
        };
        if self.intro_started.get(&participant) != Some(&Chapter::ReturnDrive) {
            return Outcome::Unsupported;
        }
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::IntroComposite) => {
                *stage = Stage::Customization;
                Outcome::Advanced(notifications(
                    participant,
                    &[
                        (b.composite, Call::Leave),
                        (b.initial_teleport, Call::Enter),
                        (b.initial_teleport, Call::Leave),
                        (b.customization, Call::Enter),
                    ],
                ))
            }
            Some(Stage::Customization) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }
    /// The owner has completed the diner movie, retired the race and verified
    /// the selected world car. This mode starts in place without relocation.
    pub fn intro_return_drive_begin(&mut self, participant: u16) -> Outcome {
        if self.intro_started.get(&participant) == Some(&Chapter::ReturnDrive) {
            return Outcome::Repeated;
        }
        if self.intro.is_none()
            || self.intro_started.get(&participant) != Some(&Chapter::SecondRace)
            || self.stages.get(&participant) != Some(&Stage::FreeRoam)
        {
            return Outcome::Unsupported;
        }
        self.intro_started.insert(participant, Chapter::ReturnDrive);
        Outcome::Advanced(vec![])
    }
    /// The story owner validated the completed movie, retired previous mode
    /// and existing selected car. This path retains that car during relocation.
    pub fn intro_second_race_begin(&mut self, participant: u16) -> Outcome {
        let Some(i) = self.intro else {
            return Outcome::Unsupported;
        };
        if self.intro_started.get(&participant) == Some(&Chapter::SecondRace) {
            return Outcome::Repeated;
        }
        if self.intro_started.get(&participant) != Some(&Chapter::FirstRace)
            || self.stages.get(&participant) != Some(&Stage::FreeRoam)
        {
            return Outcome::Unsupported;
        }
        self.intro_started.insert(participant, Chapter::SecondRace);
        self.stages.insert(participant, Stage::IntroRelocating);
        Outcome::Advanced(notifications(
            participant,
            &[(i.alive, Call::Leave), (i.relocating, Call::Enter)],
        ))
    }
    /// The installed leave-world provider has no selected sequence. Native
    /// StateSequence completes immediately on a null provider result.
    pub fn intro_relocation_ready(&mut self, participant: u16) -> Outcome {
        let Some(i) = self.intro else {
            return Outcome::Unsupported;
        };
        if self.intro_started.get(&participant) != Some(&Chapter::SecondRace) {
            return Outcome::Unsupported;
        }
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::IntroRelocating) => {
                *stage = Stage::IntroTeleporting;
                Outcome::Advanced(notifications(
                    participant,
                    &[(i.relocating, Call::Leave), (i.teleport, Call::Enter)],
                ))
            }
            Some(
                Stage::IntroTeleporting
                | Stage::IntroRendering
                | Stage::IntroOpening
                | Stage::IntroStartingDrive
                | Stage::FreeRoam,
            ) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }
    /// The story owner has verified the durable movie objective is active.
    /// Suspend driving presentation while its native sequence/movie gates run.
    pub fn intro_movie_begin(&mut self, participant: u16) -> Outcome {
        let Some(i) = self.intro else {
            return Outcome::Unsupported;
        };
        if !matches!(
            self.intro_started.get(&participant),
            Some(Chapter::FirstRace | Chapter::SecondRace)
        ) {
            return Outcome::Unsupported;
        }
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::FreeRoam) => {
                *stage = Stage::IntroMovie;
                Outcome::Advanced(notifications(participant, &[(i.alive, Call::Leave)]))
            }
            Some(Stage::IntroMovie) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }
    /// Only the owned after-sequence acknowledgement releases this gate.
    pub fn intro_movie_complete(&mut self, participant: u16) -> Outcome {
        let Some(i) = self.intro else {
            return Outcome::Unsupported;
        };
        if !matches!(
            self.intro_started.get(&participant),
            Some(Chapter::FirstRace | Chapter::SecondRace)
        ) {
            return Outcome::Unsupported;
        }
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::IntroMovie) => {
                *stage = Stage::FreeRoam;
                Outcome::Advanced(notifications(participant, &[(i.alive, Call::Enter)]))
            }
            Some(Stage::FreeRoam) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }
    pub fn intro_participants(&self) -> impl Iterator<Item = u16> + '_ {
        self.intro_started.keys().copied()
    }
    pub fn intro_chapter(&self, participant: u16) -> Option<Chapter> {
        self.intro_started.get(&participant).copied()
    }
    pub fn return_bindings(&self) -> Option<ReturnBindings> {
        self.returning
    }

    /// The story owner validates destination progress before calling this.
    /// Intro arrival is automatic and has no owned-car slot refresh.
    pub fn begin_intro_return(&mut self, participant: u16) -> Outcome {
        let Some(bindings) = self.returning else {
            return Outcome::Unsupported;
        };
        if !self.intro_started.contains_key(&participant) {
            return Outcome::Unsupported;
        }
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::FreeRoam) => {
                *stage = Stage::IntroEnteringGarage;
                Outcome::Advanced(bindings.entering(participant, None))
            }
            Some(
                Stage::IntroEnteringGarage | Stage::IntroLoadingGarage | Stage::IntroCarSelection,
            ) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }

    /// The caller has completed the owned garage waiting sequence. The intro's
    /// composite remains entered while its car-selection sequence runs.
    pub fn intro_garage_ready(&mut self, participant: u16) -> Outcome {
        let Some(bindings) = self.bindings else {
            return Outcome::Unsupported;
        };
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::IntroLoadingGarage) => {
                *stage = Stage::IntroCarSelection;
                Outcome::Advanced(notifications(
                    participant,
                    &[
                        (bindings.loading_garage, Call::Leave),
                        (bindings.composite, Call::Enter),
                    ],
                ))
            }
            Some(Stage::IntroCarSelection) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }

    /// The caller has committed the selected car, drained item notifications
    /// and validated the native selection completion. The owned world car and
    /// its readiness still gate teleport and the next arrival sequence.
    pub fn intro_selection_complete(&mut self, participant: u16) -> Outcome {
        let (Some(b), Some(i)) = (self.bindings, self.intro) else {
            return Outcome::Unsupported;
        };
        if self.intro_started.get(&participant) == Some(&Chapter::FirstRace) {
            return Outcome::Repeated;
        }
        if self.intro_started.get(&participant) != Some(&Chapter::FirstDrive)
            || self.stages.get(&participant) != Some(&Stage::IntroCarSelection)
        {
            return Outcome::Unsupported;
        }
        self.intro_started.insert(participant, Chapter::FirstRace);
        self.stages.insert(participant, Stage::IntroSpawning);
        Outcome::Advanced(notifications(
            participant,
            &[
                (b.composite, Call::Leave),
                (b.initial_teleport, Call::Enter),
                (b.initial_teleport, Call::Leave),
                (i.pending_items, Call::Enter),
                (i.pending_items, Call::Leave),
                (i.parameters, Call::Enter),
                (i.parameters, Call::LeaveTagged(31)),
                (i.spawning, Call::Enter),
            ],
        ))
    }
    pub fn bind_intro(&mut self, records: &[Record], startup_key: u32) -> Result<bool, Error> {
        if self.intro.is_some() {
            return Err(Error::DuplicateObject);
        }
        self.intro = Bindings::from_records(records, startup_key)?;
        Ok(self.intro.is_some())
    }
    pub fn intro_bindings(&self) -> Option<Bindings> {
        self.intro
    }
    /// Route intro gates before other owners of the shared teleport endpoint.
    pub fn receive_intro(
        &mut self,
        body: BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<Outcome>, Error> {
        if self.intro.is_none() {
            return Ok(None);
        }
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        self.intro_route(&envelope, &route, &owns)
    }

    /// Start from restored progress, keeping the persistent garage untouched.
    /// The zero item reference and actor association precede spawning. Vehicle
    /// construction/binding must finish before `intro_vehicle_ready` is called.
    pub fn enter_intro_with_actor(
        &mut self,
        actor: crate::actors::Binding,
    ) -> Result<Vec<HostRpc>, Error> {
        actor.encode()?;
        let b = self.bindings.ok_or(Error::UnknownObject)?;
        let i = self.intro.ok_or(Error::UnknownObject)?;
        let stage = self
            .stages
            .get_mut(&actor.participant)
            .ok_or(Error::UnknownObject)?;
        if *stage != Stage::Progressing {
            return Err(Error::Shape);
        }
        if actor.endpoint.scene != b.manager.scene {
            return Err(Error::UnknownObject);
        }
        let participant = actor.participant;
        let mut out = notifications(
            participant,
            &[
                (b.persistent_loaded, Call::Leave),
                (b.recovery, Call::Enter),
                (b.recovery, Call::Leave),
                (b.progression_loaded, Call::Enter),
                (b.progression_loaded, Call::Leave),
                (b.entered_game, Call::Enter),
            ],
        )
        .into_iter()
        .map(HostRpc::Participant)
        .collect::<Vec<_>>();
        out.push(HostRpc::Garage(crate::garage::Binding {
            endpoint: b.garage[0],
            participant,
            item: 0,
        }));
        out.extend(
            notifications(
                participant,
                &[(b.entered_game, Call::Leave), (i.parameters, Call::Enter)],
            )
            .into_iter()
            .map(HostRpc::Participant),
        );
        out.push(HostRpc::Actor(actor));
        out.extend(
            notifications(
                participant,
                &[
                    (i.parameters, Call::LeaveTagged(31)),
                    (i.spawning, Call::Enter),
                ],
            )
            .into_iter()
            .map(HostRpc::Participant),
        );
        // Validate all output before committing lifecycle state.
        for rpc in &out {
            rpc.encode()?;
        }
        *stage = Stage::IntroSpawning;
        self.intro_started.insert(participant, Chapter::FirstDrive);
        Ok(out)
    }
    pub fn intro_vehicle_ready(&mut self, participant: u16) -> Outcome {
        let Some(i) = self.intro else {
            return Outcome::Unsupported;
        };
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::IntroSpawning) => {
                *stage = Stage::IntroTeleporting;
                Outcome::Advanced(notifications(
                    participant,
                    &[(i.spawning, Call::Leave), (i.teleport, Call::Enter)],
                ))
            }
            Some(
                Stage::IntroTeleporting
                | Stage::IntroRendering
                | Stage::IntroOpening
                | Stage::IntroStartingDrive
                | Stage::FreeRoam,
            ) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }
    /// Call only after the owned opening sequence's validated completion.
    /// The first mode and car bindings still gate normal driving presentation.
    pub fn intro_opening_complete(&mut self, participant: u16) -> Outcome {
        let Some(i) = self.intro else {
            return Outcome::Unsupported;
        };
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::IntroOpening) => {
                *stage = Stage::IntroStartingDrive;
                Outcome::Advanced(notifications(participant, &[(i.opening, Call::Leave)]))
            }
            Some(Stage::IntroStartingDrive | Stage::FreeRoam) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }
    pub fn intro_drive_ready(&mut self, participant: u16) -> Outcome {
        let Some(i) = self.intro else {
            return Outcome::Unsupported;
        };
        match self.stages.get_mut(&participant) {
            Some(stage @ Stage::IntroStartingDrive) => {
                *stage = Stage::FreeRoam;
                Outcome::Advanced(notifications(participant, &[(i.alive, Call::Enter)]))
            }
            Some(Stage::FreeRoam) => Outcome::Repeated,
            _ => Outcome::Unsupported,
        }
    }
    pub(super) fn intro_route(
        &mut self,
        envelope: &Envelope<'_>,
        route: &nfs_protocol::world::rpc::Route<'_>,
        owns: &impl Fn(u16) -> bool,
    ) -> Result<Option<Outcome>, Error> {
        let Some(i) = self.intro else {
            return Ok(None);
        };
        let matches = |e: Endpoint, method| {
            envelope.references().first() == Some(&e.scene)
                && route.selector() == e.selector
                && route.method_index() == method
        };
        let teleported = matches(i.teleport, 3);
        if !teleported && !matches(i.render_ready, 0) {
            return Ok(None);
        }
        let &[_, participant] = envelope.references() else {
            return Err(Error::Shape);
        };
        if !owns(participant) {
            return Err(Error::UnknownObject);
        }
        let stage = self
            .stages
            .get_mut(&participant)
            .ok_or(Error::UnknownObject)?;
        if !self.intro_started.contains_key(&participant) {
            return Ok(None);
        }
        if !matches!(
            stage,
            Stage::IntroSpawning
                | Stage::IntroTeleporting
                | Stage::IntroRendering
                | Stage::IntroOpening
                | Stage::IntroStartingDrive
                | Stage::FreeRoam
        ) {
            return Ok(None);
        }
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || route.arguments().len() != 7
        {
            return Err(Error::Shape);
        }
        let chain = match (teleported, *stage) {
            (true, Stage::IntroTeleporting) => {
                *stage = Stage::IntroRendering;
                notifications(
                    participant,
                    &[
                        (i.teleport, Call::Leave),
                        (i.check_player, Call::Enter),
                        (i.check_player, Call::Leave),
                        (i.render_wait, Call::Enter),
                    ],
                )
            }
            (false, Stage::IntroRendering) => {
                *stage = Stage::IntroOpening;
                notifications(
                    participant,
                    &[(i.render_wait, Call::Leave), (i.opening, Call::Enter)],
                )
            }
            (
                true,
                Stage::IntroRendering
                | Stage::IntroOpening
                | Stage::IntroStartingDrive
                | Stage::FreeRoam,
            )
            | (false, Stage::IntroOpening | Stage::IntroStartingDrive | Stage::FreeRoam) => {
                return Ok(Some(Outcome::Repeated));
            }
            _ => return Err(Error::Shape),
        };
        Ok(Some(Outcome::Advanced(chain)))
    }
}

#[cfg(test)]
mod tests;
