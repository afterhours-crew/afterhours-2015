// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant pacing's own provider and four separately bound services.
use super::{events::Identity, participant_properties::identifier};
use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};
use nfs_frostbite::ebx::Object;
use nfs_world::scoring::participant_pacing::{Library, MAX_PERSONALITIES, Sources};

pub const CLASS: &str = "ParticipantPacingAccessorEntityData";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub provider: Identity,
    pub sources: Sources,
}

pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut result = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class != CLASS {
            continue;
        }
        let providers = graph.links(&node.site, 0xb68b_0dbb)?;
        let [provider] = providers.as_slice() else {
            return Err(bad("pacing participant provider"));
        };
        if provider.port != 0
            || graph.object(&provider.site)?.class
                != "ParticipantLogicParticipantProviderEntityData"
        {
            return Err(bad("pacing own participant provider"));
        }
        result.push(Node {
            identity: Identity::from(&node.site),
            provider: Identity::from(&provider.site),
            sources: Sources {
                pacing: identifier(&mut graph, &node.site, 0x7eef_cb0c)?,
                actors: identifier(&mut graph, &node.site, 0x2824_4ad5)?,
                personalities: identifier(&mut graph, &node.site, 0xb57a_a3e6)?,
                curves: identifier(&mut graph, &node.site, 0xa663_9088)?,
            },
        });
    }
    Ok(result)
}

/// The caller resolves library links and preserves their personality order.
/// Empty authored curves have distinct fallback semantics from malformed data.
pub fn library(personalities: &[Object], fallback: Option<&Object>) -> Result<Library, Error> {
    if personalities.len() > MAX_PERSONALITIES {
        return Err(bad("pacing personality bound"));
    }
    let curve = |object: &Object, class: &str| {
        if object.class != class {
            return Err(bad("pacing library class"));
        }
        let value = object
            .fields
            .get("NOSRebuildPacingCurve")
            .ok_or_else(|| bad("pacing curve missing"))?;
        if value.as_array().is_some_and(|v| v.is_empty()) {
            Ok(None)
        } else {
            super::compile_curve_rows(value, 1., 1.).map(Some)
        }
    };
    Ok(Library {
        personalities: personalities
            .iter()
            .map(|v| curve(v, "PersonalityEntityData"))
            .collect::<Result<_, _>>()?,
        fallback: fallback
            .map(|v| curve(v, "PacingLibraryEntityData"))
            .transpose()?
            .flatten(),
    })
}

#[cfg(test)]
mod tests;
