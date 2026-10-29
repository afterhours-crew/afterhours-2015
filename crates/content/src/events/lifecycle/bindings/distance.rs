// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Definition {
    pub identity: Identity,
    pub state: Endpoint,
    pub output: IntegerSource,
    /// The event owner supplies measurements from this exact route provider.
    pub route: Link,
}
pub(super) fn read<S: Source>(
    graph: &mut Graph<S>,
    node: &Node,
    index: &BTreeMap<Site, usize>,
    fields: &[FieldSite],
) -> Result<Definition, Error> {
    let states = endpoints(graph, &node.site, 0x979a_5954, index, fields)?;
    let [state] = states.as_slice() else {
        return Err(bad("distance reporter waypoint state"));
    };
    let output = integer_source(graph, &node.site, 0x25dc_65dd)?
        .ok_or_else(|| bad("distance reporter output"))?;
    let route = controller(graph, &node.site, 0xfd69_801e)?
        .ok_or_else(|| bad("distance reporter route"))?;
    if route.channel.is_none()
        && (route.port != 0 || route.class != "ChannelWaypointListProviderEntityData")
    {
        return Err(bad("distance reporter route class"));
    }
    Ok(Definition {
        identity: Identity::from(&node.site),
        state: *state,
        output,
        route,
    })
}
