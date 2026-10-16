// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compile installed scalar calculations. This does not execute a scoring
//! lifecycle or enable participants; query ownership and events are separate.
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::{Fields, Object, Value};
use nfs_world::scoring::{
    curve::{Curve, Interpolation, Knot, MAX_KNOTS},
    math::{self, FloatOp, Instruction, Operation, Program, Register},
};

pub mod arithmetic;
pub mod bindings;
pub mod boolean_fold;
pub mod creation;
pub mod delay;
pub mod events;
pub mod interfaces;
pub mod nodes;
pub mod property_gate;
pub mod realms;
pub mod stopwatch;
pub mod synced;

#[derive(Clone, Debug, PartialEq)]
pub enum Calculation {
    Math {
        program: Program,
        evaluate_on_event: bool,
    },
    Curve {
        curve: Curve,
        apply: bool,
        default_input: f32,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub asset: String,
    pub index: usize,
    /// Prefab reference occurrences, outermost first. Identical assets in two
    /// prefab instances must retain separate state and bindings.
    pub instances: Vec<(String, usize)>,
    pub calculation: Calculation,
}

fn field<'a>(fields: &'a Fields, name: &str) -> Result<&'a Value, Error> {
    fields.get(name).ok_or_else(|| bad("scoring field missing"))
}
fn scalar(fields: &Fields, name: &str) -> Result<f32, Error> {
    match field(fields, name)? {
        Value::Float32(value) if value.is_finite() => Ok(*value),
        _ => Err(bad("scoring finite float field")),
    }
}
fn boolean(fields: &Fields, name: &str) -> Result<bool, Error> {
    field(fields, name)?
        .as_bool()
        .ok_or_else(|| bad("scoring Boolean field"))
}
fn bits(value: &Value) -> Result<u32, Error> {
    match value {
        Value::Int32(v) => Ok(*v as u32),
        Value::UInt32(v) => Ok(*v),
        _ => Err(bad("scoring integer field")),
    }
}
fn register(value: u32) -> Result<u8, Error> {
    if value < math::REGISTERS as u32 {
        Ok(value as u8)
    } else {
        Err(bad("scoring register bound"))
    }
}

fn compile_math(fields: &Fields) -> Result<Calculation, Error> {
    let assembly = field(fields, "Assembly")?
        .as_fields()
        .ok_or_else(|| bad("math assembly"))?;
    let rows = field(assembly, "Instructions")?
        .as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= math::MAX_INSTRUCTIONS)
        .ok_or_else(|| bad("math instruction count"))?;
    let calls = field(assembly, "FunctionCalls")?
        .as_array()
        .filter(|rows| rows.len() <= math::MAX_INSTRUCTIONS)
        .ok_or_else(|| bad("math function count"))?;
    let mut instructions = Vec::with_capacity(rows.len());
    for row in rows {
        let fields = row
            .as_fields()
            .ok_or_else(|| bad("math instruction fields"))?;
        let (code, name) = match field(fields, "Code")? {
            Value::Enum {
                value,
                name: Some(name),
            } => (*value, name.as_str()),
            _ => return Err(bad("math opcode enum")),
        };
        let result = register(bits(field(fields, "Result")?)?)?;
        let p1 = bits(field(fields, "Param1")?)?;
        let p2 = bits(field(fields, "Param2")?)?;
        let operation = match (code, name) {
            (1, "MathOpCode_ConstI") if p2 == 0 => Operation::Integer(p1 as i32),
            (2, "MathOpCode_ConstF") if p2 == 0 => Operation::Float(f32::from_bits(p1)),
            (4, "MathOpCode_InputI") if p2 == 0 => Operation::InputInteger(p1),
            (5, "MathOpCode_InputF") if p2 == 0 => Operation::InputFloat(p1),
            (17, "MathOpCode_LessF") => Operation::LessFloat(register(p1)?, register(p2)?),
            (26, "MathOpCode_AddI") => Operation::AddInteger(register(p1)?, register(p2)?),
            (27, "MathOpCode_AddF") => {
                Operation::BinaryFloat(FloatOp::Add, register(p1)?, register(p2)?)
            }
            (32, "MathOpCode_SubF") => {
                Operation::BinaryFloat(FloatOp::Subtract, register(p1)?, register(p2)?)
            }
            (36, "MathOpCode_MulF") => {
                Operation::BinaryFloat(FloatOp::Multiply, register(p1)?, register(p2)?)
            }
            (46, "MathOpCode_DivF") => {
                Operation::BinaryFloat(FloatOp::Divide, register(p1)?, register(p2)?)
            }
            (61, "MathOpCode_PowF") => {
                Operation::BinaryFloat(FloatOp::Power, register(p1)?, register(p2)?)
            }
            (66, "MathOpCode_Func") => {
                let call = calls
                    .get(p2 as usize)
                    .and_then(Value::as_fields)
                    .ok_or_else(|| bad("math function index"))?;
                let parameters = field(call, "Parameters")?
                    .as_array()
                    .filter(|p| p.len() <= 3)
                    .ok_or_else(|| bad("math function parameter count"))?;
                let args = parameters
                    .iter()
                    .map(|p| register(bits(p)?))
                    .collect::<Result<Vec<_>, _>>()?;
                match (p1, args.as_slice()) {
                    (1_358_875_324, &[a]) => Operation::FloatFromInteger(a),
                    (193_418_252, &[condition, yes, no]) => {
                        Operation::SelectFloat { condition, yes, no }
                    }
                    (194_586_087, &[a]) => Operation::RoundFloat(a),
                    _ => return Err(bad("unsupported math function or signature")),
                }
            }
            (67, "MathOpCode_Return") => Operation::Return(match p1 {
                1 => Register::Boolean(register(p2)?),
                2 => Register::Integer(register(p2)?),
                4 => Register::Float(register(p2)?),
                _ => return Err(bad("unsupported math return type")),
            }),
            _ => return Err(bad("unsupported math instruction")),
        };
        instructions.push(Instruction { result, operation });
    }
    Ok(Calculation::Math {
        program: Program::new(instructions).map_err(|e| bad(&e.to_string()))?,
        evaluate_on_event: boolean(fields, "EvaluateOnEvent")?,
    })
}

fn compile_curve(fields: &Fields) -> Result<Calculation, Error> {
    match field(fields, "CurveEntityTrait")? {
        Value::Enum {
            value: 0,
            name: Some(name),
        } if name == "CURVE_ENTITY_TRAIT_DEFAULT" => {}
        _ => return Err(bad("unsupported scoring curve trait")),
    }
    let rows = field(fields, "ReactionCurve")?
        .as_array()
        .filter(|rows| (4..=MAX_KNOTS + 2).contains(&rows.len()))
        .ok_or_else(|| bad("scoring curve count"))?;
    let vectors = rows
        .iter()
        .map(|row| {
            let f = row.as_fields().ok_or_else(|| bad("scoring curve vector"))?;
            Ok([scalar(f, "x")?, scalar(f, "y")?, scalar(f, "z")?])
        })
        .collect::<Result<Vec<_>, Error>>()?;
    if vectors[1][2] != (vectors.len() - 2) as f32 {
        return Err(bad("scoring curve knot count mismatch"));
    }
    let interpolation = if vectors[0][2] > f32::from_bits(0x3a83_126f) {
        Interpolation::Linear
    } else {
        Interpolation::Cubic
    };
    let curve = Curve::new(
        [vectors[0][0], vectors[1][0]],
        [vectors[0][1], vectors[1][1]],
        scalar(fields, "InputScale")?,
        scalar(fields, "OutputScale")?,
        interpolation,
        vectors[2..]
            .iter()
            .map(|v| Knot {
                x: v[0],
                y: v[1],
                coefficient: v[2],
            })
            .collect(),
    )
    .map_err(|e| bad(&e.to_string()))?;
    Ok(Calculation::Curve {
        curve,
        apply: boolean(fields, "ApplyReactionCurve")?,
        default_input: scalar(fields, "Input")?,
    })
}

/// Compile one explicitly supported object. Unknown classes, enum values and
/// function signatures fail instead of yielding a default score.
pub fn compile(object: &Object) -> Result<Calculation, Error> {
    match object.class.as_str() {
        "MathEntityData" => compile_math(&object.fields),
        "CurveEntityData" => compile_curve(&object.fields),
        _ => Err(bad("unsupported scoring calculation class")),
    }
}

impl AssetIndex {
    /// Read all scalar calculations in a bounded installed composition, retaining
    /// instance identities. Other node classes are outside this method's scope;
    /// success does not validate the whole graph or activate its lifecycle.
    pub fn scoring_calculations(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter(|n| {
                matches!(
                    n.object.class.as_str(),
                    "MathEntityData" | "CurveEntityData"
                )
            })
            .map(|n| {
                Ok(Node {
                    calculation: compile(&n.object).map_err(|error| {
                        bad(&format!("{}:{}: {error}", n.site.key.0, n.site.key.1))
                    })?,
                    asset: n.site.key.0,
                    index: n.site.key.1,
                    instances: n.site.parents.into_iter().map(|f| f.reference).collect(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
