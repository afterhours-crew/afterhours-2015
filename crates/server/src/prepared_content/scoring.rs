// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Static free-roam programs and scene bindings. Interpreter sessions, inputs,
//! awards, clocks and account progress are constructed separately as before.
use super::Cache;
use nfs_content::{
    Error,
    assets::AssetIndex,
    participant_logic::Owner,
    scoring::content::{ACQUISITION, SCORING, ScoringContent},
};
use std::{collections::BTreeSet, sync::Arc};

fn validate(content: &ScoringContent) -> Result<(), Error> {
    let invalid = || Error::Cache("prepared free-roam scoring invariants");
    if content.categories.len() != 6
        || content.categories.values().collect::<BTreeSet<_>>().len() != 6
    {
        return Err(invalid());
    }
    for (part, blueprint) in [
        (&content.acquisition, ACQUISITION),
        (&content.scoring, SCORING),
    ] {
        part.program.validate().map_err(|_| invalid())?;
        if part.blueprint != blueprint
            || part.layout.owner != Owner::Scene
            || part.fields.len() != part.layout.fields.len()
            || part.bus_entities.len() > usize::from(u16::MAX)
            || part.fields.windows(2).any(|v| v[0].node >= v[1].node)
        {
            return Err(invalid());
        }
        for (ordinal, (field, layout)) in part.fields.iter().zip(&part.layout.fields).enumerate() {
            if field.serializer != ordinal
                || field.kind != layout.kind
                || usize::from(field.node) >= part.program.nodes.len()
            {
                return Err(invalid());
            }
        }
    }
    // Rebuild and validate both serializer profiles and their shared catalog.
    crate::scoring::Content::new(Arc::new(content.clone())).map_err(|_| invalid())?;
    Ok(())
}

fn decode(bytes: &[u8]) -> Result<ScoringContent, Error> {
    if bytes.len() > nfs_content::prepared_cache::MAX_PAYLOAD {
        return Err(Error::Cache("prepared scoring size"));
    }
    let content =
        serde_json::from_slice(bytes).map_err(|_| Error::Cache("prepared scoring format"))?;
    validate(&content)?;
    Ok(content)
}

pub fn prepare(
    index: &mut AssetIndex,
    scene: &str,
    cache: Option<&Cache>,
) -> Result<ScoringContent, Error> {
    let Some(cache) = cache else {
        return index.scoring_content(scene);
    };
    cache.prepare("free-roam-scoring", scene.as_bytes(), decode, || {
        tracing::info!("compiling free-roam scoring cache");
        serde_json::to_vec(&index.scoring_content(scene)?)
            .map_err(|_| Error::Cache("prepared scoring serialization"))
    })
}

#[cfg(test)]
mod tests;
