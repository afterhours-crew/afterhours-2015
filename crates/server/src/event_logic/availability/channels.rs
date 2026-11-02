// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Exact loaded scalar publishers and their later event writes. Channel output
//! is the last publication, not the OR of Boolean publisher values.
use nfs_content::{
    events::discovery::channels::{Publisher, Signal},
    scoring::events::Identity,
};
use nfs_world::scoring::{
    Error,
    math::Value,
    nodes::{self, Action, Effect, Node},
    participant_properties::Property,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct State {
    nodes: BTreeMap<Identity, Node>,
    outputs: BTreeMap<Identity, bool>,
    destinations: BTreeMap<Identity, BTreeSet<Property>>,
    writes: BTreeMap<Signal, (Identity, u32)>,
    values: BTreeMap<Property, bool>,
}
impl State {
    pub fn new(definitions: &BTreeMap<Property, Vec<Publisher>>) -> Result<Self, Error> {
        if definitions.len() > 16 {
            return Err(Error::Bound);
        }
        let mut result = Self {
            nodes: BTreeMap::new(),
            outputs: BTreeMap::new(),
            destinations: BTreeMap::new(),
            writes: BTreeMap::new(),
            values: BTreeMap::new(),
        };
        let mut configurations = BTreeMap::new();
        for (property, publishers) in definitions {
            property.validate()?;
            if publishers.is_empty() || publishers.len() > 64 {
                return Err(Error::InvalidGraph);
            }
            let mut channels = BTreeSet::new();
            for publisher in publishers {
                if !channels.insert(&publisher.channel)
                    || !publisher.configuration.defaults.is_empty()
                    || publisher.writes.len() > 64
                {
                    return Err(Error::InvalidGraph);
                }
                let nodes::Definition::Boolean { default } = publisher.configuration.definition
                else {
                    return Err(Error::InvalidGraph);
                };
                // Cross-scene activation order is not encoded by this definition.
                // Equal initialized inputs establish a value without guessing it.
                if result
                    .values
                    .insert(property.clone(), default)
                    .is_some_and(|old| old != default)
                {
                    return Err(Error::InvalidGraph);
                }
                if configurations
                    .insert(publisher.input.clone(), publisher.configuration.clone())
                    .is_some_and(|old| old != publisher.configuration)
                {
                    return Err(Error::InvalidGraph);
                }
                result
                    .nodes
                    .entry(publisher.input.clone())
                    .or_insert(Node::new(publisher.configuration.definition)?);
                result.outputs.insert(publisher.input.clone(), default);
                result
                    .destinations
                    .entry(publisher.input.clone())
                    .or_default()
                    .insert(property.clone());
                for (signal, command) in &publisher.writes {
                    if !matches!(*command, nodes::SET_TRUE | nodes::SET_FALSE | nodes::TOGGLE) {
                        return Err(Error::InvalidGraph);
                    }
                    let target = (publisher.input.clone(), *command);
                    // Multiple channels may share one scalar owner. Different
                    // commands/owners for one signal need an explicit global order.
                    if result
                        .writes
                        .insert(signal.clone(), target.clone())
                        .is_some_and(|old| old != target)
                    {
                        return Err(Error::InvalidGraph);
                    }
                }
            }
        }
        Ok(result)
    }
    pub fn values(&self) -> &BTreeMap<Property, bool> {
        &self.values
    }
    pub fn handles(&self, signal: &Signal) -> bool {
        self.writes.contains_key(signal)
    }
    pub fn signal(&mut self, signal: &Signal) -> Result<Vec<(Property, bool)>, Error> {
        let (owner, command) = self.writes.get(signal).ok_or(Error::MissingInput)?.clone();
        let mut next = self.clone();
        let old = *next.outputs.get(&owner).ok_or(Error::MissingInput)?;
        let effects = next
            .nodes
            .get_mut(&owner)
            .ok_or(Error::MissingInput)?
            .apply(Action::Event(command), |port| {
                (port == nodes::FLOAT_VALUE).then_some(Value::Boolean(old))
            })?;
        let mut output = Vec::new();
        for effect in effects {
            let Effect::Publish(nodes::FLOAT_VALUE, Value::Boolean(value)) = effect else {
                return Err(Error::InvalidGraph);
            };
            next.outputs.insert(owner.clone(), value);
            for property in next.destinations.get(&owner).ok_or(Error::MissingInput)? {
                next.values.insert(property.clone(), value);
                output.push((property.clone(), value));
            }
        }
        *self = next;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_content::scoring::nodes::Configuration;
    fn id(index: usize) -> Identity {
        Identity {
            asset: "constructed".into(),
            index,
            instances: vec![],
        }
    }
    fn property() -> Property {
        Property {
            channel: "constructed/control".into(),
            port: 17,
        }
    }
    fn signal(index: usize, event: u32) -> Signal {
        Signal {
            source: id(index),
            event,
        }
    }
    pub(super) fn definitions() -> BTreeMap<Property, Vec<Publisher>> {
        BTreeMap::from([(
            property(),
            (1..=2)
                .map(|i| Publisher {
                    channel: id(i + 10),
                    input: id(i),
                    configuration: Configuration {
                        definition: nodes::Definition::Boolean { default: false },
                        defaults: vec![],
                    },
                    writes: vec![
                        (signal(i + 20, 1), nodes::SET_TRUE),
                        (signal(i + 20, 2), nodes::SET_FALSE),
                        (signal(i + 20, 3), nodes::TOGGLE),
                    ],
                })
                .collect(),
        )])
    }
    #[test]
    fn independent_publishers_and_repeated_commands_keep_last_publication_semantics() {
        let definitions = definitions();
        let mut s = State::new(&definitions).unwrap();
        let other = s.clone();
        assert!(!s.values()[&property()]);
        assert_eq!(s.signal(&signal(21, 1)).unwrap(), vec![(property(), true)]);
        // Publisher 1 remains true, but publisher 2's false is now the channel.
        assert_eq!(s.signal(&signal(22, 2)).unwrap(), vec![(property(), false)]);
        assert!(s.outputs[&id(1)]);
        assert_eq!(s.signal(&signal(22, 3)).unwrap(), vec![(property(), true)]);
        assert_eq!(s.signal(&signal(22, 3)).unwrap(), vec![(property(), false)]);
        assert_eq!(s.signal(&signal(21, 1)).unwrap(), vec![(property(), true)]);
        assert_eq!(s.signal(&signal(21, 1)).unwrap(), vec![(property(), true)]);
        assert!(!other.values()[&property()]);
        assert_eq!(State::new(&definitions).unwrap().values(), other.values());
        let before = s.values().clone();
        assert!(s.signal(&signal(99, 1)).is_err());
        assert_eq!(s.values(), &before);
    }
    #[test]
    fn missing_conflicting_initialization_and_ambiguous_event_order_fail() {
        let mut definitions = definitions();
        definitions.get_mut(&property()).unwrap()[1]
            .configuration
            .definition = nodes::Definition::Boolean { default: true };
        assert!(State::new(&definitions).is_err());
        let mut definitions = self::definitions();
        definitions.get_mut(&property()).unwrap().clear();
        assert!(State::new(&definitions).is_err());
        let mut definitions = self::definitions();
        definitions.get_mut(&property()).unwrap()[1]
            .writes
            .push((signal(21, 1), nodes::SET_FALSE));
        assert!(State::new(&definitions).is_err());
    }
}
