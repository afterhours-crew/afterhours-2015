// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed score -> context -> owned participant / keeper-channel references.
//! Scene keeper resolution and mutation remain the enclosing runtime's job.
use super::{creation, events::Identity, field};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Endpoint, Graph, Site, Source, bad, bits32},
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::scoring::{events::MAX_NODES, score};

const KEEPER_TYPE: u32 = 1_060_499_882;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeeperChannel {
    pub identity: Identity,
    pub field: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Context {
    pub identity: Identity,
    pub participant_provider: Identity,
    /// Ordered occurrences, including duplicates. Resolve only through this
    /// participant instance's channel bindings, never a global asset lookup.
    pub keepers: Vec<KeeperChannel>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Award {
    pub identity: Identity,
    pub default: i32,
    pub context: Context,
}
pub fn compile(object: &Object) -> Result<i32, Error> {
    if object.class != "ScoreEntityData" {
        return Err(bad("score class"));
    }
    match field(&object.fields, "Score")? {
        Value::Int32(value) => Ok(*value),
        _ => Err(bad("score signed integer default")),
    }
}
fn one<S: Source>(graph: &mut Graph<S>, site: &Site, port: u32) -> Result<Endpoint, Error> {
    let mut rows = graph.links(site, port)?;
    if rows.len() != 1 || rows[0].port != 0 {
        return Err(bad("score singular object link"));
    }
    Ok(rows.remove(0))
}
fn keeper<S: Source>(graph: &mut Graph<S>, endpoint: Endpoint) -> Result<KeeperChannel, Error> {
    if endpoint.port == 0
        || graph.object(&endpoint.site)?.class != "LinkedSchematicChannelReferenceEntityData"
    {
        return Err(bad("score keeper channel endpoint"));
    }
    let key = graph.target_field(&endpoint.site, "ChannelAsset")?;
    let asset = graph.source.object(&key)?;
    if asset.class != "SchematicChannelAsset" {
        return Err(bad("score keeper channel asset"));
    }
    let rows = field(&asset.fields, "Links")?
        .as_array()
        .filter(|r| r.len() <= score::MAX_KEEPERS)
        .ok_or_else(|| bad("score keeper channel declaration bound"))?;
    let mut count = 0;
    for row in rows {
        let fields = row
            .as_fields()
            .ok_or_else(|| bad("score keeper channel declaration"))?;
        if bits32(field(fields, "Id")?)? != endpoint.port {
            continue;
        }
        count += 1;
        if bits32(field(fields, "LinkTypeHash")?)? != KEEPER_TYPE {
            return Err(bad("score keeper link type"));
        }
        if !matches!(field(fields,"Realm")?,Value::Enum {value:1,name:Some(n)} if n=="Realm_Server")
            && !matches!(field(fields,"Realm")?,Value::Enum {value:2,name:Some(n)} if n=="Realm_ClientAndServer")
        {
            return Err(bad("score keeper channel server realm"));
        }
    }
    if count != 1 {
        return Err(bad("score keeper declaration missing or ambiguous"));
    }
    Ok(KeeperChannel {
        identity: Identity::from(&endpoint.site),
        field: endpoint.port,
    })
}
fn read<S: Source>(
    source: &mut S,
    blueprint: &str,
    provider_root: u32,
) -> Result<Vec<Award>, Error> {
    if blueprint.is_empty() || blueprint.len() > 512 || provider_root == 0 {
        return Err(bad("score input bound"));
    }
    let mut graph = Graph::new(source);
    let nodes = graph.read(Site::root(blueprint))?;
    if !nodes.iter().any(|n| n.object.class == "ScoreEntityData") {
        return Ok(Vec::new());
    }
    let roots: Vec<_> = nodes
        .iter()
        .filter(|n| n.object.class == "ParticipantLogicEntityData")
        .collect();
    if roots.len() != 1 {
        return Err(bad("score participant root count"));
    }
    let in_graph = |site: &Site| nodes.iter().any(|n| n.site == *site);
    let mut result = vec![];
    for node in nodes.iter().filter(|n| n.object.class == "ScoreEntityData") {
        if result.len() >= MAX_NODES {
            return Err(bad("score node bound"));
        }
        if !creation::compile(&node.object)?.is_some_and(|r| r.includes_server()) {
            continue;
        }
        let context = one(&mut graph, &node.site, score::CONTEXT)?;
        let data = graph.object(&context.site)?;
        if !in_graph(&context.site)
            || data.class != "ScoreContextEntityData"
            || !creation::compile(&data)?.is_some_and(|r| r.includes_server())
        {
            return Err(bad("score context outside server composition"));
        }
        let provider = one(&mut graph, &context.site, score::PARTICIPANT)?;
        let data = graph.object(&provider.site)?;
        if !in_graph(&provider.site)
            || data.class != "ParticipantLogicParticipantProviderEntityData"
            || !creation::compile(&data)?.is_some_and(|r| r.includes_server())
        {
            return Err(bad("score participant provider"));
        }
        if one(&mut graph, &provider.site, provider_root)?.site != roots[0].site {
            return Err(bad("foreign score participant"));
        }
        let endpoints = graph.links(&context.site, score::KEEPERS)?;
        if endpoints.len() > score::MAX_KEEPERS {
            return Err(bad("score keeper bound"));
        }
        let mut keepers = vec![];
        for endpoint in endpoints {
            if !in_graph(&endpoint.site) {
                return Err(bad("score keeper outside composition"));
            }
            keepers.push(keeper(&mut graph, endpoint)?);
        }
        result.push(Award {
            identity: Identity::from(&node.site),
            default: compile(&node.object)?,
            context: Context {
                identity: Identity::from(&context.site),
                participant_provider: Identity::from(&provider.site),
                keepers,
            },
        });
    }
    Ok(result)
}
impl AssetIndex {
    /// Compile the supported own-participant/channel-provider subset. Validates
    /// declarations and identities, not activation, current scores or persistence.
    pub fn scoring_awards(&mut self, blueprint: &str) -> Result<Vec<Award>, Error> {
        let port = self.identity.participant_logic.boolean_ports["provider_root"];
        read(&mut self.assets, blueprint, port)
    }
}
#[cfg(test)]
mod tests;
