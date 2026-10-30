// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Static values of installed property graphs at creation: constants, interface
//! defaults, hubs selected by start-time integer range comparisons, conditional
//! floats and Math programs. Any other node, or an ambiguous connection, is an
//! error rather than a guess. Field and event IDs are the client's name hashes.
use crate::{Error, graph::*};
use nfs_frostbite::ebx::Value;
use nfs_world::scoring::math::{Type, Value as MathValue};

/// djb2 with XOR mixing: the client's field and event name hash.
pub(crate) const fn name(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let mut h = 5381u32;
    let mut i = 0;
    while i < bytes.len() {
        h = h.wrapping_mul(33) ^ bytes[i] as u32;
        i += 1;
    }
    h
}
const OUT: u32 = name("Out");
const VALUE: u32 = name("Value");
const OUTPUT: u32 = name("Output");
const CONDITION: u32 = name("Condition");
const IF_TRUE: u32 = name("ValueIfTrue");
const IF_FALSE: u32 = name("ValueIfFalse");
const IN_RANGE: u32 = name("InRange");
const MAX_DEPTH: usize = 64;
/// Bound on the values one field may receive across prefab boundaries.
const MAX_CANDIDATES: usize = 16;

#[cfg(test)]
mod precedence;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    Integer(i32),
    Float(f32),
    Boolean(bool),
}
impl Number {
    fn as_math(self, ty: Type) -> Result<MathValue, Error> {
        Ok(match (self, ty) {
            (Self::Integer(v), Type::Integer) => MathValue::Integer(v),
            (Self::Float(v), Type::Float) => MathValue::Float(v),
            (Self::Boolean(v), Type::Boolean) => MathValue::Boolean(v),
            _ => return Err(bad("property type mismatch")),
        })
    }
}

fn scalar(value: &Value) -> Result<Number, Error> {
    Ok(match value {
        Value::Int32(v) => Number::Integer(*v),
        Value::UInt32(v) => Number::Integer(i32::try_from(*v).map_err(|_| bad("property range"))?),
        Value::Float32(v) => Number::Float(*v),
        Value::Bool(v) => Number::Boolean(*v),
        _ => return Err(bad("unsupported property value")),
    })
}
/// An interface field default: the type name, a space and the value.
fn default(text: &str) -> Result<Number, Error> {
    let (kind, value) = text
        .split_once(' ')
        .ok_or_else(|| bad("interface default"))?;
    Ok(match kind {
        "Int32" => Number::Integer(value.parse().map_err(|_| bad("interface integer"))?),
        "Float32" => Number::Float(value.parse().map_err(|_| bad("interface float"))?),
        "Boolean" => Number::Boolean(match value {
            "true" | "True" => true,
            "false" | "False" => false,
            _ => return Err(bad("interface Boolean")),
        }),
        _ => return Err(bad("unsupported interface default type")),
    })
}

pub(crate) struct Evaluator<'g, 'a, S: Source> {
    pub graph: &'g mut Graph<'a, S>,
    channels: Option<&'g std::collections::BTreeMap<Site, Site>>,
    depth: usize,
}
impl<'g, 'a, S: Source> Evaluator<'g, 'a, S> {
    pub fn new(graph: &'g mut Graph<'a, S>) -> Self {
        Self {
            graph,
            channels: None,
            depth: 0,
        }
    }
    pub fn with_channels(
        graph: &'g mut Graph<'a, S>,
        channels: &'g std::collections::BTreeMap<Site, Site>,
    ) -> Self {
        Self {
            graph,
            channels: Some(channels),
            depth: 0,
        }
    }
    fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(bad("property graph depth"));
        }
        Ok(())
    }
    /// Every value a property connection may carry into `site`'s field `port`.
    /// General fan-in retains every candidate. Known startup publishers can
    /// supersede an unbound bus-interface initialization default: constants at
    /// binding and ConditionalFloat's selected Output at activation. A bound
    /// interface remains a producer, so it receives no such precedence.
    fn candidates(&mut self, site: &Site, port: u32) -> Result<Vec<Number>, Error> {
        self.enter()?;
        let sources = self.graph.property_sources(site, port)?;
        if let [a, b] = sources.as_slice() {
            for (publisher, fallback) in [(a, b), (b, a)] {
                let publishes = match self.graph.object(&publisher.site)?.class.as_str() {
                    "IntEntityData" | "FloatEntityData" | "BoolEntityData" => {
                        publisher.port == VALUE
                    }
                    "ConditionalFloatEntityData" => publisher.port == OUTPUT,
                    _ => false,
                };
                if !publishes
                    || self.graph.object(&fallback.site)?.class != "InterfaceDescriptorData"
                {
                    continue;
                }
                let mut parent = fallback.site.clone();
                let unbound = match parent.parents.pop() {
                    Some(frame) => {
                        parent.key = frame.reference;
                        parent.bus = frame.bus;
                        self.graph
                            .property_sources(&parent, fallback.port)?
                            .is_empty()
                    }
                    None => true,
                };
                if unbound {
                    let value = self.output(&publisher.site, publisher.port)?;
                    if self
                        .interface_default(&fallback.site, fallback.port)?
                        .is_some_and(|v| {
                            std::mem::discriminant(&v) != std::mem::discriminant(&value)
                        })
                    {
                        return Err(bad(
                            "startup publisher and interface fallback types disagree",
                        ));
                    }
                    self.depth -= 1;
                    return Ok(vec![value]);
                }
            }
        }
        let mut values = Vec::new();
        for source in sources {
            values.extend(self.values(&source.site, source.port)?);
            if values.len() > MAX_CANDIDATES {
                return Err(bad("property input fan-in bound"));
            }
        }
        self.depth -= 1;
        Ok(values)
    }
    /// The value flowing into `site`'s field `port`, or `None` when unconnected.
    /// Several connections are accepted only when they all carry one value.
    pub fn input(&mut self, site: &Site, port: u32) -> Result<Option<Number>, Error> {
        let values = self.candidates(site, port)?;
        match values.split_first() {
            None => Ok(None),
            Some((first, rest)) if rest.iter().all(|v| v == first) => Ok(Some(*first)),
            Some(_) => Err(bad(&format!(
                "conflicting property inputs {}:{} field {port}: {values:?}",
                site.key.0, site.key.1
            ))),
        }
    }
    /// Every value a node may publish on `port`. Prefab interfaces pass on all
    /// the values connected to them; other nodes publish one value.
    fn values(&mut self, site: &Site, port: u32) -> Result<Vec<Number>, Error> {
        self.enter()?;
        let class = self.graph.object(site)?.class.clone();
        let values = if class == "InterfaceDescriptorData" {
            // A prefab input: the values connected to the prefab instance in
            // its parent, else the interface default.
            let mut parent = site.clone();
            let connected = match parent.parents.pop() {
                Some(frame) => {
                    parent.key = frame.reference;
                    parent.bus = frame.bus;
                    self.candidates(&parent, port)?
                }
                None => Vec::new(),
            };
            if connected.is_empty() {
                self.interface_default(site, port)?.into_iter().collect()
            } else {
                connected
            }
        } else if class.ends_with("ReferenceObjectData") {
            // A prefab output: the values reaching the instance interface's
            // field from inside the prefab.
            let child = self.graph.target_field(site, "Blueprint")?;
            let mut inner = site.clone();
            inner.parents.push(Frame {
                bus: site.bus.clone(),
                reference: site.key.clone(),
            });
            inner.bus = child.clone();
            inner.key = child;
            inner.key = self.graph.target_field(&inner, "Interface")?;
            let values = self.candidates(&inner, port)?;
            if values.is_empty() {
                return Err(bad("unconnected prefab output"));
            }
            values
        } else {
            vec![self.output(site, port)?]
        };
        self.depth -= 1;
        Ok(values)
    }
    fn interface_default(&mut self, site: &Site, port: u32) -> Result<Option<Number>, Error> {
        let object = self.graph.object(site)?;
        let fields = object
            .fields
            .get("Fields")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("interface fields"))?;
        let mut found = fields
            .iter()
            .filter_map(Value::as_fields)
            .filter(|f| f.get("Id").and_then(Value::bits32) == Some(port));
        let field = found.next().ok_or_else(|| bad("interface field missing"))?;
        if found.next().is_some() {
            return Err(bad("duplicate interface field"));
        }
        let text = field
            .get("Value")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("interface default value"))?;
        // An empty interface declaration publishes nothing. Receiving nodes
        // keep their own authored defaults until a source supplies a value.
        if text.is_empty() {
            return Ok(None);
        }
        default(text).map(Some).map_err(|error| {
            bad(&format!(
                "interface {}:{} field {port}: {error}",
                site.key.0, site.key.1
            ))
        })
    }
    /// One value of `values`, refusing a field whose connections disagree.
    fn unique(&mut self, site: &Site, port: u32) -> Result<Number, Error> {
        let values = self.values(site, port)?;
        match values.split_first() {
            Some((first, rest)) if rest.iter().all(|v| v == first) => Ok(*first),
            _ => Err(bad(&format!(
                "conflicting prefab values {}:{} field {port}: {values:?}",
                site.key.0, site.key.1
            ))),
        }
    }
    fn required(&mut self, site: &Site, port: u32) -> Result<Number, Error> {
        self.input(site, port)?.ok_or_else(|| {
            bad(&format!(
                "unconnected required property {}:{} port {port}",
                site.key.0, site.key.1
            ))
        })
    }
    /// A channel can have an interface fallback and one start-selected hub.
    /// Interface defaults bind before activation; the hub's explicit startup
    /// selection then publishes the difficulty-specific value.
    fn channel_value(&mut self, site: &Site, port: u32) -> Result<Number, Error> {
        let rows = self.graph.property_sources(site, port)?;
        if rows.len() <= 1 {
            return self.required(site, port);
        }
        let mut hub = None;
        let mut fallback = None;
        for source in rows {
            match self.graph.object(&source.site)?.class.as_str() {
                "FloatHubEntityData" | "IntHubEntityData" if hub.is_none() => {
                    hub = Some(source);
                }
                "InterfaceDescriptorData" if fallback.is_none() => {
                    fallback = Some(source);
                }
                _ => return Err(bad("ambiguous startup channel value")),
            }
        }
        let (Some(hub), Some(fallback)) = (hub, fallback) else {
            return Err(bad("startup channel sources"));
        };
        // Refuse a hub that never receives an initial selection.
        let count = self
            .graph
            .object(&hub.site)?
            .fields
            .get("InputCount")
            .and_then(Value::bits32)
            .filter(|n| (1..=16).contains(n))
            .ok_or_else(|| bad("startup channel hub count"))?;
        let mut selects = false;
        for i in 0..count {
            for (source, event) in self
                .graph
                .event_sources(&hub.site, name(&format!("SelectIn{i}")))?
            {
                selects |= self.fires_at_start(&source, event)?;
            }
        }
        if !selects {
            return Err(bad("channel hub has no startup selection"));
        }
        let a = self.output(&fallback.site, fallback.port)?;
        let b = self.output(&hub.site, hub.port)?;
        if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
            return Err(bad("channel fallback type"));
        }
        Ok(b)
    }
    /// An input that falls back to the node's own field.
    pub fn field_or_input(&mut self, site: &Site, port: u32, field: &str) -> Result<Number, Error> {
        match self.input(site, port)? {
            Some(v) => Ok(v),
            None => match self.graph.object(site)?.fields.get(field) {
                Some(v) => scalar(v),
                None => Err(bad("missing property field")),
            },
        }
    }
    /// The value a node publishes on `port`.
    pub fn output(&mut self, site: &Site, port: u32) -> Result<Number, Error> {
        self.enter()?;
        let object = self.graph.object(site)?;
        let value = match object.class.as_str() {
            "LinkedSchematicChannelReferenceEntityData" => {
                let source = match self.channels.and_then(|bindings| bindings.get(site)) {
                    Some(site) => Endpoint {
                        site: site.clone(),
                        port: 0,
                    },
                    None => self.graph.linked_source(site, 0)?,
                };
                if source.port != 0
                    || self.graph.object(&source.site)?.class != "LinkedSchematicChannelEntityData"
                    || self.graph.target_field(&source.site, "ChannelAsset")?
                        != self.graph.target_field(site, "ChannelAsset")?
                {
                    return Err(bad(&format!(
                        "static schematic channel binding: {}:{} field {} class {}",
                        source.site.key.0,
                        source.site.key.1,
                        source.port,
                        self.graph.object(&source.site)?.class
                    )));
                }
                self.channel_value(&source.site, port)?
            }
            class
                if class == "InterfaceDescriptorData" || class.ends_with("ReferenceObjectData") =>
            {
                self.unique(site, port)?
            }
            "IntEntityData" | "FloatEntityData" | "BoolEntityData" if port == VALUE => scalar(
                object
                    .fields
                    .get("DefaultValue")
                    .ok_or_else(|| bad("constant default value"))?,
            )?,
            "IntHubEntityData" | "FloatHubEntityData" if port == OUT => {
                let count = object
                    .fields
                    .get("InputCount")
                    .and_then(Value::bits32)
                    .filter(|n| (1..=16).contains(n))
                    .ok_or_else(|| bad("hub input count"))?;
                let mut selected = Vec::new();
                for k in 0..count {
                    let select = name(&format!("SelectIn{k}"));
                    for (source, event) in self.graph.event_sources(site, select)? {
                        if self.fires_at_start(&source, event)? {
                            selected.push(k);
                        }
                    }
                }
                selected.sort_unstable();
                selected.dedup();
                let k = match selected.as_slice() {
                    [] => object
                        .fields
                        .get("InputSelect")
                        .and_then(Value::bits32)
                        .filter(|v| *v < count)
                        .ok_or_else(|| bad("hub default selection"))?,
                    &[k] => k,
                    _ => return Err(bad("hub selection is not unique at start")),
                };
                self.required(site, name(&format!("In{k}")))?
            }
            "ConditionalFloatEntityData" if port == OUTPUT => {
                let condition = match self.field_or_input(site, CONDITION, "Condition")? {
                    Number::Boolean(v) => v,
                    _ => return Err(bad("conditional float condition type")),
                };
                let selected = if condition {
                    self.field_or_input(site, IF_TRUE, "ValueIfTrue")?
                } else {
                    self.field_or_input(site, IF_FALSE, "ValueIfFalse")?
                };
                if !matches!(selected, Number::Float(v) if v.is_finite()) {
                    return Err(bad("conditional float selected value type or range"));
                }
                selected
            }
            "MathEntityData" if port == OUT => {
                let crate::scoring::Calculation::Math { program, .. } =
                    crate::scoring::compile(&object)?
                else {
                    return Err(bad("math calculation"));
                };
                let mut inputs = Vec::new();
                for (input, ty) in program.inputs() {
                    let value = self.required(site, input)?.as_math(ty)?;
                    inputs.push((input, value));
                }
                let result = program
                    .evaluate(|port, _| {
                        inputs
                            .iter()
                            .find(|(p, _)| *p == port)
                            .map(|(_, v)| v.clone())
                    })
                    .map_err(|_| bad("math evaluation"))?
                    .ok_or_else(|| bad("math division by zero"))?;
                match result {
                    MathValue::Integer(v) => Number::Integer(v),
                    MathValue::Float(v) => Number::Float(v),
                    MathValue::Boolean(v) => Number::Boolean(v),
                    _ => return Err(bad("unsupported math result type")),
                }
            }
            class => {
                return Err(bad(&format!(
                    "unsupported property source {class} {}:{} field {port}",
                    site.key.0, site.key.1
                )));
            }
        };
        self.depth -= 1;
        Ok(value)
    }
    /// Whether `event` of `source` fires at creation: an integer range comparison
    /// that triggers on start and whose value lies in its closed range.
    fn fires_at_start(&mut self, source: &Site, event: u32) -> Result<bool, Error> {
        let object = self.graph.object(source)?;
        if object.class == "CompareBoolEntityData" && event == name("OnTrue") {
            // Initial property publications are pending comparisons until
            // activation. An unbound interface publishes nothing.
            let input = self.input(source, name("Bool"))?;
            let initial = object
                .fields
                .get("Bool")
                .and_then(Value::as_bool)
                .ok_or_else(|| bad("Boolean selector default"))?;
            let value = match input {
                Some(Number::Boolean(v)) => v,
                None => initial,
                _ => return Err(bad("Boolean selector input")),
            };
            return Ok(value
                && (object.fields.get("TriggerOnStart").and_then(Value::as_bool) == Some(true)
                    || (input.is_some()
                        && object
                            .fields
                            .get("TriggerOnPropertyChange")
                            .and_then(Value::as_bool)
                            == Some(true))));
        }
        if object.class != "CompareIntRangeEntityData" || event != IN_RANGE {
            return Err(bad("unsupported hub selector"));
        }
        if object.fields.get("TriggerOnStart").and_then(Value::as_bool) != Some(true) {
            return Err(bad("hub selector does not trigger on start"));
        }
        let bound = |name: &str| -> Result<i32, Error> {
            match object.fields.get(name) {
                Some(Value::Int32(v)) => Ok(*v),
                None => Ok(0),
                _ => Err(bad("range bound type")),
            }
        };
        let (min, max) = (bound("Min")?, bound("Max")?);
        // Several inputs are accepted when every one gives the same outcome.
        let mut outcome = None;
        for value in self.candidates(source, VALUE)? {
            let Number::Integer(value) = value else {
                return Err(bad("range comparison value type"));
            };
            let inside = (min..=max).contains(&value);
            if outcome.is_some_and(|o| o != inside) {
                return Err(bad(&format!(
                    "range comparison inputs disagree {}:{} range {min}..={max}",
                    source.key.0, source.key.1
                )));
            }
            outcome = Some(inside);
        }
        outcome.ok_or_else(|| bad("unconnected range comparison"))
    }
}

#[cfg(test)]
mod channel_tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};
    use std::collections::BTreeMap;
    fn fixture() -> Memory {
        let mut m = Memory::default();
        m.put(
            "r",
            0,
            "Blueprint",
            &[
                (
                    "LinkConnections",
                    Value::Array(vec![edge(local(2), 0, local(1), 0)]),
                ),
                (
                    "PropertyConnections",
                    Value::Array(vec![edge(local(3), VALUE, local(2), 77)]),
                ),
            ],
        );
        m.put(
            "r",
            1,
            "LinkedSchematicChannelReferenceEntityData",
            &[("ChannelAsset", ptr("c", 0))],
        );
        m.put(
            "r",
            2,
            "LinkedSchematicChannelEntityData",
            &[("ChannelAsset", ptr("c", 0))],
        );
        m.put(
            "r",
            3,
            "IntEntityData",
            &[("DefaultValue", Value::Int32(123))],
        );
        m.put("c", 0, "SchematicChannelAsset", &[]);
        m
    }
    fn site(i: usize) -> Site {
        Site {
            key: ("r".into(), i),
            ..Site::root("r")
        }
    }
    #[test]
    fn conditional_uses_authored_default_until_an_input_publishes() {
        use nfs_frostbite::ebx::Fields;
        let mut m = fixture();
        m.put(
            "r",
            0,
            "Blueprint",
            &[(
                "PropertyConnections",
                Value::Array(vec![edge(local(4), 7, local(3), CONDITION)]),
            )],
        );
        let interface = |text: &str| {
            Value::Array(vec![Value::Struct(Fields(vec![
                ("Id".into(), Value::UInt32(7)),
                ("Value".into(), Value::String(text.into())),
            ]))])
        };
        m.put(
            "r",
            3,
            "ConditionalFloatEntityData",
            &[
                ("Condition", Value::Bool(false)),
                ("ValueIfTrue", Value::Float32(2.)),
                ("ValueIfFalse", Value::Float32(1.)),
            ],
        );
        for (input, expected) in [("", 1.), ("Boolean true", 2.), ("Boolean false", 1.)] {
            m.put(
                "r",
                4,
                "InterfaceDescriptorData",
                &[("Fields", interface(input))],
            );
            assert_eq!(
                Evaluator::new(&mut Graph::new(&mut m))
                    .output(&site(3), OUTPUT)
                    .unwrap(),
                Number::Float(expected)
            );
        }
        m.put(
            "r",
            4,
            "InterfaceDescriptorData",
            &[("Fields", interface("Int32 1"))],
        );
        assert!(
            Evaluator::new(&mut Graph::new(&mut m))
                .output(&site(3), OUTPUT)
                .is_err()
        );
    }
    #[test]
    fn constant_binding_supersedes_unbound_bus_default_but_not_another_producer() {
        use nfs_frostbite::ebx::Fields;
        let mut m = fixture();
        m.put("r", 6, "CompareIntRangeEntityData", &[]);
        m.put(
            "r",
            4,
            "InterfaceDescriptorData",
            &[(
                "Fields",
                Value::Array(vec![Value::Struct(Fields(vec![
                    ("Id".into(), Value::UInt32(7)),
                    ("Value".into(), Value::String("Int32 1".into())),
                ]))]),
            )],
        );
        for reverse in [false, true] {
            let mut edges = vec![
                edge(local(3), VALUE, local(6), VALUE),
                edge(local(4), 7, local(6), VALUE),
            ];
            if reverse {
                edges.reverse();
            }
            m.put(
                "r",
                0,
                "Blueprint",
                &[("PropertyConnections", Value::Array(edges))],
            );
            assert_eq!(
                Evaluator::new(&mut Graph::new(&mut m))
                    .input(&site(6), VALUE)
                    .unwrap(),
                Some(Number::Integer(123))
            );
        }
        m.put(
            "r",
            4,
            "IntEntityData",
            &[("DefaultValue", Value::Int32(1))],
        );
        m.put(
            "r",
            0,
            "Blueprint",
            &[(
                "PropertyConnections",
                Value::Array(vec![
                    edge(local(3), VALUE, local(6), VALUE),
                    edge(local(4), VALUE, local(6), VALUE),
                ]),
            )],
        );
        assert!(
            Evaluator::new(&mut Graph::new(&mut m))
                .input(&site(6), VALUE)
                .is_err()
        );
    }
    #[test]
    fn static_channel_values_follow_only_the_supplied_or_wired_binding() {
        let mut m = fixture();
        assert_eq!(
            Evaluator::new(&mut Graph::new(&mut m))
                .output(&site(1), 77)
                .unwrap(),
            Number::Integer(123)
        );
        // Spawned references use the parent game-mode binding, not a name search.
        m.put(
            "r",
            0,
            "Blueprint",
            &[
                ("LinkConnections", Value::Array(vec![])),
                (
                    "PropertyConnections",
                    Value::Array(vec![edge(local(3), VALUE, local(2), 77)]),
                ),
            ],
        );
        assert!(
            Evaluator::new(&mut Graph::new(&mut m))
                .output(&site(1), 77)
                .is_err()
        );
        let bindings = BTreeMap::from([(site(1), site(2))]);
        assert_eq!(
            Evaluator::with_channels(&mut Graph::new(&mut m), &bindings)
                .output(&site(1), 77)
                .unwrap(),
            Number::Integer(123)
        );
        m.put(
            "r",
            2,
            "LinkedSchematicChannelEntityData",
            &[("ChannelAsset", ptr("foreign", 0))],
        );
        assert!(
            Evaluator::with_channels(&mut Graph::new(&mut m), &bindings)
                .output(&site(1), 77)
                .is_err()
        );
    }
    #[test]
    fn unbound_or_cyclic_static_channel_values_are_errors() {
        let mut m = fixture();
        assert!(
            Evaluator::new(&mut Graph::new(&mut m))
                .output(&site(1), 78)
                .is_err()
        );
        m.put(
            "r",
            0,
            "Blueprint",
            &[
                (
                    "LinkConnections",
                    Value::Array(vec![edge(local(2), 0, local(1), 0)]),
                ),
                (
                    "PropertyConnections",
                    Value::Array(vec![edge(local(1), 77, local(2), 77)]),
                ),
            ],
        );
        assert!(
            Evaluator::new(&mut Graph::new(&mut m))
                .output(&site(1), 77)
                .is_err()
        );
    }
    #[test]
    fn channel_hub_startup_overrides_its_interface_fallback() {
        use nfs_frostbite::ebx::Fields;
        let mut m = fixture();
        let field = |id, text: &str| {
            Value::Struct(Fields(vec![
                ("Id".into(), Value::UInt32(id)),
                ("Value".into(), Value::String(text.into())),
            ]))
        };
        let event = |id| Value::Struct(Fields(vec![("Id".into(), Value::UInt32(id))]));
        m.put(
            "r",
            0,
            "Blueprint",
            &[
                (
                    "LinkConnections",
                    Value::Array(vec![edge(local(2), 0, local(1), 0)]),
                ),
                (
                    "PropertyConnections",
                    Value::Array(vec![
                        edge(local(4), 7, local(5), name("In0")),
                        edge(local(4), 8, local(2), 77),
                        edge(local(5), OUT, local(2), 77),
                        edge(local(3), VALUE, local(6), VALUE),
                    ]),
                ),
                (
                    "EventConnections",
                    Value::Array(vec![Value::Struct(Fields(vec![
                        ("Source".into(), local(6)),
                        ("Target".into(), local(5)),
                        ("SourceEvent".into(), event(IN_RANGE)),
                        ("TargetEvent".into(), event(name("SelectIn0"))),
                    ]))]),
                ),
            ],
        );
        m.put(
            "r",
            3,
            "IntEntityData",
            &[("DefaultValue", Value::Int32(0))],
        );
        m.put(
            "r",
            4,
            "InterfaceDescriptorData",
            &[(
                "Fields",
                Value::Array(vec![field(7, "Float32 60"), field(8, "Float32 44")]),
            )],
        );
        m.put(
            "r",
            5,
            "FloatHubEntityData",
            &[
                ("InputCount", Value::UInt32(1)),
                ("InputSelect", Value::UInt32(0)),
            ],
        );
        m.put(
            "r",
            6,
            "CompareIntRangeEntityData",
            &[
                ("Min", Value::Int32(0)),
                ("Max", Value::Int32(15)),
                ("TriggerOnStart", Value::Bool(true)),
            ],
        );
        assert_eq!(
            Evaluator::new(&mut Graph::new(&mut m))
                .output(&site(1), 77)
                .unwrap(),
            Number::Float(60.)
        );
        m.put(
            "r",
            6,
            "CompareIntRangeEntityData",
            &[
                ("Min", Value::Int32(1)),
                ("Max", Value::Int32(15)),
                ("TriggerOnStart", Value::Bool(true)),
            ],
        );
        assert!(
            Evaluator::new(&mut Graph::new(&mut m))
                .output(&site(1), 77)
                .is_err()
        );
    }
}
