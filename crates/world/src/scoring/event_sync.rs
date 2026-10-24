// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Route registry-82 acknowledgements to one owned graph instance. The wire
//! event identifies EventSync data (Flags low 25 bits), not an output port.
//! Native receiver 0397d600 emits Out with optional connection-local context.
use super::{
    Error,
    events::{EntityContext, MAX_NODES},
};
use crate::logic::Message;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    pub entity: u32,
    pub event: u32,
    pub node: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Routed {
    pub node: u16,
    pub context: Option<EntityContext>,
}
#[derive(Clone, Debug)]
pub struct Router {
    ghost: u16,
    nodes: BTreeMap<(u32, u32), u16>,
}
impl Router {
    pub fn new(ghost: u16, bindings: &[Binding]) -> Result<Self, Error> {
        if ghost == 0 || ghost > 8191 || bindings.len() > MAX_NODES {
            return Err(Error::Bound);
        }
        let mut nodes = BTreeMap::new();
        for binding in bindings {
            if binding.entity == 0
                || binding.entity > 0x7ff_ffff
                || binding.event > 0x1ff_ffff
                || usize::from(binding.node) >= MAX_NODES
            {
                return Err(Error::Bound);
            }
            if nodes
                .insert((binding.entity, binding.event), binding.node)
                .is_some()
            {
                return Err(Error::InvalidGraph);
            }
        }
        Ok(Self { ghost, nodes })
    }
    /// `players` is the sending connection's validated local-player table.
    /// Unresolved identities and foreign local slots have no graph effects.
    /// Repeated acknowledgements remain events; lifecycle code owns idempotence.
    pub fn route(&self, message: &Message, players: &[Option<u16>; 8]) -> Option<Routed> {
        let Message::Reached {
            event,
            target,
            player,
        } = message
        else {
            return None;
        };
        if target.ghost != self.ghost {
            return None;
        }
        let &node = self.nodes.get(&(target.entity, *event))?;
        let context = if *player == 255 {
            None
        } else {
            let object = players.get(usize::from(*player)).copied().flatten()?;
            if object == 0 || object > 8191 {
                return None;
            }
            Some(EntityContext {
                object: Some(object),
            })
        };
        Some(Routed { node, context })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::EntityRef;
    #[test]
    fn identity_and_connection_local_context_are_required_without_deduplicating_events() {
        let bindings = [
            Binding {
                entity: 2,
                event: 19,
                node: 5,
            },
            Binding {
                entity: 3,
                event: 19,
                node: 6,
            },
        ];
        let router = Router::new(41, &bindings).unwrap();
        let other = Router::new(42, &bindings).unwrap();
        let mut players = [None; 8];
        players[0] = Some(7);
        let message = |ghost, entity, event, player| Message::Reached {
            event,
            target: EntityRef { ghost, entity },
            player,
        };
        let expected = Some(Routed {
            node: 5,
            context: Some(EntityContext { object: Some(7) }),
        });
        for _ in 0..2 {
            assert_eq!(router.route(&message(41, 2, 19, 0), &players), expected);
        }
        assert_eq!(other.route(&message(41, 2, 19, 0), &players), None);
        assert_eq!(
            router.route(&message(41, 3, 19, 255), &players),
            Some(Routed {
                node: 6,
                context: None
            })
        );
        for m in [
            message(41, 1, 19, 0),
            message(41, 2, 20, 0),
            message(41, 2, 19, 1),
            message(41, 2, 19, 8),
        ] {
            assert_eq!(router.route(&m, &players), None);
        }
        assert!(Router::new(41, &[bindings[0]; 2]).is_err());
        assert!(Router::new(0, &bindings).is_err());
    }
}
