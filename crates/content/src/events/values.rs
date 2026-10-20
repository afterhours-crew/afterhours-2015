// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Static values of installed property graphs at creation: constants, interface
//! defaults, hubs selected by start-time integer range comparisons, conditional
//! floats and Math programs. Any other node, or an ambiguous connection, is an
//! error rather than a guess. Field and event IDs are the client's name hashes.
use crate::{Error, graph::*, scan::Assets};
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

pub(crate) struct Evaluator<'g, 'a> {
    pub graph: &'g mut Graph<'a, Assets>,
    depth: usize,
}
impl<'g, 'a> Evaluator<'g, 'a> {
    pub fn new(graph: &'g mut Graph<'a, Assets>) -> Self {
        Self { graph, depth: 0 }
    }
    fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(bad("property graph depth"));
        }
        Ok(())
    }
    /// Every value a property connection may carry into `site`'s field `port`.
    /// A field with several connections takes whichever propagates last, which
    /// the content does not fix, so all of them are kept, including those that
    /// cross prefab interfaces.
    fn candidates(&mut self, site: &Site, port: u32) -> Result<Vec<Number>, Error> {
        self.enter()?;
        let sources = self.graph.property_sources(site, port)?;
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
                vec![self.interface_default(site, port)?]
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
    fn interface_default(&mut self, site: &Site, port: u32) -> Result<Number, Error> {
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
        default(
            field
                .get("Value")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("interface default value"))?,
        )
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
        self.input(site, port)?
            .ok_or_else(|| bad("unconnected required property"))
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
                let [k] = <[u32; 1]>::try_from(selected)
                    .map_err(|_| bad("hub selection is not unique at start"))?;
                self.required(site, name(&format!("In{k}")))?
            }
            "ConditionalFloatEntityData" if port == OUTPUT => {
                let condition = match self.required(site, CONDITION)? {
                    Number::Boolean(v) => v,
                    _ => return Err(bad("conditional float condition type")),
                };
                if condition {
                    self.field_or_input(site, IF_TRUE, "ValueIfTrue")?
                } else {
                    self.field_or_input(site, IF_FALSE, "ValueIfFalse")?
                }
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
                    .evaluate(|port, _| inputs.iter().find(|(p, _)| *p == port).map(|(_, v)| *v))
                    .map_err(|_| bad("math evaluation"))?
                    .ok_or_else(|| bad("math division by zero"))?;
                match result {
                    MathValue::Integer(v) => Number::Integer(v),
                    MathValue::Float(v) => Number::Float(v),
                    MathValue::Boolean(v) => Number::Boolean(v),
                    _ => return Err(bad("unsupported math result type")),
                }
            }
            class => return Err(bad(&format!("unsupported property source {class}"))),
        };
        self.depth -= 1;
        Ok(value)
    }
    /// Whether `event` of `source` fires at creation: an integer range comparison
    /// that triggers on start and whose value lies in its closed range.
    fn fires_at_start(&mut self, source: &Site, event: u32) -> Result<bool, Error> {
        let object = self.graph.object(source)?;
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
                return Err(bad("range comparison inputs disagree"));
            }
            outcome = Some(inside);
        }
        outcome.ok_or_else(|| bad("unconnected range comparison"))
    }
}
