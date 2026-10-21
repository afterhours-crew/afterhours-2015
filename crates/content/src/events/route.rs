// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    Error,
    graph::{Graph, Node, Site, Source, bad},
};

pub(super) fn drift(nodes: &[Node]) -> Result<Option<bool>, Error> {
    let mut commands = nodes
        .iter()
        .filter(|n| n.object.class == "WaypointChannelGotoWaypointCommandEntityData");
    let Some(command) = commands.next() else {
        return Ok(None);
    };
    if commands.next().is_some() {
        return Err(bad("ambiguous race driving command"));
    }
    command
        .object
        .fields
        .get("DoDrift")
        .and_then(nfs_frostbite::ebx::Value::as_bool)
        .map(Some)
        .ok_or_else(|| bad("race driving command DoDrift"))
}

/// A volume waypoint's normal locator, or its explicitly linked continuation
/// locator. The latter supplies the final AI destination past the finish line;
/// the checkpoint volume and its notification endpoint stay unchanged.
pub(super) fn point<S: Source>(g: &mut Graph<S>, waypoint: &Site) -> Result<[f32; 3], Error> {
    let continuation = g.links(waypoint, (-1_550_420_368i32) as u32)?;
    if continuation.is_empty() {
        return checkpoint(g, waypoint);
    }
    locator(g, &continuation)
}

/// Pacing measures progress to the ordinary waypoint locator. Its continuation
/// is an AI driving destination, not the finish position used for distance.
pub(super) fn checkpoint<S: Source>(g: &mut Graph<S>, waypoint: &Site) -> Result<[f32; 3], Error> {
    let links = g.links(waypoint, 1_975_932_280)?;
    locator(g, &links)
}

fn locator<S: Source>(
    g: &mut Graph<S>,
    links: &[crate::graph::Endpoint],
) -> Result<[f32; 3], Error> {
    let [locator] = links else {
        return Err(bad("checkpoint locator count"));
    };
    if locator.port != 0 || g.object(&locator.site)?.class != super::LOCATOR {
        return Err(bad("checkpoint locator endpoint"));
    }
    Ok(super::spatial::location(g, &locator.site)?.position)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local};
    use nfs_frostbite::ebx::{Fields, Value};

    fn vector(v: [f32; 3]) -> Value {
        Value::Struct(Fields(
            ["x", "y", "z"]
                .into_iter()
                .zip(v)
                .map(|(k, v)| (k.into(), Value::Float32(v)))
                .collect(),
        ))
    }
    #[test]
    fn explicit_continuation_changes_only_the_ai_destination_and_is_unambiguous() {
        let mut m = Memory::default();
        let transform = |x| {
            Value::Struct(Fields(vec![
                ("right".into(), vector([1., 0., 0.])),
                ("up".into(), vector([0., 1., 0.])),
                ("forward".into(), vector([0., 0., 1.])),
                ("trans".into(), vector([x, 0., 0.])),
            ]))
        };
        m.put("a", 1, super::super::CHECKPOINT, &[]);
        m.put(
            "a",
            2,
            super::super::LOCATOR,
            &[("Transform", transform(10.))],
        );
        m.put(
            "a",
            3,
            super::super::LOCATOR,
            &[("Transform", transform(20.))],
        );
        let site = Site {
            key: ("a".into(), 1),
            ..Site::root("a")
        };
        let ordinary = edge(local(1), 1_975_932_280, local(2), 0);
        let extra = edge(local(1), (-1_550_420_368i32) as u32, local(3), 0);
        for (links, expected) in [
            (vec![ordinary.clone()], Some([10., 0., 0.])),
            (vec![ordinary.clone(), extra.clone()], Some([20., 0., 0.])),
            (
                vec![
                    ordinary,
                    extra.clone(),
                    edge(local(1), (-1_550_420_368i32) as u32, local(2), 0),
                ],
                None,
            ),
            (vec![], None),
        ] {
            m.put(
                "a",
                0,
                "Blueprint",
                &[("LinkConnections", Value::Array(links))],
            );
            assert_eq!(point(&mut Graph::new(&mut m), &site).ok(), expected);
            if expected == Some([20., 0., 0.]) {
                assert_eq!(
                    checkpoint(&mut Graph::new(&mut m), &site).unwrap(),
                    [10., 0., 0.]
                );
            }
        }
    }
}
