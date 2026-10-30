// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Integer mirror identifiers are assets with HashedId, not channel port names.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
pub enum Manager {
    Local(Identity),
    Channel(Property),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Mirror {
    pub identity: Identity,
    pub identifier: u32,
    pub manager: Option<Manager>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Provider {
    pub identity: Identity,
    pub identifier: u32,
    pub manager: Option<Manager>,
    pub source: Option<IntegerSource>,
}

pub(super) fn read<S: Source>(graph: &mut Graph<S>, node: &Node) -> Result<Mirror, Error> {
    let key = graph.target_field(&node.site, "Identifier")?;
    let asset = graph.source.object(&key)?;
    if asset.class != "IntParticipantPropertyIdentifier" {
        return Err(bad("integer mirror identifier class"));
    }
    let identifier = bits32(
        asset
            .fields
            .get("HashedId")
            .ok_or_else(|| bad("integer mirror hash"))?,
    )?;
    let manager = match controller(graph, &node.site, 0xc286_a9af)? {
        None => None,
        Some(link) => Some(if let Some(channel) = link.channel {
            Manager::Channel(channel)
        } else if link.port == 0 && link.class == "MirroringManagerEntityData" {
            Manager::Local(link.identity)
        } else {
            return Err(bad("integer mirror manager class"));
        }),
    };
    Ok(Mirror {
        identity: Identity::from(&node.site),
        identifier,
        manager,
    })
}

pub(super) fn provider<S: Source>(graph: &mut Graph<S>, node: &Node) -> Result<Provider, Error> {
    let mirror = read(graph, node)?;
    Ok(Provider {
        identity: mirror.identity,
        identifier: mirror.identifier,
        manager: mirror.manager,
        source: integer_source(graph, &node.site, 0x0df2_30d5)?,
    })
}
