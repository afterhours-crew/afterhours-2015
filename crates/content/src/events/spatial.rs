// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};
use nfs_frostbite::ebx::Value;

fn vector(v: &Value) -> Result<[f32; 3], Error> {
    let f = v.as_fields().ok_or_else(|| bad("event locator vector"))?;
    let mut out = [0.; 3];
    for (i, k) in ["x", "y", "z"].iter().enumerate() {
        out[i] = match f.get(k) {
            Some(Value::Float32(v)) if v.is_finite() && v.abs() <= 1_000_000. => *v,
            _ => return Err(bad("event locator coordinate")),
        };
    }
    Ok(out)
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transform {
    pub(crate) basis: [[f32; 3]; 3],
    pub(crate) position: [f32; 3],
}
impl Transform {
    fn read(v: &Value) -> Result<Self, Error> {
        let f = v
            .as_fields()
            .ok_or_else(|| bad("event locator transform"))?;
        let get = |k| {
            vector(
                f.get(k)
                    .ok_or_else(|| bad("event locator transform field"))?,
            )
        };
        let value = Self {
            basis: [get("right")?, get("up")?, get("forward")?],
            position: get("trans")?,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), Error> {
        nfs_world::replication::vehicle::orientation::quaternion_from_basis(self.basis)
            .map_err(|_| bad("event locator orientation"))?;
        if self
            .position
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
        {
            return Err(bad("event locator world position"));
        }
        Ok(())
    }
    fn direction(&self, v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| {
            self.basis[0][i] * v[0] + self.basis[1][i] * v[1] + self.basis[2][i] * v[2]
        })
    }
    fn compose(self, child: Self) -> Result<Self, Error> {
        let out = Self {
            basis: child.basis.map(|v| self.direction(v)),
            // Preserve the native transform's f32 accumulation order: parent
            // translation joins the x contribution before the y and z terms.
            position: std::array::from_fn(|i| {
                ((self.basis[0][i] * child.position[0] + self.position[i])
                    + self.basis[1][i] * child.position[1])
                    + self.basis[2][i] * child.position[2]
            }),
        };
        out.validate()?;
        Ok(out)
    }
}

pub(crate) fn location<S: Source>(g: &mut Graph<S>, site: &Site) -> Result<Transform, Error> {
    let o = g.object(site)?;
    let mut value = Transform::read(
        o.fields
            .get("Transform")
            .ok_or_else(|| bad("event locator transform"))?,
    )?;
    for frame in site.parents.iter().rev() {
        let reference = g.source.object(&frame.reference)?;
        if let Some(transform) = reference.fields.get("BlueprintTransform") {
            value = Transform::read(transform)?.compose(value)?;
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_translation_preserves_native_single_precision_accumulation() {
        let c = std::f32::consts::FRAC_1_SQRT_2;
        let parent = Transform {
            basis: [[c, 0., -c], [0., 1., 0.], [c, 0., c]],
            position: [524_288., 0., 0.],
        };
        let child = Transform {
            basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            position: [1., 0., -1.],
        };
        let world = parent.compose(child).unwrap();
        // Combining the x/z offsets before adding the translation gives
        // 524288 instead. Each native add rounds in single precision.
        assert_eq!(world.position[0], 524_287.97);
        assert_eq!(world.basis, parent.basis);
    }
}
