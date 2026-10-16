// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Property binding topology for the installed local connection subset.
//! This does not choose value types, construction order or node activation.
use super::{
    events::{Composition, DataConnection},
    interfaces::{Access, Interface},
    realms::Realm,
};
use crate::{Error, graph::bad};
use nfs_world::scoring::{
    events::MAX_CONNECTIONS,
    properties::{MAX_PROPERTIES, Port},
};
use std::collections::{BTreeMap, BTreeSet};

/// A producer and a multiple-producer input are different storage locations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Source {
    Output(Port),
    /// The receiving port uniquely identifies a merge within this composition.
    Merge(Port),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Input {
    pub target: Port,
    /// Preserved from the first row, separately from connection realm flags.
    pub mode: u8,
    /// Authored order; repeated producer rows remain significant.
    pub producers: Vec<Port>,
}
impl Input {
    pub fn source(&self) -> Source {
        if self.producers.len() == 1 {
            Source::Output(self.producers[0])
        } else {
            Source::Merge(self.target)
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputAlias {
    /// Output cell in the parent bus, owned by its prefab reference.
    pub parent: Port,
    /// Producer in this bus which uses the parent output cell.
    pub child: Port,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bus {
    pub index: u16,
    pub interface: Option<u16>,
    pub parent_reference: Option<u16>,
    pub inputs: Vec<Input>,
    /// Authored boundary order, followed by implicit blueprint outputs.
    pub outputs: Vec<OutputAlias>,
    /// Source metadata after local realm filtering and merge construction.
    /// The low three bits are the combined consumer realm; bit 3 is retained.
    /// Ordering here is stable model identity, not native pointer order.
    pub sources: BTreeMap<Source, u32>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    /// Parent buses precede their children. Node IDs come from Composition.
    pub buses: Vec<Bus>,
    pub excluded_rows: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    /// Distinct producer cells forwarded into this merge, in authored order.
    /// Empty for an ordinary output. This is not listener registration order.
    pub producers: Vec<usize>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    pub plan: Plan,
    /// Per-session storage identities, with no initial value or guessed type.
    pub cells: Vec<Cell>,
    /// Source and target namespaces remain separate, including prefab ports.
    pub outputs: BTreeMap<Port, usize>,
    pub inputs: BTreeMap<Port, usize>,
}

fn local_flags(flags: Option<u32>) -> Result<u32, Error> {
    let flags = flags.ok_or_else(|| bad("property flags missing"))?;
    // Only local client/both/server rows with modes 0/1 occur in this subset.
    // Networked delivery, reference binding and other mode bits need contracts.
    if !matches!(flags & 7, 1..=3) || flags & !0x13 != 0 {
        return Err(bad("unsupported property connection flags"));
    }
    Ok(flags)
}
fn includes(flags: u32, realm: Realm) -> bool {
    !matches!((flags & 7, realm), (2, Realm::Server) | (3, Realm::Client))
}
fn combine(previous: u32, next: u32) -> u32 {
    let a = previous & 7;
    let b = next & 7;
    let realm = if a == 0 {
        b
    } else if a == b {
        a
    } else {
        1
    };
    ((previous | next) & 8) | realm
}

impl Composition {
    /// Resolve storage identity across prefab boundaries without executing any
    /// default, forwarding callback or node activation. Shared cells are aliases;
    /// multiple producers always retain separate cells and a distinct merge.
    pub fn property_layout(&self, interfaces: &[Interface], realm: Realm) -> Result<Layout, Error> {
        let plan = self.property_plan(interfaces, realm)?;
        resolve(plan)
    }
    /// Compile the installed local, at-most-two-producer topology. Interfaces
    /// must be from this same composition. This is not an executable Store:
    /// aliases, forwarding, defaults and activation still need ordered binding.
    pub fn property_plan(&self, interfaces: &[Interface], realm: Realm) -> Result<Plan, Error> {
        if !matches!(realm, Realm::Client | Realm::Server) {
            return Err(bad("property plan requires one execution realm"));
        }
        if self.nodes.is_empty()
            || self.nodes.len() > nfs_world::scoring::events::MAX_NODES
            || self.buses.is_empty()
            || self.buses.len() > self.nodes.len()
            || self.properties.len() > MAX_CONNECTIONS
            || interfaces.len() > self.buses.len()
        {
            return Err(bad("property plan composition bound"));
        }
        let mut rows = vec![Vec::new(); self.buses.len()];
        for row in &self.properties {
            let Some(bus) = self.buses.get(usize::from(row.bus)) else {
                return Err(bad("property row bus"));
            };
            for port in [row.source, row.target] {
                let node = self
                    .nodes
                    .get(usize::from(port.node))
                    .ok_or_else(|| bad("property row node"))?;
                if node.identity.asset != bus.asset || node.identity.instances != bus.instances {
                    return Err(bad("property endpoint outside bus"));
                }
            }
            let grouped = &mut rows[usize::from(row.bus)];
            if usize::from(row.index) != grouped.len() {
                return Err(bad("property row authored order"));
            }
            local_flags(row.flags)?;
            grouped.push(*row);
        }
        let mut interface_nodes = BTreeSet::new();
        let mut definitions = BTreeMap::new();
        let mut declared_fields = 0usize;
        for interface in interfaces {
            let bus = self
                .buses
                .iter()
                .position(|b| *b == interface.bus)
                .ok_or_else(|| bad("property interface bus"))?;
            let node = self
                .nodes
                .iter()
                .position(|n| {
                    n.identity == interface.identity && n.class == "InterfaceDescriptorData"
                })
                .ok_or_else(|| bad("property interface identity"))? as u16;
            if !interface_nodes.insert(node)
                || definitions.insert(bus, (node, interface)).is_some()
                || interface.identity.asset != interface.bus.asset
                || interface.identity.instances != interface.bus.instances
            {
                return Err(bad("duplicate or foreign property interface"));
            }
            let mut fields = BTreeSet::new();
            declared_fields = declared_fields
                .checked_add(interface.fields.len())
                .filter(|&n| n <= MAX_PROPERTIES)
                .ok_or_else(|| bad("property declaration bound"))?;
            if interface.fields.iter().any(|f| !fields.insert(f.id)) {
                return Err(bad("property interface field bound or duplicate"));
            }
        }
        if self.nodes.iter().enumerate().any(|(i, n)| {
            n.class == "InterfaceDescriptorData" && !interface_nodes.contains(&(i as u16))
        }) {
            return Err(bad("property interface declaration missing"));
        }
        let mut plan = Plan {
            buses: Vec::new(),
            excluded_rows: 0,
        };
        let mut total_sources = 0usize;
        for (index, authored) in rows.iter().enumerate() {
            let identity = &self.buses[index];
            let blueprint = self
                .nodes
                .iter()
                .position(|n| n.identity == *identity)
                .ok_or_else(|| bad("property blueprint identity"))?;
            if self.buses[..index].contains(identity) {
                return Err(bad("duplicate property bus"));
            }
            let definition = definitions.get(&index).copied();
            let interface = definition.map(|(node, _)| node);
            let parent_reference = match interface {
                Some(node) => self.nodes[usize::from(node)].forward,
                None => None,
            };
            let parent = if let Some(reference) = parent_reference {
                let node = self
                    .nodes
                    .get(usize::from(reference))
                    .ok_or_else(|| bad("property parent reference"))?;
                if node.forward != interface
                    || node.class != "LogicPrefabReferenceObjectData"
                    || self.nodes[blueprint].class != "LogicPrefabBlueprint"
                {
                    return Err(bad("unsupported property prefab boundary"));
                }
                let parent_index = self.buses[..index]
                    .iter()
                    .position(|b| {
                        b.asset == node.identity.asset && b.instances == node.identity.instances
                    })
                    .ok_or_else(|| bad("property parent must precede child"))?;
                let mut path = node.identity.instances.clone();
                path.push((node.identity.asset.clone(), node.identity.index));
                if path != identity.instances {
                    return Err(bad("property prefab instance path"));
                }
                Some(&plan.buses[parent_index])
            } else {
                if !identity.instances.is_empty() {
                    return Err(bad("nested property bus needs parent interface"));
                }
                None
            };
            let mut bus = Bus {
                index: index as u16,
                interface,
                parent_reference,
                inputs: Vec::new(),
                outputs: Vec::new(),
                sources: BTreeMap::new(),
            };
            let mut records = Vec::<(Source, u32)>::new();
            for row in authored {
                if Some(row.target.node) == interface {
                    let Some((parent, reference)) = parent.zip(parent_reference) else {
                        plan.excluded_rows += 1;
                        continue;
                    };
                    let output = Port {
                        node: reference,
                        field: row.target.field,
                    };
                    let Some(&flags) = parent.sources.get(&Source::Output(output)) else {
                        plan.excluded_rows += 1;
                        continue;
                    };
                    if !includes(flags, realm) {
                        plan.excluded_rows += 1;
                        continue;
                    }
                    // The native interface-to-interface bridge has separate
                    // forwarding/copy initialization. Do not alias its ends.
                    if Some(row.source.node) == interface {
                        return Err(bad("unsupported property interface bridge"));
                    }
                    bus.outputs.push(OutputAlias {
                        parent: output,
                        child: row.source,
                    });
                    bus.sources
                        .entry(Source::Output(row.source))
                        .or_insert(flags);
                } else if includes(row.flags.unwrap_or_default(), realm) {
                    add_input(&mut bus, &mut records, *row)?;
                } else {
                    plan.excluded_rows += 1;
                }
            }
            if let (Some(parent), Some(reference), Some((_, descriptor))) =
                (parent, parent_reference, definition)
            {
                // Prefab blueprints expose fields not declared as interface
                // Sources through the blueprint object itself, not its interface.
                let mut outputs: Vec<_> = parent
                    .sources
                    .keys()
                    .filter_map(|source| match source {
                        Source::Output(port) if port.node == reference => Some(*port),
                        _ => None,
                    })
                    .collect();
                outputs.sort_by_key(|p| p.field as i32);
                for output in outputs {
                    if descriptor
                        .fields
                        .iter()
                        .any(|f| f.id == output.field && f.access == Access::Source)
                    {
                        continue;
                    }
                    let child = Port {
                        node: blueprint as u16,
                        field: output.field,
                    };
                    bus.outputs.push(OutputAlias {
                        parent: output,
                        child,
                    });
                    bus.sources.entry(Source::Output(child)).or_insert(0);
                }
            }
            for (source, flags) in records {
                let prior = bus.sources.entry(source).or_default();
                *prior = combine(*prior, flags);
            }
            total_sources = total_sources
                .checked_add(bus.sources.len())
                .filter(|&n| n <= MAX_PROPERTIES)
                .ok_or_else(|| bad("property source bound"))?;
            plan.buses.push(bus);
        }
        Ok(plan)
    }
}

fn resolve(plan: Plan) -> Result<Layout, Error> {
    let mut cells = Vec::<Cell>::new();
    let mut outputs = BTreeMap::<Port, usize>::new();
    let mut inputs = BTreeMap::<Port, usize>::new();
    for bus in &plan.buses {
        let mut local = BTreeMap::new();
        // Parent outputs exist before child construction. Native assignment is
        // last-wins, but conflicting aliases need a separately verified model;
        // reject them instead of silently losing an externally visible output.
        for alias in &bus.outputs {
            let cell = *outputs
                .get(&alias.parent)
                .ok_or_else(|| bad("parent output cell missing"))?;
            if local
                .insert(Source::Output(alias.child), cell)
                .is_some_and(|old| old != cell)
            {
                return Err(bad("unsupported conflicting prefab output aliases"));
            }
        }
        // Interface inputs reuse the parent's resolved target (which can be a
        // merge). Ordinary output aliases above have construction precedence.
        for &source in bus.sources.keys() {
            if let Source::Output(port) = source
                && Some(port.node) == bus.interface
                && let Some(reference) = bus.parent_reference
                && let Some(&cell) = inputs.get(&Port {
                    node: reference,
                    field: port.field,
                })
            {
                local.entry(source).or_insert(cell);
            }
        }
        for &source in bus.sources.keys() {
            if let std::collections::btree_map::Entry::Vacant(entry) = local.entry(source) {
                if cells.len() >= MAX_PROPERTIES {
                    return Err(bad("resolved property cell bound"));
                }
                entry.insert(cells.len());
                cells.push(Cell {
                    producers: Vec::new(),
                });
            }
        }
        for (&source, &cell) in &local {
            if let Source::Output(port) = source
                && outputs.insert(port, cell).is_some()
            {
                return Err(bad("property output appears in multiple buses"));
            }
        }
        for input in &bus.inputs {
            let cell = *local
                .get(&input.source())
                .ok_or_else(|| bad("resolved source missing"))?;
            if inputs.insert(input.target, cell).is_some() {
                return Err(bad("property target appears in multiple buses"));
            }
            if input.producers.len() > 1 {
                let producers = input
                    .producers
                    .iter()
                    .map(|p| {
                        local
                            .get(&Source::Output(*p))
                            .copied()
                            .ok_or_else(|| bad("merge producer missing"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if producers.contains(&cell) || !cells[cell].producers.is_empty() {
                    return Err(bad("invalid property merge identity"));
                }
                cells[cell].producers = producers;
            }
        }
    }
    Ok(Layout {
        plan,
        cells,
        outputs,
        inputs,
    })
}

fn add_input(
    bus: &mut Bus,
    records: &mut Vec<(Source, u32)>,
    row: DataConnection,
) -> Result<(), Error> {
    if let Some(input) = bus.inputs.iter_mut().find(|i| i.target == row.target) {
        if input.producers.len() != 1 {
            return Err(bad("unsupported property fan-in beyond two producers"));
        }
        let first = Source::Output(input.producers[0]);
        // Replace the first retained source entry with the merge output while
        // keeping that entry's flags. Add the distinct producer cells neutrally.
        let record = records
            .iter_mut()
            .find(|(source, _)| *source == first)
            .ok_or_else(|| bad("property merge source missing"))?;
        record.0 = Source::Merge(input.target);
        records.push((first, 0));
        records.push((Source::Output(row.source), 0));
        input.producers.push(row.source);
    } else {
        bus.inputs.push(Input {
            target: row.target,
            mode: ((row.flags.unwrap_or_default() >> 4) & 3) as u8,
            producers: vec![row.source],
        });
        records.push((Source::Output(row.source), row.flags.unwrap_or_default()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
