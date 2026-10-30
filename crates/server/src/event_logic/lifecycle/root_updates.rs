// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Root updates visit registered states, then registered transitions, then the
//! root's own state machine. Each callback completes before the next lookup.
//! The root state machine and the enclosing manager's cadence belong to the edge.
use super::{
    graphs::Step,
    links::{Endpoint, Object, Registry},
};
use nfs_world::scoring::{
    Error,
    events::native::{Call, Operation, Request},
    interpreter::hosted::Action,
    program::{Behavior, Program},
};
use std::collections::{BTreeMap, BTreeSet};

const STATES: u32 = 0x8fe7_512c;
const TRANSITIONS: u32 = 0xbb14_0b43;
const MAX_ROOTS: usize = 256;
const MAX_PENDING: usize = 4096;

#[derive(Clone, Debug)]
enum Work {
    States {
        root: Object,
        objects: Vec<Object>,
        index: usize,
    },
    Transitions {
        root: Object,
        objects: Vec<Object>,
        index: usize,
    },
}
#[derive(Clone, Debug)]
pub enum Resume {
    Steps(Vec<Step>),
    /// Only the root's state-machine update remains. The containing owner must
    /// delegate this to its edge rather than scheduling the registration lists again.
    Root(Request),
}
#[derive(Clone, Debug)]
pub struct RootUpdates {
    owner: u64,
    next: u64,
    roots: BTreeSet<Object>,
    pending: BTreeMap<u64, Work>,
}
impl RootUpdates {
    pub fn new(owner: u64) -> Result<Self, Error> {
        if owner == 0 {
            return Err(Error::InvalidGraph);
        }
        Ok(Self {
            owner,
            next: 0,
            roots: BTreeSet::new(),
            pending: BTreeMap::new(),
        })
    }
    pub fn install(
        &mut self,
        root: Object,
        program: &Program,
        links: &Registry,
    ) -> Result<(), Error> {
        program.validate()?;
        let node = program
            .nodes
            .get(usize::from(root.node))
            .ok_or(Error::UnknownNode)?;
        let installed = links.node(root)?;
        if installed.class != "GameModeEntityData"
            || node.behavior != Behavior::Native
            || !node.created
            || node.class != installed.class
            || node.identity.asset != installed.identity.asset
            || node.identity.index as usize != installed.identity.index
            || node
                .identity
                .instances
                .iter()
                .map(|(a, i)| (a.clone(), *i as usize))
                .collect::<Vec<_>>()
                != installed.identity.instances
            || self.roots.contains(&root)
        {
            return Err(Error::InvalidGraph);
        }
        if self.roots.len() >= MAX_ROOTS {
            return Err(Error::Bound);
        }
        // Resolve both lists before accepting ownership; preserve order/repeats.
        Self::objects(root, STATES, links)?;
        Self::objects(root, TRANSITIONS, links)?;
        self.roots.insert(root);
        Ok(())
    }
    fn objects(root: Object, port: u32, links: &Registry) -> Result<Vec<Object>, Error> {
        let objects = links.input(Endpoint { object: root, port })?;
        if objects.len() > 256 {
            return Err(Error::Bound);
        }
        if objects.contains(&root) {
            return Err(Error::InvalidGraph);
        }
        Ok(objects)
    }
    fn queue(&mut self, work: Work) -> Result<Step, Error> {
        if self.pending.len() >= MAX_PENDING {
            return Err(Error::Bound);
        }
        self.next = self.next.checked_add(1).ok_or(Error::Bound)?;
        self.pending.insert(self.next, work);
        Ok(Step::Continue {
            owner: self.owner,
            token: self.next,
        })
    }
    pub fn invoke(
        &mut self,
        request: &Request,
        links: &Registry,
    ) -> Result<Option<Vec<Step>>, Error> {
        let root = Object {
            instance: request.instance,
            node: request.node,
        };
        if request.call != Call::Update || !self.roots.contains(&root) {
            return Ok(None);
        }
        let objects = Self::objects(root, STATES, links)?;
        Ok(Some(vec![self.queue(Work::States {
            root,
            objects,
            index: 0,
        })?]))
    }
    pub fn resume(
        &mut self,
        owner: u64,
        token: u64,
        links: &Registry,
    ) -> Result<Option<Resume>, Error> {
        if owner != self.owner {
            return Ok(None);
        }
        let work = self.pending.remove(&token).ok_or(Error::InvalidGraph)?;
        let (root, objects, index, transitions) = match work {
            Work::States {
                root,
                objects,
                index,
            } => (root, objects, index, false),
            Work::Transitions {
                root,
                objects,
                index,
            } => (root, objects, index, true),
        };
        if let Some(object) = objects.get(index).copied() {
            let work = if transitions {
                Work::Transitions {
                    root,
                    objects,
                    index: index + 1,
                }
            } else {
                Work::States {
                    root,
                    objects,
                    index: index + 1,
                }
            };
            return Ok(Some(Resume::Steps(vec![
                Step::Graph {
                    id: object.instance,
                    action: Action::Graph(Operation::Update { node: object.node }),
                },
                self.queue(work)?,
            ])));
        }
        if transitions {
            Ok(Some(Resume::Root(Request {
                instance: root.instance,
                node: root.node,
                call: Call::Update,
            })))
        } else {
            let objects = Self::objects(root, TRANSITIONS, links)?;
            Ok(Some(Resume::Steps(vec![self.queue(
                Work::Transitions {
                    root,
                    objects,
                    index: 0,
                },
            )?])))
        }
    }
}

#[cfg(test)]
mod tests;
