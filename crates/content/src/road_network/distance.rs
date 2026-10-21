// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::{float, internal, point};
use crate::{Error, graph::bad};
use nfs_frostbite::ebx::{Fields, Object, Value};
use nfs_world::road_distance::{GRID_SIZE, Lane, Network, Spline};
use std::collections::BTreeMap;

fn array<'a>(fields: &'a Fields, name: &str) -> Result<&'a [Value], Error> {
    fields
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| bad("road distance array"))
}
fn fields<'a>(value: Option<&'a Value>, what: &str) -> Result<&'a Fields, Error> {
    value.and_then(Value::as_fields).ok_or_else(|| bad(what))
}
fn spline(object: &Object) -> Result<Spline, Error> {
    let data = fields(object.fields.get("Spline"), "road distance spline")?;
    let form = fields(data.get("MatrixFormData"), "road spline matrix form")?;
    let matrix = fields(form.get("Matrix"), "road spline matrix")?;
    let base = fields(form.get("BaseData"), "road spline controls")?;
    let mut controls = [[0.; 3]; 4];
    for (i, p) in controls.iter_mut().enumerate() {
        *p = point(
            base.get(&format!("P{i}"))
                .ok_or_else(|| bad("road spline control"))?,
        )?;
    }
    let mut coefficients = [[0.; 3]; 4];
    for (row, values) in coefficients.iter_mut().enumerate() {
        for (column, value) in values.iter_mut().enumerate() {
            *value = float(matrix.get(&format!("m{}{}", row + 1, column + 1)))?;
        }
    }
    Ok(Spline {
        controls,
        coefficients,
        points: array(data, "SegmentPoints")?
            .iter()
            .map(point)
            .collect::<Result<_, _>>()?,
        end_times: array(data, "SegmentEndTime")?
            .iter()
            .map(|v| float(Some(v)))
            .collect::<Result<_, _>>()?,
        end_distances: array(data, "SegmentEndDistance")?
            .iter()
            .map(|v| float(Some(v)))
            .collect::<Result<_, _>>()?,
    })
}
fn references(
    fields: &Fields,
    name: &str,
    map: &BTreeMap<usize, usize>,
) -> Result<Vec<usize>, Error> {
    array(fields, name)?
        .iter()
        .map(|value| {
            let object = internal(Some(value), "road distance internal reference")?;
            map.get(&object)
                .copied()
                .ok_or_else(|| bad("road distance reference target"))
        })
        .collect()
}

/// Authored grid, spline samples and arc-distance tables for the race navigation
/// query. LaneDirection is deliberately irrelevant to this distance query.
pub fn parse(objects: &[Object]) -> Result<Network, Error> {
    let mut roots = objects
        .iter()
        .filter(|o| o.class == "RoadNetworkEntityData");
    let root = roots
        .next()
        .ok_or_else(|| bad("road network entity missing"))?;
    if roots.next().is_some() {
        return Err(bad("ambiguous road network"));
    }
    let mut node_of = BTreeMap::new();
    for value in array(&root.fields, "RoadNodes")? {
        let object = internal(Some(value), "road distance node")?;
        if objects.get(object).is_none_or(|o| o.class != "RoadNode") {
            return Err(bad("road distance node type"));
        }
        if node_of.insert(object, node_of.len()).is_some() {
            return Err(bad("duplicate road distance node"));
        }
    }
    let mut splines = Vec::new();
    let mut segment_of = BTreeMap::new();
    for (i, object) in objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.class == "LaneSegment")
    {
        segment_of.insert(i, splines.len());
        splines.push(spline(object)?);
    }
    let mut lanes = Vec::new();
    for value in array(&root.fields, "RoadLanes")? {
        let index = internal(Some(value), "road distance lane")?;
        let object = objects
            .get(index)
            .filter(|o| o.class == "Lane")
            .ok_or_else(|| bad("road distance lane type"))?;
        let endpoint = |name: &str| -> Result<usize, Error> {
            let index = internal(object.fields.get(name), "road distance endpoint")?;
            node_of
                .get(&index)
                .copied()
                .ok_or_else(|| bad("road distance endpoint target"))
        };
        let layer = match object.fields.get("LaneLayer") {
            Some(Value::Enum { value, .. }) => {
                u16::try_from(*value).map_err(|_| bad("road lane layer"))?
            }
            _ => return Err(bad("road lane layer")),
        };
        let segments = references(&object.fields, "Segments", &segment_of)?;
        for value in array(&object.fields, "Segments")? {
            let segment = &objects[internal(Some(value), "lane segment")?];
            if internal(segment.fields.get("Lane"), "segment parent lane")? != index {
                return Err(bad("segment parent lane mismatch"));
            }
        }
        lanes.push(Lane {
            start: endpoint("Start")?,
            end: endpoint("End")?,
            segments,
            layer,
        });
    }
    let grid = fields(root.fields.get("Grid"), "road distance grid")?;
    let x = array(grid, "X")?;
    if x.len() != GRID_SIZE {
        return Err(bad("road grid width"));
    }
    let mut grid = Vec::with_capacity(GRID_SIZE);
    for column in x {
        let column = fields(Some(column), "road grid column")?;
        let z = array(column, "Z")?;
        if z.len() != GRID_SIZE {
            return Err(bad("road grid height"));
        }
        let mut cells = Vec::with_capacity(GRID_SIZE);
        for cell in z {
            cells.push(references(
                fields(Some(cell), "road grid cell")?,
                "LaneSegments",
                &segment_of,
            )?);
        }
        grid.push(cells);
    }
    Network::new(node_of.len(), lanes, splines, grid)
        .map_err(|e| bad(&format!("road distance: {e}")))
}
