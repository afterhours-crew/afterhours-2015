// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bind eligibility facts to the exact live owner in the authored composition.
use super::*;
use crate::events::discovery::membership::{Fact, Resolved};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub fact: Fact,
    pub owner: Owner,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Owner {
    Ai {
        manager: SceneEndpoint,
    },
    Traffic {
        manager: Identity,
    },
    Cops {
        dispatch: Identity,
        roadblocks: Identity,
    },
}

const COP_SET: u32 = 0xddd7_2344;

fn cops<S: Source>(graph: &mut Graph<S>, set: &Site) -> Result<Owner, Error> {
    let branch = graph.read(Site {
        key: set.bus.clone(),
        ..set.clone()
    })?;
    let mut dispatch = Vec::new();
    let mut roadblocks = Vec::new();
    for node in branch {
        let owners = match node.object.class.as_str() {
            "CopDispatchManagerEntityData" => &mut dispatch,
            "RoadblockManagerEntityData" => &mut roadblocks,
            _ => continue,
        };
        let links = graph.links(&node.site, COP_SET)?;
        if links.iter().any(|end| end.site == *set && end.port == 0) {
            if links.len() != 1 {
                return Err(bad("AI scoring cop set owner is ambiguous"));
            }
            owners.push(Identity::from(&node.site));
        }
    }
    match (dispatch.as_slice(), roadblocks.as_slice()) {
        ([dispatch], [roadblocks]) => Ok(Owner::Cops {
            dispatch: dispatch.clone(),
            roadblocks: roadblocks.clone(),
        }),
        _ => Err(bad("AI scoring mutable set has no supported owner")),
    }
}

pub(super) fn bind<S: Source>(
    graph: &mut Graph<S>,
    resolved: &Resolved,
    endpoint: &mut impl FnMut(&mut Graph<S>, &Site) -> Result<SceneEndpoint, Error>,
) -> Result<Vec<Binding>, Error> {
    let mut result = Vec::new();
    for fact in resolved.program.facts().collect::<BTreeSet<_>>() {
        let (id, kind) = match fact {
            Fact::Ai(id) => (id, 0),
            Fact::Traffic(id) => (id, 1),
            Fact::Mutable(id) if !resolved.program.empty_sets().contains(id) => (id, 2),
            Fact::States(_) | Fact::Mutable(_) => continue,
            _ => return Err(bad("AI scoring membership owner unsupported")),
        };
        let site = resolved
            .sites
            .get(id)
            .ok_or_else(|| bad("AI scoring membership source"))?;
        let owner = match kind {
            0 => Owner::Ai {
                manager: endpoint(graph, site)?,
            },
            1 => Owner::Traffic {
                manager: id.clone(),
            },
            _ => cops(graph, site)?,
        };
        result.push(Binding {
            fact: fact.clone(),
            owner,
        });
    }
    Ok(result)
}

pub(super) fn validate(deployment: &Deployment) -> Result<(), Error> {
    let invalid = || bad("AI scoring membership bindings");
    if deployment.membership.len() > 256 {
        return Err(invalid());
    }
    let required: BTreeSet<_> = deployment
        .eligibility
        .facts()
        .filter(|fact| match fact {
            Fact::Ai(_) | Fact::Traffic(_) => true,
            Fact::Mutable(id) => !deployment.eligibility.empty_sets().contains(id),
            _ => false,
        })
        .collect();
    let mut found = BTreeSet::new();
    for binding in &deployment.membership {
        if !found.insert(&binding.fact) {
            return Err(invalid());
        }
        let valid = match (&binding.fact, &binding.owner) {
            (Fact::Ai(id), Owner::Ai { manager }) => identity_valid(id) && manager.scene_key != 0,
            (Fact::Traffic(id), Owner::Traffic { manager }) => identity_valid(id) && id == manager,
            (
                Fact::Mutable(id),
                Owner::Cops {
                    dispatch,
                    roadblocks,
                },
            ) => {
                [id, dispatch, roadblocks].into_iter().all(identity_valid)
                    && dispatch != roadblocks
                    && id != dispatch
                    && id != roadblocks
                    && dispatch.instances.starts_with(&id.instances)
                    && roadblocks.instances.starts_with(&id.instances)
            }
            _ => false,
        };
        if !valid {
            return Err(invalid());
        }
    }
    if found != required {
        return Err(invalid());
    }
    if deployment.eligibility.facts().any(|f| {
        !matches!(
            f,
            Fact::Ai(_) | Fact::Traffic(_) | Fact::Mutable(_) | Fact::States(_)
        )
    }) {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
