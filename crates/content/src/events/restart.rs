// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};

/// Bound on an authored restart grid, independent of the participant count.
pub const MAX_LOCATIONS: usize = 32;

/// The start area's authored teleport location, including all parent transforms.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Destination {
    pub position: [f32; 3],
    pub basis: [[f32; 3]; 3],
}

pub(crate) fn destinations<S: Source>(
    graph: &mut Graph<S>,
    area: &Site,
) -> Result<Option<Vec<Destination>>, Error> {
    // RestartEventLogic's TeleportLocation port is exposed by this start area.
    // Resolve it out through the instance, preserving any linked variant's context.
    let locations = graph.links(area, (-2_033_632_871i32) as u32)?;
    if locations.is_empty() {
        return Ok(None);
    }
    let [location] = locations.as_slice() else {
        return Err(bad("ambiguous restart teleport location"));
    };
    if location.port != 0 || graph.object(&location.site)?.class != "TeleportLocationEntityData" {
        return Err(bad("restart teleport location type"));
    }
    let locators = graph.links(&location.site, 1_361_382_029)?;
    if locators.is_empty() || locators.len() > MAX_LOCATIONS {
        return Err(bad("restart teleport locator count"));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(locators.len());
    // Preserve the authored link order. The native location rotates an index
    // through the resolved list, rather than choosing the closest locator.
    for locator in locators {
        if locator.port != 0
            || graph.object(&locator.site)?.class != "LocatorEntityData"
            || !seen.insert(locator.site.clone())
        {
            return Err(bad("restart teleport locator type or duplicate"));
        }
        let transform = super::spatial::location(graph, &locator.site)?;
        out.push(Destination {
            position: transform.position,
            basis: transform.basis,
        });
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local};
    use nfs_frostbite::ebx::{Fields, Value};

    #[test]
    fn destinations_preserve_grid_order_and_reject_duplicates_or_wrong_types() {
        let mut source = Memory::default();
        let links = vec![
            edge(local(1), (-2_033_632_871i32) as u32, local(2), 0),
            edge(local(2), 1_361_382_029, local(4), 0),
            edge(local(2), 1_361_382_029, local(3), 0),
        ];
        source.put(
            "a",
            0,
            "Blueprint",
            &[("LinkConnections", Value::Array(links.clone()))],
        );
        source.put("a", 1, "ReferenceObjectData", &[]);
        source.put("a", 2, "TeleportLocationEntityData", &[]);
        let vector = |v: [f32; 3]| {
            Value::Struct(Fields(
                ["x", "y", "z"]
                    .into_iter()
                    .zip(v)
                    .map(|(k, v)| (k.into(), Value::Float32(v)))
                    .collect(),
            ))
        };
        let transform = |pos| {
            Value::Struct(Fields(vec![
                ("right".into(), vector([0., 0., -1.])),
                ("up".into(), vector([0., 1., 0.])),
                ("forward".into(), vector([1., 0., 0.])),
                ("trans".into(), vector(pos)),
            ]))
        };
        source.put(
            "a",
            3,
            "LocatorEntityData",
            &[("Transform", transform([12., 5., -40.]))],
        );
        source.put(
            "a",
            4,
            "LocatorEntityData",
            &[("Transform", transform([2., 5., -40.]))],
        );
        let site = Site {
            key: ("a".into(), 1),
            ..Site::root("a")
        };
        let d = destinations(&mut Graph::new(&mut source), &site)
            .unwrap()
            .unwrap();
        assert_eq!(
            d.iter().map(|d| d.position).collect::<Vec<_>>(),
            [[2., 5., -40.], [12., 5., -40.]]
        );
        assert_eq!(d[1].basis[2], [1., 0., 0.]);
        let mut duplicates = links.clone();
        duplicates.push(edge(local(2), 1_361_382_029, local(3), 0));
        source.put(
            "a",
            0,
            "Blueprint",
            &[("LinkConnections", Value::Array(duplicates))],
        );
        assert!(destinations(&mut Graph::new(&mut source), &site).is_err());
        source.put(
            "a",
            0,
            "Blueprint",
            &[("LinkConnections", Value::Array(links))],
        );
        source.put("a", 4, "OtherEntityData", &[]);
        assert!(destinations(&mut Graph::new(&mut source), &site).is_err());
    }
}
