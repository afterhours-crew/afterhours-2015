// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Independent static products for rival routing and road-driven scoring.
//! Rebuild spatial indexes through ordinary validating constructors on every
//! load. No vehicle positions, routes or mutable driving state are serialized.
use super::Cache;
use nfs_content::{Error, assets::AssetIndex};
use nfs_world::{road_graph::Graph, scoring::road_network::Network};

mod codec;

fn decode_graph(bytes: &[u8]) -> Result<Graph, Error> {
    let input = codec::decode_graph(bytes)?;
    Graph::new(input.nodes, input.lanes).map_err(|_| Error::Cache("prepared road graph invariants"))
}

fn decode_network(bytes: &[u8]) -> Result<Network, Error> {
    let input = codec::decode_network(bytes)?;
    if input.iter().any(|segment| segment.speed_limit_mph > 300) {
        return Err(Error::Cache("prepared road speed limit"));
    }
    Network::new(input).map_err(|_| Error::Cache("prepared road network invariants"))
}

pub fn graph(index: &mut AssetIndex, cache: Option<&Cache>) -> Result<Graph, Error> {
    let Some(cache) = cache else {
        return index.road_graph();
    };
    cache.prepare("rival-road-graph", b"", decode_graph, || {
        codec::encode_graph(&index.road_graph_input()?)
    })
}

pub fn network(index: &mut AssetIndex, cache: Option<&Cache>) -> Result<Network, Error> {
    let Some(cache) = cache else {
        return index.road_network();
    };
    cache.prepare("scoring-road-network", b"", decode_network, || {
        codec::encode_network(&index.road_segments()?)
    })
}

#[cfg(test)]
mod tests;
