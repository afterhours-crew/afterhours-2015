// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::Invitations;
use crate::{
    Error,
    graph::{Graph, Site, Source, bad, bits32, scenes::SceneLookup},
    scan::Assets,
};
use nfs_frostbite::ebx::Value;

const AVAILABILITY: u32 = (-1_704_961_356i32) as u32;
const OUT: u32 = 193_453_899;
const SET_TRUE: u32 = (-1_541_066_415i32) as u32;
const SET_FALSE: u32 = 668_205_626;
const GREATER: u32 = 193_445_336;
const LESS_OR_EQUAL: u32 = 2_088_725_063;

fn source<S: Source>(graph: &mut Graph<S>, root: Site) -> Result<(Site, usize), Error> {
    // The interface is a separate blueprint field, not a constructed entity in
    // Objects. Resolve that pointer without changing scene serialization order.
    let interface = Site {
        key: graph.target_field(&root, "Interface")?,
        ..root
    };
    if graph.object(&interface)?.class != "InterfaceDescriptorData" {
        return Err(bad("invitation scene interface"));
    }
    let outputs = graph.property_sources(&interface, AVAILABILITY)?;
    let [output] = outputs.as_slice() else {
        return Err(bad("invitation availability output count"));
    };
    let and = graph.object(&output.site)?;
    if output.port != OUT || and.class != "AndEntityData" {
        return Err(bad("invitation availability conjunction"));
    }
    let synced = graph.input(&output.site, 193_450_867)?;
    if synced.port != OUT || graph.object(&synced.site)?.class != "SyncedBoolEntityData" {
        return Err(bad("invitation availability synchronized input"));
    }
    let on = graph.event_sources(&synced.site, SET_TRUE)?;
    let off = graph.event_sources(&synced.site, SET_FALSE)?;
    let [(compare, GREATER)] = on.as_slice() else {
        return Err(bad("invitation count greater event"));
    };
    if off != [(compare.clone(), LESS_OR_EQUAL)] {
        return Err(bad("invitation count complementary event"));
    }
    let comparison = graph.object(compare)?;
    if comparison.class != "CompareIntEntityData"
        || comparison.fields.get("Realm").and_then(Value::as_enum_name) != Some("Realm_Server")
        || comparison
            .fields
            .get("TriggerOnStart")
            .and_then(Value::as_bool)
            != Some(true)
        || comparison
            .fields
            .get("TriggerOnPropertyChange")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(bad("invitation count comparison"));
    }
    let count_above = bits32(
        comparison
            .fields
            .get("B")
            .ok_or_else(|| bad("invitation threshold"))?,
    )?;
    if count_above > 16 {
        return Err(bad("invitation threshold bound"));
    }
    let count = graph.input(compare, 177_636)?;
    if count.port != (-422_150_567i32) as u32
        || graph.object(&count.site)?.class != "ParticipantSetCounterEntityData"
    {
        return Err(bad("invitation participant count source"));
    }
    Ok((synced.site, count_above as usize))
}

pub(super) fn extract(
    graph: &mut Graph<Assets>,
    scenes: &mut SceneLookup<'_>,
) -> Result<Invitations, Error> {
    let root = scenes
        .roots
        .get("levels/genesis01/events")
        .ok_or_else(|| bad("missing invitation scene"))?
        .clone();
    let (site, count_above) = source(graph, root)?;
    let (scene_content_key, serializer) = scenes.endpoint(graph, &site)?;
    Ok(Invitations {
        scene_content_key,
        serializer,
        count_above,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, local};
    use nfs_frostbite::ebx::Fields;

    fn fields(items: &[(&str, Value)]) -> Value {
        Value::Struct(Fields(
            items
                .iter()
                .map(|(k, v)| ((*k).into(), v.clone()))
                .collect(),
        ))
    }
    fn fixture(threshold: u32, complementary: bool) -> Memory {
        let mut m = Memory::default();
        let property = |from, output, to, input| {
            fields(&[
                ("Source", local(from)),
                ("Target", local(to)),
                ("SourceFieldId", Value::UInt32(output)),
                ("TargetFieldId", Value::UInt32(input)),
            ])
        };
        let event = |output, input| {
            fields(&[
                ("Source", local(4)),
                ("Target", local(3)),
                ("SourceEvent", fields(&[("Id", Value::UInt32(output))])),
                ("TargetEvent", fields(&[("Id", Value::UInt32(input))])),
            ])
        };
        let mut events = vec![event(GREATER, SET_TRUE)];
        if complementary {
            events.push(event(LESS_OR_EQUAL, SET_FALSE));
        }
        m.put(
            "a",
            0,
            "Blueprint",
            &[
                ("Interface", local(1)),
                (
                    "PropertyConnections",
                    Value::Array(vec![
                        property(2, OUT, 1, AVAILABILITY),
                        property(3, OUT, 2, 193_450_867),
                        property(5, (-422_150_567i32) as u32, 4, 177_636),
                    ]),
                ),
                ("EventConnections", Value::Array(events)),
            ],
        );
        m.put("a", 1, "InterfaceDescriptorData", &[]);
        m.put("a", 2, "AndEntityData", &[]);
        m.put("a", 3, "SyncedBoolEntityData", &[]);
        m.put(
            "a",
            4,
            "CompareIntEntityData",
            &[
                ("B", Value::UInt32(threshold)),
                (
                    "Realm",
                    Value::Enum {
                        value: 0,
                        name: Some("Realm_Server".into()),
                    },
                ),
                ("TriggerOnStart", Value::Bool(true)),
                ("TriggerOnPropertyChange", Value::Bool(true)),
            ],
        );
        m.put("a", 5, "ParticipantSetCounterEntityData", &[]);
        m
    }
    fn read(m: &mut Memory) -> Result<(Site, usize), Error> {
        source(&mut Graph::new(m), Site::root("a"))
    }
    #[test]
    fn follows_both_count_events_and_preserves_authored_threshold() {
        for threshold in [1, 3] {
            let (site, actual) = read(&mut fixture(threshold, true)).unwrap();
            assert_eq!(site.key, ("a".into(), 3));
            assert_eq!(actual, threshold as usize);
        }
        assert!(read(&mut fixture(1, false)).is_err());
        assert!(read(&mut fixture(u32::MAX, true)).is_err());
        let mut wrong = fixture(1, true);
        wrong.put("a", 5, "FloatEntityData", &[]);
        assert!(read(&mut wrong).is_err());
    }
}
