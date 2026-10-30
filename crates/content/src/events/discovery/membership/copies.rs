// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A bounded, complete mirror controller. The copy, gated accessor and input
//! listener are different owners; neither initialization nor a copy aliases P.
use super::*;
use crate::scoring::{events as authored, nodes as scalar};
use nfs_world::scoring::{
    events::{Connection, Delivery, Endpoint as Event},
    nodes,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub input_owner: Identity,
    pub input: Program,
    pub comparison: Identity,
    pub comparison_config: scalar::Configuration,
    pub channel: Property,
}

fn site_at(root: &Site, identity: &Identity) -> Result<Site, Error> {
    if identity.instances
        != root
            .parents
            .iter()
            .map(|f| f.reference.clone())
            .collect::<Vec<_>>()
    {
        return Err(bad("copy control nested identity"));
    }
    Ok(Site {
        key: (identity.asset.clone(), identity.index),
        ..root.clone()
    })
}
fn unique(c: &authored::Composition, class: &str) -> Result<u16, Error> {
    let found: Vec<_> = c
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.class == class)
        .map(|(i, _)| i as u16)
        .collect();
    match found.as_slice() {
        [i] => Ok(*i),
        _ => Err(bad("copy control role count")),
    }
}
fn event_source(c: &authored::Composition, node: u16, event: u32) -> Result<Event, Error> {
    let sources: Vec<_> = c
        .connections
        .iter()
        .filter(|r| r.target == Event { node, event })
        .map(|r| r.source)
        .collect();
    match sources.as_slice() {
        [s] => Ok(*s),
        _ => Err(bad("copy control incoming event count")),
    }
}
fn edge(from: u16, out: u32, to: u16, event: u32) -> Connection {
    Connection {
        source: Event {
            node: from,
            event: out,
        },
        target: Event { node: to, event },
        delivery: Delivery::Local,
    }
}

fn controller<S: Source>(
    g: &mut Graph<S>,
    copy: &Site,
) -> Result<(Identity, scalar::Configuration, Property), Error> {
    let root = Site {
        key: copy.bus.clone(),
        ..copy.clone()
    };
    let c = authored::read_at(g.source, root.clone())?;
    if c.nodes.len() != 8 || c.buses.len() != 1 || !c.properties.is_empty() {
        return Err(bad("copy controller composition"));
    }
    let copy_id = unique(&c, "ParticipantSetCopyEntityData")?;
    if c.nodes[usize::from(copy_id)].identity != Identity::from(copy) {
        return Err(bad("copy controller owner"));
    }
    let interface = unique(&c, "InterfaceDescriptorData")?;
    let accessor = unique(&c, "ParticipantSetAccessorEntityData")?;
    let listener = unique(&c, "ParticipantSetListenerEntityData")?;
    let provider = unique(&c, "ParticipantEventToParticipantProviderEntityData")?;
    unique(&c, "LogicPrefabBlueprint")?;
    let add = event_source(&c, accessor, (-1_657_392_005i32) as u32)?;
    let remove = event_source(&c, accessor, (-772_017_604i32) as u32)?;
    if add.node == remove.node || add.event != nodes::OUT || remove.event != nodes::OUT {
        return Err(bad("copy controller accessor gates"));
    }
    for gate in [add.node, remove.node] {
        let n = &c.nodes[usize::from(gate)];
        let object = g.object(&site_at(&root, &n.identity)?)?;
        if n.class != "EventGateEntityData"
            || scalar::compile(&object)?.definition != (nodes::Definition::Gate { open: false })
            || !crate::scoring::creation::compile(&object)?.is_some_and(|r| r.includes_server())
        {
            return Err(bad("copy controller gate initialization or realm"));
        }
    }
    let enable = event_source(&c, copy_id, 0x7c81_3ec0)?;
    let disable = event_source(&c, copy_id, 0x0cac_9f9c)?;
    if enable.node != interface || disable.node != interface || enable.event == disable.event {
        return Err(bad("copy controller interface events"));
    }
    let expected = vec![
        edge(listener, (-414_835_110i32) as u32, add.node, nodes::IN),
        edge(add.node, nodes::OUT, provider, nodes::IN),
        edge(add.node, nodes::OUT, accessor, (-1_657_392_005i32) as u32),
        edge(listener, (-2_103_682_472i32) as u32, remove.node, nodes::IN),
        edge(remove.node, nodes::OUT, provider, nodes::IN),
        edge(remove.node, nodes::OUT, accessor, (-772_017_604i32) as u32),
        edge(interface, enable.event, copy_id, 0x7c81_3ec0),
        edge(interface, enable.event, remove.node, nodes::OPEN),
        edge(interface, enable.event, add.node, nodes::OPEN),
        edge(interface, disable.event, remove.node, nodes::CLOSE),
        edge(interface, disable.event, add.node, nodes::CLOSE),
        edge(interface, disable.event, copy_id, 0x0cac_9f9c),
    ];
    if c.connections != expected {
        return Err(bad("copy controller event order or extra writer"));
    }
    for node in [copy_id, add.node, remove.node, provider, accessor] {
        if !c.nodes[usize::from(node)]
            .listeners
            .is_some_and(|l| l.events.includes_server())
        {
            return Err(bad("copy controller event listener realm"));
        }
    }
    let links: BTreeSet<_> = c
        .links
        .iter()
        .map(|e| {
            (
                (e.source.node, e.source.field),
                (e.target.node, e.target.field),
            )
        })
        .collect();
    let input_port = c
        .links
        .iter()
        .find(|e| e.source.node == copy_id && e.source.field == name("ParticipantSet"))
        .filter(|e| e.target.node == interface)
        .map(|e| e.target.field)
        .ok_or_else(|| bad("copy controller input boundary"))?;
    let export_port = c
        .links
        .iter()
        .find(|e| e.source.node == interface && e.target.node == copy_id && e.target.field == 0)
        .map(|e| e.source.field)
        .ok_or_else(|| bad("copy controller output boundary"))?;
    let expected_links = BTreeSet::from([
        ((accessor, name("ParticipantSet")), (copy_id, 0)),
        ((accessor, (-1_232_400_965i32) as u32), (provider, 0)),
        ((listener, name("ParticipantSet")), (interface, input_port)),
        ((copy_id, name("ParticipantSet")), (interface, input_port)),
        ((interface, export_port), (copy_id, 0)),
    ]);
    if c.links.len() != 5
        || links != expected_links
        || !owned::known_consumers(
            g,
            copy,
            &BTreeSet::from([c.nodes[usize::from(accessor)].identity.clone()]),
        )?
    {
        return Err(bad("copy controller set links or external writer"));
    }
    let frame = copy
        .parents
        .last()
        .ok_or_else(|| bad("copy controller parent"))?;
    let mut reference = copy.clone();
    reference.key = frame.reference.clone();
    reference.bus = frame.bus.clone();
    reference.parents.pop();
    let enabled = g.event_sources(&reference, enable.event)?;
    let disabled = g.event_sources(&reference, disable.event)?;
    let ([(on, on_event)], [(off, off_event)]) = (enabled.as_slice(), disabled.as_slice()) else {
        return Err(bad("copy control comparison sources"));
    };
    if on != off || *on_event != nodes::ON_TRUE || *off_event != nodes::ON_FALSE {
        return Err(bad("copy control comparison outputs"));
    }
    let object = g.object(on)?;
    if object.class != "CompareBoolEntityData" {
        return Err(bad("copy control comparison class"));
    }
    constant::no_events(g, on)?;
    if g.input_ports(on)?.iter().any(|p| *p != nodes::BOOL) {
        return Err(bad("copy control unexpected comparison input"));
    }
    let config = scalar::compile(&object)?;
    if !crate::scoring::creation::compile(&object)?.is_some_and(|r| r.includes_server())
        || !crate::scoring::realms::Listeners::read(&object)?
            .is_some_and(|l| l.properties.includes_server())
    {
        return Err(bad("copy control comparison realm"));
    }
    let inputs = g.inputs(on, nodes::BOOL)?;
    let [input] = inputs.as_slice() else {
        return Err(bad("copy control comparison input"));
    };
    let channel = g.object(&input.site)?;
    if channel.class != "SchematicChannelEntityData" {
        return Err(bad("copy control channel class"));
    }
    let key = g.target_field(&input.site, "Channel")?;
    let schema = g.source.object(&key)?;
    if schema.class != "SchematicChannelAsset"
        || !crate::events::lifecycle::channels::property_ids(&channel, &schema, "OutputProperties")?
            .contains(&input.port)
    {
        return Err(bad("copy control channel output declaration"));
    }
    let property = Property {
        channel: key.0,
        port: input.port,
    };
    Ok((Identity::from(on), config, property))
}

pub(in crate::events::discovery) fn read<S: Source>(
    g: &mut Graph<S>,
    roots: &[Site],
    selected: &BTreeSet<Identity>,
    identified: &BTreeMap<Site, String>,
    endpoint: &mut impl FnMut(&mut Graph<S>, &Site) -> Result<Endpoint, Error>,
) -> Result<BTreeMap<Identity, Control>, Error> {
    if selected.is_empty() {
        return Ok(BTreeMap::new());
    }
    if selected.len() > MAX_EVENTS {
        return Err(bad("copy control owner bound"));
    }
    let mut result = BTreeMap::new();
    for root in roots {
        for node in g.read(root.clone())? {
            let identity = Identity::from(&node.site);
            if !selected.contains(&identity) || result.contains_key(&identity) {
                continue;
            }
            if node.object.class != "ParticipantSetCopyEntityData" {
                return Err(bad("copy state owner class"));
            }
            let input = link(g, &node.site, name("ParticipantSet"))?;
            let input_owner = Identity::from(&input);
            let input = read_set(g, input, identified, endpoint)?;
            if input
                .facts()
                .any(|f| !matches!(f, Fact::Player(_) | Fact::States(_)))
            {
                return Err(bad("copy source needs another runtime owner"));
            }
            let (comparison, comparison_config, channel) = controller(g, &node.site)?;
            result.insert(
                identity,
                Control {
                    input_owner,
                    input,
                    comparison,
                    comparison_config,
                    channel,
                },
            );
        }
    }
    if result.len() != selected.len() {
        return Err(bad("copy owner absent from loaded scenes"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
