// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Initial Boolean expressions, with dynamic inputs supplied by owned state.
//! Stored BoolEntity defaults apply to creation only; this is not an event VM.
use super::*;
use serde_json::{Value as Json, json};

const MAX_NODES: usize = 128;
const MAX_DEPTH: usize = 24;
const PORTS: &[&str] = &[
    "synced_input",
    "logic_output",
    "bool_value",
    "not_input",
    "accessor_output",
    "participant",
    "provider_root",
    "property_link",
    "set_link",
    "set_contains",
    "context_link",
    "context_output",
    "active",
    "math_input_bool",
    "math_function",
    "math_return",
    "math_return_bool",
    "math_if_bool",
    "bool_type",
    "participant_property_type",
    "participant_set_type",
    "context_type",
];
pub(super) fn ports(v: &Json) -> Result<BTreeMap<String, u32>, Error> {
    let map = v
        .as_object()
        .filter(|m| m.len() == PORTS.len())
        .ok_or_else(|| bad("Boolean port rules"))?;
    PORTS
        .iter()
        .map(|k| {
            Ok((
                (*k).into(),
                map.get(*k)
                    .and_then(Json::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n != 0)
                    .ok_or_else(|| bad("Boolean port rule"))?,
            ))
        })
        .collect()
}

/// A reference occurrence in this participant's serializer layout, not a global
/// lookup by channel asset. The same asset may be bound in different contexts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Port {
    pub serializer: usize,
    pub field: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Input {
    ChannelProperty(Port),
    ParticipantProperty(Port),
    ParticipantSetContains(Port),
    /// The caller owns context-reporter state and its category interpretation.
    /// The installed Active expression is evaluated before this input is asked.
    Context {
        channel: Port,
        categories: Vec<String>,
        active: bool,
    },
}
impl Input {
    pub fn to_json(&self) -> Json {
        let (kind, port) = match self {
            Self::ChannelProperty(p) => ("channel_property", p),
            Self::ParticipantProperty(p) => ("participant_property", p),
            Self::ParticipantSetContains(p) => ("participant_set_contains", p),
            Self::Context {
                channel,
                categories,
                active,
            } => {
                return json!({
                "kind":"context", "channel":channel.serializer, "port":channel.field,
                "categories":categories, "active":active});
            }
        };
        json!({"kind":kind,"channel":port.serializer,"port":port.field})
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Op {
    Constant(bool),
    Input(Input),
    Not(usize),
    All(Vec<usize>),
    Any(Vec<usize>),
    Select([usize; 3]),
    Context {
        channel: Port,
        categories: Vec<String>,
        active: usize,
    },
}
/// Bounded postorder program. Construction validates all inputs, even branches
/// that a particular state would not select. Missing state never becomes false.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Program {
    ops: Vec<Op>,
}
impl Program {
    pub fn evaluate(&self, mut state: impl FnMut(&Input) -> Option<bool>) -> Result<bool, Error> {
        let mut values: Vec<bool> = Vec::with_capacity(self.ops.len());
        for op in &self.ops {
            values.push(match op {
                Op::Constant(v) => *v,
                Op::Input(i) => state(i).ok_or_else(|| bad("Boolean state input missing"))?,
                Op::Not(i) => !values[*i],
                Op::All(inputs) => inputs.iter().all(|i| values[*i]),
                Op::Any(inputs) => inputs.iter().any(|i| values[*i]),
                Op::Select([c, t, f]) => values[if values[*c] { *t } else { *f }],
                Op::Context {
                    channel,
                    categories,
                    active,
                } => state(&Input::Context {
                    channel: channel.clone(),
                    categories: categories.clone(),
                    active: values[*active],
                })
                .ok_or_else(|| bad("Boolean context state missing"))?,
            });
        }
        values
            .last()
            .copied()
            .ok_or_else(|| bad("empty Boolean program"))
    }
    /// An inspection representation; programs are constructed from installation
    /// graphs, not accepted from unvalidated JSON.
    pub fn to_json(&self) -> Json {
        Json::Array(self.ops.iter().map(|op| match op {
            Op::Constant(v) => json!({"constant":v}),
            Op::Input(i) => json!({"input":i.to_json()}),
            Op::Not(i) => json!({"not":i}),
            Op::All(i) => json!({"all":i}),
            Op::Any(i) => json!({"any":i}),
            Op::Select(i) => json!({"select":i}),
            Op::Context {channel,categories,active} => json!({"context":{
                "channel":channel.serializer,"port":channel.field,"categories":categories,"active":active}}),
        }).collect())
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub serializer: usize,
    /// None means the installed serializer has no property input. It does not
    /// mean an unsupported or unresolved expression, which is an error.
    pub initial: Option<Program>,
}

struct Compiler<'a, 'b, S: Source> {
    graph: &'a mut Graph<'b, S>,
    fields: &'a [(Site, Kind)],
    rules: &'a Rules,
    ops: Vec<Op>,
    active: BTreeSet<Endpoint>,
}
impl<S: Source> Compiler<'_, '_, S> {
    fn input(&mut self, site: &Site, port: u32) -> Result<usize, Error> {
        let endpoint = self.graph.input(site, port)?;
        self.expression(endpoint)
    }
    fn one_link(&mut self, site: &Site, port: u32) -> Result<Endpoint, Error> {
        let mut endpoints = self.graph.links(site, port)?;
        if endpoints.len() != 1 {
            return Err(bad("Boolean link endpoint count"));
        }
        Ok(endpoints.remove(0))
    }
    fn self_participant(&mut self, site: &Site) -> Result<(), Error> {
        let provider = self.one_link(site, self.rules.boolean_ports["participant"])?;
        if provider.port != 0
            || self.graph.object(&provider.site)?.class
                != "ParticipantLogicParticipantProviderEntityData"
        {
            return Err(bad("Boolean participant provider"));
        }
        let root = self.one_link(&provider.site, self.rules.boolean_ports["provider_root"])?;
        if root.port != 0 || root.site != self.fields[0].0 {
            return Err(bad("foreign Boolean participant"));
        }
        Ok(())
    }
    fn channel(&mut self, endpoint: Endpoint, kind: &str) -> Result<Port, Error> {
        if endpoint.port == 0 {
            return Err(bad("zero Boolean channel port"));
        }
        let serializer = self
            .fields
            .iter()
            .position(|(site, kind)| *site == endpoint.site && *kind == Kind::RpcReference)
            .ok_or_else(|| bad("Boolean channel outside participant layout"))?;
        if self.graph.object(&endpoint.site)?.class != "LinkedSchematicChannelReferenceEntityData" {
            return Err(bad("Boolean channel class"));
        }
        let key = self.graph.target_field(&endpoint.site, "ChannelAsset")?;
        let asset = self.graph.source.object(&key)?;
        let (table, ty) = if kind == "bool_type" {
            ("Properties", "FieldTypeHash")
        } else {
            ("Links", "LinkTypeHash")
        };
        if asset.class != "SchematicChannelAsset" {
            return Err(bad("Boolean channel asset class"));
        }
        let rows = asset
            .fields
            .get(table)
            .and_then(Value::as_array)
            .filter(|r| r.len() <= 4096)
            .ok_or_else(|| bad("Boolean channel declarations"))?;
        let mut matches = 0;
        for row in rows {
            let fields = row
                .as_fields()
                .ok_or_else(|| bad("Boolean channel declaration"))?;
            if bits32(
                fields
                    .get("Id")
                    .ok_or_else(|| bad("Boolean channel declaration ID"))?,
            )? == endpoint.port
            {
                matches += 1;
                if bits32(
                    fields
                        .get(ty)
                        .ok_or_else(|| bad("Boolean channel declaration type"))?,
                )? != self.rules.boolean_ports[kind]
                {
                    return Err(bad("Boolean channel type mismatch"));
                }
            }
        }
        if matches != 1 {
            return Err(bad("Boolean channel port missing or duplicate"));
        }
        Ok(Port {
            serializer,
            field: endpoint.port,
        })
    }
    fn expression(&mut self, endpoint: Endpoint) -> Result<usize, Error> {
        if self.active.len() >= MAX_DEPTH || self.ops.len() >= MAX_NODES {
            return Err(bad("Boolean expression bound"));
        }
        if !self.active.insert(endpoint.clone()) {
            return Err(bad("Boolean expression cycle"));
        }
        let object = self.graph.object(&endpoint.site)?;
        let site = &endpoint.site;
        let p = &self.rules.boolean_ports;
        let op = match object.class.as_str() {
            "BoolEntityData" if endpoint.port == p["bool_value"] => Op::Constant(
                object
                    .fields
                    .get("DefaultValue")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| bad("Boolean default"))?,
            ),
            "NotEntityData" if endpoint.port == p["logic_output"] => {
                Op::Not(self.input(site, p["not_input"])?)
            }
            "AndEntityData" | "OrEntityData" if endpoint.port == p["logic_output"] => {
                let count = bits32(
                    object
                        .fields
                        .get("InputCount")
                        .ok_or_else(|| bad("Boolean input count"))?,
                )? as usize;
                let ports = self.graph.input_ports(site)?;
                if count == 0
                    || count > 16
                    || ports.len() != count
                    || ports.iter().collect::<BTreeSet<_>>().len() != count
                {
                    return Err(bad("Boolean input count or duplicate"));
                }
                let inputs = ports
                    .into_iter()
                    .map(|port| self.input(site, port))
                    .collect::<Result<_, _>>()?;
                if object.class == "AndEntityData" {
                    Op::All(inputs)
                } else {
                    Op::Any(inputs)
                }
            }
            "LinkedSchematicChannelReferenceEntityData" => Op::Input(Input::ChannelProperty(
                self.channel(endpoint.clone(), "bool_type")?,
            )),
            "BoolParticipantPropertyAccessorEntityData"
                if endpoint.port == p["accessor_output"] =>
            {
                self.self_participant(site)?;
                let target = self.one_link(site, p["property_link"])?;
                Op::Input(Input::ParticipantProperty(
                    self.channel(target, "participant_property_type")?,
                ))
            }
            "ParticipantSetAccessorEntityData" if endpoint.port == p["set_contains"] => {
                self.self_participant(site)?;
                let target = self.one_link(site, p["set_link"])?;
                Op::Input(Input::ParticipantSetContains(
                    self.channel(target, "participant_set_type")?,
                ))
            }
            "ParticipantContextReporterEntityData" if endpoint.port == p["context_output"] => {
                self.self_participant(site)?;
                let target = self.one_link(site, p["context_link"])?;
                let channel = self.channel(target, "context_type")?;
                let rows = object
                    .fields
                    .get("Categories")
                    .and_then(Value::as_array)
                    .filter(|a| !a.is_empty() && a.len() <= 16)
                    .ok_or_else(|| bad("Boolean context categories"))?;
                let mut categories = Vec::new();
                for row in rows {
                    let key = self
                        .graph
                        .source
                        .target(
                            &site.key.0,
                            row.as_pointer()
                                .ok_or_else(|| bad("context category pointer"))?,
                        )?
                        .ok_or_else(|| bad("null context category"))?;
                    let category = self.graph.source.object(&key)?;
                    if category.class != "GameModeCategory"
                        || key.1 != 0
                        || categories.contains(&key.0)
                    {
                        return Err(bad("context category class or duplicate"));
                    }
                    categories.push(key.0);
                }
                let active = self.input(site, p["active"])?;
                Op::Context {
                    channel,
                    categories,
                    active,
                }
            }
            "MathEntityData" if endpoint.port == p["logic_output"] => {
                self.conditional(site, &object)?
            }
            _ => {
                return Err(bad(&format!(
                    "unsupported Boolean output {}:{:08x}",
                    object.class, endpoint.port
                )));
            }
        };
        if self.ops.len() >= MAX_NODES {
            return Err(bad("Boolean expression bound"));
        }
        let index = self.ops.len();
        self.ops.push(op);
        self.active.remove(&endpoint);
        Ok(index)
    }
    fn conditional(
        &mut self,
        site: &Site,
        object: &nfs_frostbite::ebx::Object,
    ) -> Result<Op, Error> {
        if object
            .fields
            .get("EvaluateOnEvent")
            .and_then(Value::as_bool)
            != Some(false)
        {
            return Err(bad("event-driven Boolean math is unsupported"));
        }
        let assembly = object
            .fields
            .get("Assembly")
            .and_then(Value::as_fields)
            .ok_or_else(|| bad("Boolean math assembly"))?;
        let instructions = assembly
            .get("Instructions")
            .and_then(Value::as_array)
            .filter(|a| a.len() == 5)
            .ok_or_else(|| bad("Boolean math instructions"))?;
        let calls = assembly
            .get("FunctionCalls")
            .and_then(Value::as_array)
            .filter(|a| a.len() == 1)
            .ok_or_else(|| bad("Boolean math calls"))?;
        let args = calls[0]
            .as_fields()
            .and_then(|f| f.get("Parameters"))
            .and_then(Value::as_array)
            .filter(|a| a.len() == 3)
            .ok_or_else(|| bad("Boolean math arguments"))?;
        if args.iter().map(bits32).collect::<Result<Vec<_>, _>>()? != [0, 1, 2] {
            return Err(bad("Boolean math argument order"));
        }
        let mut rows = Vec::new();
        for instruction in instructions {
            let f = instruction
                .as_fields()
                .ok_or_else(|| bad("Boolean math instruction"))?;
            let code = match f.get("Code") {
                Some(Value::Enum { value, .. }) => *value as u32,
                _ => return Err(bad("Boolean math opcode")),
            };
            let n = |k| bits32(f.get(k).ok_or_else(|| bad("Boolean math operand"))?);
            rows.push([code, n("Param1")?, n("Param2")?, n("Result")?]);
        }
        let p = &self.rules.boolean_ports;
        if rows[3] != [p["math_function"], p["math_if_bool"], 0, 0]
            || rows[4] != [p["math_return"], p["math_return_bool"], 0, 0]
        {
            return Err(bad("unsupported Boolean math function or return"));
        }
        let mut inputs = [0; 3];
        for i in 0..3 {
            if rows[i][0] != p["math_input_bool"] || rows[i][2] != 0 || rows[i][3] != i as u32 {
                return Err(bad("Boolean math input register"));
            }
            inputs[i] = self.input(site, rows[i][1])?;
        }
        Ok(Op::Select(inputs))
    }
}

fn compile<S: Source>(
    graph: &mut Graph<S>,
    fields: &[(Site, Kind)],
    rules: &Rules,
) -> Result<Vec<Field>, Error> {
    let mut result = Vec::new();
    for (serializer, (site, kind)) in fields.iter().enumerate() {
        if *kind != Kind::BoolProperty {
            continue;
        }
        let ports = graph.input_ports(site)?;
        let input = rules.boolean_ports["synced_input"];
        let initial = if ports.is_empty() {
            None
        } else {
            if ports != [input] {
                return Err(bad("Boolean serializer input ports"));
            }
            let mut compiler = Compiler {
                graph,
                fields,
                rules,
                ops: Vec::new(),
                active: BTreeSet::new(),
            };
            compiler.input(site, input)?;
            Some(Program { ops: compiler.ops })
        };
        result.push(Field {
            serializer,
            initial,
        });
    }
    Ok(result)
}
impl AssetIndex {
    /// Derive every Boolean serializer's initial expression. Dynamic leaves
    /// identify this participant's own channel occurrences. Resolve those using
    /// `participant_layout` and evaluate with the current participant's state.
    pub fn participant_boolean_values(&mut self, blueprint: &str) -> Result<Vec<Field>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("participant name bound"));
        }
        let rules = Rules::from_json(&self.identity.participant_logic.to_json())?;
        let mut graph = Graph::new(&mut self.assets);
        let nodes = graph.read(Site::root(blueprint))?;
        let (_, fields) = shape(&nodes, &rules)?;
        compile(&mut graph, &fields, &rules)
    }
}

#[cfg(test)]
mod tests;
