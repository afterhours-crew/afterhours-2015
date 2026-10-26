// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared startup membership driven by scene-service domain operations. The
//! service owns wire publication; this host owns observer-visible transitions.
use crate::event_logic::lifecycle::{
    Effect, Plan,
    commands::Command,
    world::{Instance, Scoped, WorldHost},
};
use nfs_content::assets::AssetIndex;
use nfs_world::{
    participant_states::{Action, Membership},
    participants::{Endpoint, Lifecycle, membership::Operation},
    replication::{Error, Initial, players::Players},
};
use std::{collections::BTreeMap, sync::Arc};
mod presentation;

#[derive(Clone, Debug)]
pub struct Content {
    key: u32,
    plan: Arc<Plan>,
    presentation: Vec<presentation::Definition>,
}
impl Content {
    #[cfg(test)]
    pub(crate) fn with_test_presentation(
        mut self,
        layout: nfs_content::participant_logic::Layout,
        states: &[usize],
    ) -> Self {
        let mut scope = self.plan.data().clone();
        let predicate = nfs_content::participant_logic::activation::Predicate::Property(
            nfs_world::participant_states::properties::Definition::List(
                states
                    .iter()
                    .map(|&serializer| nfs_content::events::discovery::Endpoint {
                        scene_key: self.key,
                        serializer,
                    })
                    .collect(),
            ),
        );
        presentation::install(&mut scope, &predicate, self.key, "test/presentation").unwrap();
        self.plan = Arc::new(Plan::scene(&scope).unwrap());
        self.presentation
            .push(presentation::Definition::new(layout).unwrap());
        self
    }
    #[cfg(test)]
    pub(crate) fn for_test(key: u32, serializers: impl IntoIterator<Item = usize>) -> Self {
        use nfs_content::events::lifecycle::{Scope, State, StateConfiguration, StateKind};
        let scope = Scope {
            states: serializers
                .into_iter()
                .map(|serializer| State {
                    serializer,
                    kind: StateKind::HostOwned,
                    configuration: StateConfiguration::Default,
                    handles_marked_for_delete: true,
                    send_rpc_to_client: true,
                })
                .collect(),
            ..Scope::default()
        };
        Self {
            key,
            plan: Arc::new(Plan::scene(&scope).unwrap()),
            presentation: vec![],
        }
    }
    pub fn prepare(index: &mut AssetIndex) -> Result<Self, nfs_content::Error> {
        let source =
            index.participant_scene_membership("levels/genesis01/participantstatemachine")?;
        let scene = "levels/genesis01/gameplay";
        let mut scope = source.scope();
        let mut presentation = Vec::new();
        for blueprint in presentation::BLUEPRINTS {
            let predicate = index.participant_state_predicate(scene, blueprint)?;
            presentation::install(&mut scope, &predicate, source.content_key, blueprint).map_err(
                |e| {
                    nfs_content::Error::Content(format!(
                        "presentation controller {blueprint}: {e:?}"
                    ))
                },
            )?;
            presentation.push(presentation::Definition::prepare(index, scene, blueprint)?);
        }
        let plan = Plan::scene(&scope)
            .map_err(|e| nfs_content::Error::Content(format!("startup membership: {e:?}")))?;
        Ok(Self {
            key: source.content_key,
            plan: Arc::new(plan),
            presentation,
        })
    }
    pub fn assets(
        &self,
    ) -> impl Iterator<Item = nfs_world::replication::entity::creation::Asset> + '_ {
        self.presentation
            .iter()
            .map(presentation::Definition::asset)
    }
    fn endpoint(&self, players: &Players, endpoint: Endpoint) -> Result<u16, Error> {
        let scene = players
            .objects()
            .scene(self.key)
            .ok_or(Error::UnknownObject)?;
        if scene != endpoint.scene {
            return Err(Error::UnknownObject);
        }
        let object = players.objects().get(scene).ok_or(Error::UnknownObject)?;
        let Initial::SubLevel { fields, .. } = &object.initial else {
            return Err(Error::TypeMismatch);
        };
        let mut found = self.plan.data().states.iter().filter(|state| {
            fields
                .get(state.serializer)
                .and_then(nfs_world::replication::sublevel::Initial::rpc)
                .is_some_and(|rpc| {
                    rpc.selector == endpoint.selector && rpc.serial == endpoint.serial
                })
        });
        let state = found.next().ok_or(Error::UnknownObject)?;
        if found.next().is_some() {
            return Err(Error::DuplicateObject);
        }
        u16::try_from(state.serializer).map_err(|_| Error::Bound)
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    host: WorldHost,
    instance: Instance,
    ghosts: BTreeMap<(usize, u16), u16>,
}
impl Session {
    #[cfg(test)]
    pub(crate) fn contains(&self, serializer: u16, participant: u16) -> bool {
        self.host
            .view(self.instance)
            .unwrap()
            .members(serializer)
            .unwrap()
            .contains(&participant.into())
    }
    fn new(content: &Content) -> Result<Self, Error> {
        let mut host = WorldHost::default();
        let instance = host
            .install(content.plan.clone(), Some(content.key))
            .map_err(invalid)?;
        let commands = content
            .plan
            .data()
            .bindings
            .order
            .iter()
            .map(|&observer| Scoped {
                instance,
                value: Command::Activate {
                    observer,
                    binding: false,
                },
            })
            .collect();
        host.dispatch(commands, &mut |_, _| Ok(vec![]))
            .map_err(invalid)?;
        Ok(Self {
            host,
            instance,
            ghosts: BTreeMap::new(),
        })
    }
    fn apply(
        &mut self,
        content: &Content,
        players: &mut Players,
        owner: (u8, u64),
        operation: Operation,
        output: &mut Staged,
    ) -> Result<(), Error> {
        let endpoint = |e| content.endpoint(players, e);
        let action = match operation {
            Operation::Enter { participant, state } => Action::Enter {
                participant: participant.into(),
                state: endpoint(state)?,
            },
            Operation::Complete { participant, state } => Action::Complete {
                participant: participant.into(),
                state: endpoint(state)?,
            },
            Operation::Remove { participant, state } => Action::Remove {
                participant: participant.into(),
                state: endpoint(state)?,
            },
            Operation::Move {
                participant,
                source,
                target,
            } => Action::Move {
                participant: participant.into(),
                source: endpoint(source)?,
                target: target.map(endpoint).transpose()?,
            },
        };
        let participant = match &action {
            Action::Enter { participant, .. }
            | Action::Complete { participant, .. }
            | Action::Remove { participant, .. }
            | Action::Move { participant, .. } => *participant,
            _ => unreachable!(),
        };
        if !players.owns_participant(owner.0, owner.1, participant as u16) {
            return Err(Error::UnknownObject);
        }
        // A missing source would otherwise be a valid native no-op and hide a
        // missing service hook. Domain operations must originate in this owner.
        if let Action::Complete { state, participant }
        | Action::Remove { state, participant }
        | Action::Move {
            source: state,
            participant,
            ..
        } = &action
            && !self
                .host
                .view(self.instance)
                .map_err(invalid)?
                .members(*state)
                .map_err(invalid)?
                .contains(participant)
        {
            tracing::warn!(
                ?operation,
                "scene domain operation has no source membership"
            );
            return Err(Error::Shape);
        }
        let effects = self
            .host
            .dispatch(
                vec![Scoped {
                    instance: self.instance,
                    value: Command::State(action),
                }],
                &mut |_, _| Ok(vec![]),
            )
            .map_err(invalid)?;
        for effect in effects {
            let Effect::Logic {
                spawner,
                participant,
                create,
            } = effect.value
            else {
                continue;
            };
            let participant = u16::try_from(participant).map_err(|_| Error::Bound)?;
            let key = (spawner, participant);
            if create {
                if self.ghosts.contains_key(&key) {
                    return Err(Error::DuplicateObject);
                }
                let definition = content.presentation.get(spawner).ok_or(Error::Shape)?;
                let record = definition.spawn(players, participant)?;
                self.ghosts.insert(key, record.id);
                output.records.push(record);
            } else {
                let ghost = self.ghosts.remove(&key).ok_or(Error::UnknownObject)?;
                players.remove_entities(&[ghost])?;
                output.deleted.push(ghost);
            }
        }
        Ok(())
    }
}
fn invalid(error: nfs_world::participant_states::Error) -> Error {
    match error {
        nfs_world::participant_states::Error::Bound => Error::Bound,
        _ => Error::Shape,
    }
}

/// Returns a new candidate. The caller commits it with players, lifecycle and
/// admitted output, so queue pressure or a later failure cannot move membership.
#[derive(Default)]
pub struct Staged {
    pub session: Option<Session>,
    pub records: Vec<nfs_world::replication::Record>,
    pub deleted: Vec<u16>,
}
pub fn stage(
    content: Option<&Content>,
    current: Option<&Session>,
    players: &mut Players,
    owner: (u8, u64),
    lifecycle: &mut Lifecycle,
) -> Result<Staged, Error> {
    let operations = lifecycle.take_state_operations()?;
    let mut output = Staged::default();
    let Some(content) = content else {
        return Ok(output);
    };
    let mut next = match current {
        Some(current) => current.clone(),
        None => Session::new(content)?,
    };
    for operation in operations {
        next.apply(content, players, owner, operation, &mut output)?;
    }
    output.session = Some(next);
    Ok(output)
}
