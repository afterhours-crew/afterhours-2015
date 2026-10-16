// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Type equality across aliases and merge forwarding, without value aliasing.
use super::Layout;
use crate::{Error, graph::bad};
use nfs_world::scoring::properties::{Definition, Kind, MAX_PORTS, MAX_PROPERTIES, Port};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endpoint {
    Input(Port),
    Output(Port),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Constraint {
    pub endpoint: Endpoint,
    pub kind: Kind,
}

impl Layout {
    /// Resolve only types supported by explicit endpoint declarations. Unknown
    /// components remain None. A merge and its producers require the same type,
    /// but remain separate cells. No default or initialization is inferred.
    pub fn property_types(&self, constraints: &[Constraint]) -> Result<Vec<Option<Kind>>, Error> {
        if self.cells.len() > MAX_PROPERTIES
            || constraints.len() > MAX_PORTS
            || self.inputs.len().saturating_add(self.outputs.len()) > MAX_PORTS
        {
            return Err(bad("property type bound"));
        }
        let mut represented = vec![false; self.cells.len()];
        for &cell in self.inputs.values().chain(self.outputs.values()) {
            *represented
                .get_mut(cell)
                .ok_or_else(|| bad("property cell index"))? = true;
        }
        if represented.contains(&false) {
            return Err(bad("property cell without endpoint"));
        }
        let mut neighbours = vec![Vec::new(); self.cells.len()];
        for (cell, definition) in self.cells.iter().enumerate() {
            if !definition.producers.is_empty() && definition.producers.len() != 2 {
                return Err(bad("unsupported typed merge shape"));
            }
            for &producer in &definition.producers {
                if producer >= self.cells.len() || producer == cell {
                    return Err(bad("typed merge producer index"));
                }
                neighbours[cell].push(producer);
                neighbours[producer].push(cell);
            }
        }
        let mut kinds = vec![None; self.cells.len()];
        let mut pending = Vec::new();
        for constraint in constraints {
            let (ports, port) = match constraint.endpoint {
                Endpoint::Input(port) => (&self.inputs, port),
                Endpoint::Output(port) => (&self.outputs, port),
            };
            let &cell = ports
                .get(&port)
                .ok_or_else(|| bad("type constraint endpoint missing"))?;
            assign(&mut kinds, &mut pending, cell, constraint.kind)?;
        }
        while let Some(cell) = pending.pop() {
            let kind = kinds[cell].ok_or_else(|| bad("property type worklist"))?;
            for &next in &neighbours[cell] {
                assign(&mut kinds, &mut pending, next, kind)?;
            }
        }
        Ok(kinds)
    }

    /// Allocate only fully typed, unready cells. No listeners, forwarding,
    /// defaults, private unconsumed outputs or activation are synthesized.
    /// The caller must perform those in a separately established order.
    pub fn unready_definitions(
        &self,
        constraints: &[Constraint],
    ) -> Result<Vec<Definition>, Error> {
        let kinds = self.property_types(constraints)?;
        let mut definitions = kinds
            .into_iter()
            .map(|kind| {
                Ok(Definition {
                    kind: kind.ok_or_else(|| bad("unresolved property type"))?,
                    initial: None,
                    ports: Vec::new(),
                    listeners: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut identities = BTreeMap::new();
        for (&port, &cell) in self.outputs.iter().chain(&self.inputs) {
            match identities.insert(port, cell) {
                Some(previous) if previous != cell => {
                    return Err(bad("distinct input/output cells share one store port"));
                }
                Some(_) => {}
                None => definitions[cell].ports.push(port),
            }
        }
        Ok(definitions)
    }
}

fn assign(
    kinds: &mut [Option<Kind>],
    pending: &mut Vec<usize>,
    cell: usize,
    kind: Kind,
) -> Result<(), Error> {
    match kinds[cell] {
        Some(previous) if previous != kind => Err(bad("conflicting property types")),
        Some(_) => Ok(()),
        None => {
            kinds[cell] = Some(kind);
            pending.push(cell);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
