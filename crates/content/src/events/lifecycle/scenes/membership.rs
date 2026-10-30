// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::events::lifecycle::{Scope, State, StateConfiguration, StateKind};
use crate::graph::Node;

/// The constructor occurrence is retained even when two prefab instances use
/// the same data object. Serializer identity belongs to this scene occurrence.
#[derive(Clone, Debug, PartialEq)]
pub struct MemberState {
    pub identity: Identity,
    pub class: String,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Membership {
    pub content_key: u32,
    pub states: Vec<MemberState>,
}
impl Membership {
    /// Membership storage only. The scene service must drive actual Enter,
    /// Complete, Move and Remove operations; network RPCs are not that input.
    /// No transitions, observers, initial state or completion hook are invented.
    pub fn scope(&self) -> Scope {
        Scope {
            states: self.states.iter().map(|s| s.state.clone()).collect(),
            ..Scope::default()
        }
    }
}

fn extract<'a>(
    content_key: u32,
    rows: impl IntoIterator<Item = (&'a Node, Option<usize>)>,
) -> Result<Membership, Error> {
    let mut states = Vec::new();
    let mut identities = BTreeSet::new();
    let mut serializers = BTreeSet::new();
    for (row, serializer) in rows {
        if !super::super::is_scene_state(&row.object.class) {
            if row.object.class.ends_with("ParticipantStateEntityData") {
                return Err(bad("unsupported scene participant state class"));
            }
            continue;
        }
        let identity = Identity::from(&row.site);
        let serializer = serializer.ok_or_else(|| bad("scene state has no serializer"))?;
        if states.len() >= super::super::MAX_STATES
            || serializer == 0
            || serializer > u16::MAX as usize
            || !serializers.insert(serializer)
            || !identities.insert(identity.clone())
        {
            return Err(bad("scene state bound or duplicate identity"));
        }
        states.push(MemberState {
            identity,
            class: row.object.class.clone(),
            state: State {
                serializer,
                kind: StateKind::HostOwned,
                configuration: StateConfiguration::Default,
                handles_marked_for_delete: super::super::boolean(
                    &row.object,
                    "HandlesMarkedForDelete",
                )?,
                send_rpc_to_client: super::super::boolean(&row.object, "SendRpcToClientSide")?,
            },
        });
    }
    Ok(Membership {
        content_key,
        states,
    })
}

impl AssetIndex {
    /// Read the loaded scene's state membership contract using the canonical
    /// constructor walk. Specialized behavior stays with the scene service.
    pub fn participant_scene_membership(&mut self, scene: &str) -> Result<Membership, Error> {
        if scene.is_empty() || scene.len() > 512 {
            return Err(bad("scene membership name bound"));
        }
        let scene = scene.to_lowercase();
        let rules = self.identity.scenes.clone();
        let startup = self.startup_entries(&rules.level)?;
        let roots = scene_roots(&mut Graph::new(&mut self.assets), &rules.level, &startup)?;
        let root = roots
            .get(&scene)
            .ok_or_else(|| bad("membership scene is not loaded"))?
            .clone();
        let rows = Graph::new(&mut self.assets).read(root)?;
        let (_, occurrences) = crate::scenes::construct(&mut self.assets, &rules, &scene, true)?;
        if rows.len() != occurrences.len()
            || rows.iter().zip(&occurrences).any(|(row, occurrence)| {
                row.site.key != (occurrence.asset.clone(), occurrence.index)
            })
        {
            return Err(bad("scene membership constructor order"));
        }
        let key = if scene == rules.level.to_lowercase() {
            1
        } else {
            crate::assets::scene_key(&scene)
        };
        extract(
            key,
            rows.iter()
                .zip(occurrences)
                .map(|(row, occurrence)| (row, occurrence.serializer)),
        )
    }
}

#[cfg(test)]
mod tests;
