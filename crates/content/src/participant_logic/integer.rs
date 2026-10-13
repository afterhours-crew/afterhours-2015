// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Initial integer expressions and installed score-to-level calculators.
//! Event-driven gates and later counter mutations require a lifecycle model;
//! they are rejected instead of being replaced with default or captured values.
pub use super::boolean::Port;
use super::*;
use nfs_services::reputation::{Field as LevelField, MAX_THRESHOLDS, Scores, Thresholds};
use serde_json::{Value as Json, json};

const MAX_NODES: usize = 128;
const MAX_DEPTH: usize = 24;
const PORTS: &[&str] = &[
    "synced_input",
    "synced_output",
    "int_value",
    "accessor_output",
    "property_link",
    "score_input",
    "level_output",
    "next_threshold_output",
    "current_threshold_output",
    "participant_property_type",
];
pub(super) fn ports(v: &Json) -> Result<BTreeMap<String, u32>, Error> {
    let map = v
        .as_object()
        .filter(|m| m.len() == PORTS.len())
        .ok_or_else(|| bad("integer port rules"))?;
    PORTS
        .iter()
        .map(|k| {
            Ok((
                (*k).into(),
                map.get(*k)
                    .and_then(Json::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n != 0)
                    .ok_or_else(|| bad("integer port rule"))?,
            ))
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Op {
    Constant(i32),
    ParticipantProperty(Port),
    Threshold {
        score: usize,
        values: Vec<i32>,
        field: LevelField,
    },
}
/// A validated, bounded postorder expression. All dynamic inputs belong to the
/// participant whose layout was used to compile it. Missing state is an error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Program {
    ops: Vec<Op>,
}
impl Program {
    pub fn evaluate(&self, mut state: impl FnMut(&Port) -> Option<i32>) -> Result<i32, Error> {
        let mut result: Vec<i32> = Vec::with_capacity(self.ops.len());
        for op in &self.ops {
            result.push(match op {
                Op::Constant(v) => *v,
                Op::ParticipantProperty(port) => {
                    state(port).ok_or_else(|| bad("integer state input missing"))?
                }
                Op::Threshold {
                    score,
                    values,
                    field,
                } => Thresholds::new(values.clone())
                    .map_err(|_| bad("integer thresholds"))?
                    .evaluate(Scores {
                        total: result[*score],
                        build: 0,
                        crew: 0,
                        outlaw: 0,
                        speed: 0,
                        style: 0,
                    })
                    .get(*field),
            });
        }
        result
            .last()
            .copied()
            .ok_or_else(|| bad("empty integer program"))
    }
    /// Inspection only. Programs are compiled from validated installation data,
    /// never accepted as arbitrary JSON instructions.
    pub fn to_json(&self) -> Json {
        Json::Array(
            self.ops
                .iter()
                .map(|op| match op {
                    Op::Constant(v) => json!({"constant":v}),
                    Op::ParticipantProperty(p) => {
                        json!({"participant_property":{"channel":p.serializer,"port":p.field}})
                    }
                    Op::Threshold {
                        score,
                        values,
                        field,
                    } => json!({"score":score,"thresholds":values,"output":match field {
                        LevelField::Level=>"level", LevelField::NextThreshold=>"next_threshold",
                        LevelField::CurrentThreshold=>"current_threshold", _=>unreachable!(),
                    }}),
                })
                .collect(),
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub serializer: usize,
    /// None means no property input exists. An unresolved, ambiguous or
    /// unsupported source always returns an error instead.
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
    fn expression(&mut self, endpoint: Endpoint) -> Result<usize, Error> {
        if self.active.len() >= MAX_DEPTH || self.ops.len() >= MAX_NODES {
            return Err(bad("integer expression bound"));
        }
        if !self.active.insert(endpoint.clone()) {
            return Err(bad("integer expression cycle"));
        }
        let object = self.graph.object(&endpoint.site)?;
        let site = &endpoint.site;
        let p = &self.rules.integer_ports;
        let op = match object.class.as_str() {
            "IntEntityData" if endpoint.port == p["int_value"] => {
                Op::Constant(match object.fields.get("DefaultValue") {
                    Some(Value::Int32(v)) => *v,
                    _ => return Err(bad("integer default type")),
                })
            }
            "IntParticipantPropertyAccessorEntityData" if endpoint.port == p["accessor_output"] => {
                Op::ParticipantProperty(boolean::property_binding(
                    self.graph,
                    self.fields,
                    self.rules,
                    site,
                    p["property_link"],
                )?)
            }
            "SyncedIntEntityData" if endpoint.port == p["synced_output"] => {
                if !self
                    .fields
                    .iter()
                    .any(|(s, k)| s == site && *k == Kind::I32Property)
                {
                    return Err(bad("integer source outside participant layout"));
                }
                let index = self.input(site, p["synced_input"])?;
                self.active.remove(&endpoint);
                return Ok(index);
            }
            "IntScoreToIntLevelEntityData" => {
                let field = if endpoint.port == p["level_output"] {
                    LevelField::Level
                } else if endpoint.port == p["next_threshold_output"] {
                    LevelField::NextThreshold
                } else if endpoint.port == p["current_threshold_output"] {
                    LevelField::CurrentThreshold
                } else {
                    return Err(bad("unsupported integer calculator output"));
                };
                let key = self.graph.target_field(site, "LevelThresholdsAsset")?;
                let asset = self.graph.source.object(&key)?;
                if asset.class != "IntScoreToIntLevelAsset" {
                    return Err(bad("integer threshold asset class"));
                }
                let rows = asset
                    .fields
                    .get("LevelThresholds")
                    .and_then(Value::as_array)
                    .filter(|a| !a.is_empty() && a.len() <= MAX_THRESHOLDS)
                    .ok_or_else(|| bad("integer threshold count"))?;
                let values = rows
                    .iter()
                    .map(|v| match v {
                        Value::Int32(v) => Ok(*v),
                        _ => Err(bad("integer threshold type")),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Thresholds::new(values.clone()).map_err(|_| bad("integer threshold order"))?;
                let score = self.input(site, p["score_input"])?;
                Op::Threshold {
                    score,
                    values,
                    field,
                }
            }
            _ => {
                return Err(bad(&format!(
                    "unsupported integer output {}:{:08x}",
                    object.class, endpoint.port
                )));
            }
        };
        if self.ops.len() >= MAX_NODES {
            return Err(bad("integer expression bound"));
        }
        let index = self.ops.len();
        self.ops.push(op);
        self.active.remove(&endpoint);
        Ok(index)
    }
}
fn compile<S: Source>(
    graph: &mut Graph<S>,
    fields: &[(Site, Kind)],
    rules: &Rules,
) -> Result<Vec<Field>, Error> {
    let mut result = Vec::new();
    for (serializer, (site, kind)) in fields.iter().enumerate() {
        if *kind != Kind::I32Property {
            continue;
        }
        let ports = graph.input_ports(site)?;
        let input = rules.integer_ports["synced_input"];
        let initial = if ports.is_empty() {
            None
        } else {
            if ports != [input] {
                return Err(bad("integer serializer input ports"));
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
    /// Derive initial integer expressions, following exact participant channel
    /// occurrences and the calculator's installed threshold reference.
    /// Event-driven sources require a separate owned lifecycle and are rejected.
    pub fn participant_integer_values(&mut self, blueprint: &str) -> Result<Vec<Field>, Error> {
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
