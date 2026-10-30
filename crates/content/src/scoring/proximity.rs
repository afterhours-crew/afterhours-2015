// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Authored proximity-filter radius fallback (E916) and the participant set
//! the filter reads (E1130). A connected ready property overrides the radius;
//! the installed prefabs connect a Float constant. The `Participants` link
//! resolves through the prefab interfaces to one channel reference, and the
//! channel must declare that link as a participant set.
use super::{events::Identity, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad, bits32},
};
use nfs_frostbite::{
    ebx::{Object, Value},
    name_hash,
};
use nfs_world::scoring::proximity::PARTICIPANTS;

pub const CLASS: &str = "VehicleProximitySetFilterEntityData";
const CHANNEL_REFERENCE: &str = "LinkedSchematicChannelReferenceEntityData";
const CHANNEL: &str = "SchematicChannelAsset";
const PARTICIPANT_SET_TYPE: &[u8] = b"ParticipantSetEntityData";

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub radius: f32,
    /// Channel link of the participant set the filter reads.
    pub set: u32,
}
pub fn compile(object: &Object) -> Result<f32, Error> {
    if object.class != CLASS {
        return Err(bad("unsupported proximity filter class"));
    }
    let radius = scalar(&object.fields, "Radius")?;
    if radius < 0.0 {
        return Err(bad("negative proximity radius"));
    }
    Ok(radius)
}
/// The channel link a filter's `Participants` resolves to, checked against
/// the channel's declared link types.
fn participant_set<S: Source>(graph: &mut Graph<'_, S>, filter: &Site) -> Result<u32, Error> {
    let mut linked = graph.links(filter, PARTICIPANTS)?;
    if linked.len() != 1 {
        return Err(bad("missing or ambiguous proximity participant set"));
    }
    let endpoint = linked.remove(0);
    if graph.object(&endpoint.site)?.class != CHANNEL_REFERENCE {
        return Err(bad("proximity participant set is not a channel link"));
    }
    let channel = Site {
        key: graph.target_field(&endpoint.site, "ChannelAsset")?,
        ..endpoint.site.clone()
    };
    let channel = graph.object(&channel)?;
    if channel.class != CHANNEL {
        return Err(bad("proximity participant set channel class"));
    }
    let set_type = name_hash(PARTICIPANT_SET_TYPE);
    let declared = channel
        .fields
        .get("Links")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("proximity participant set channel links"))?
        .iter()
        .filter_map(Value::as_fields)
        .any(|link| {
            link.get("Id").and_then(|v| bits32(v).ok()) == Some(endpoint.port)
                && link.get("LinkTypeHash").and_then(|v| bits32(v).ok()) == Some(set_type)
        });
    if !declared {
        return Err(bad("proximity participant set is not a declared set link"));
    }
    Ok(endpoint.port)
}
impl AssetIndex {
    pub fn scoring_proximities(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        let mut graph = Graph::new(&mut self.assets);
        let rows = graph.read(Site::root(blueprint))?;
        rows.into_iter()
            .filter(|row| row.object.class == CLASS)
            .map(|row| {
                Ok(Node {
                    identity: Identity::from(&row.site),
                    radius: compile(&row.object)?,
                    set: participant_set(&mut graph, &row.site)?,
                })
            })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::{self, Asset, Field, PartitionWriter, guid};
    use nfs_frostbite::ebx::Fields;
    use nfs_world::scoring::proximity::{DRIFT_PARTICIPANTS, HIGH_SPEED_PARTICIPANTS};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn radius_is_a_finite_nonnegative_float_of_the_declared_class() {
        let mut object = Object {
            class: CLASS.into(),
            guid: None,
            fields: Fields(vec![("Radius".into(), Value::Float32(0.0))]),
        };
        assert_eq!(compile(&object).unwrap(), 0.0);
        object.fields.0[0].1 = Value::Float32(-1.0);
        assert!(compile(&object).is_err());
        object.fields.0[0].1 = Value::Int32(3);
        assert!(compile(&object).is_err());
        object.fields.0[0].1 = Value::Float32(30.0);
        object.class = "ParticipantSetCounterEntityData".into();
        assert!(compile(&object).is_err());
    }

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
    const PREFAB: &str = "synthetic/scoring/drifttrain";
    const CHANNEL_ASSET: &str = "synthetic/scoring/driftparticipantchannel";

    /// A prefab whose filter links `Participants` to a channel reference
    /// (port `set`), and a channel declaring `declared` with `link_type`.
    fn index(set: u32, declared: u32, link_type: &[u8]) -> (std::path::PathBuf, AssetIndex) {
        let channel = (guid(0x51), guid(0x52));
        let prefab = Asset {
            name: PREFAB.into(),
            partition: PartitionWriter::default().build_many(
                guid(0x41),
                "LogicPrefabBlueprint",
                guid(0x42),
                vec![
                    ("Name", Field::CString(PREFAB.into())),
                    ("Objects", Field::RawPointers(vec![2, 3])),
                    (
                        "LinkConnections",
                        Field::Structs(
                            "LinkConnection",
                            vec![vec![
                                ("Source", Field::RawPointer(2)),
                                ("SourceFieldId", Field::UInt(PARTICIPANTS)),
                                ("Target", Field::RawPointer(3)),
                                ("TargetFieldId", Field::UInt(set)),
                            ]],
                        ),
                    ),
                ],
                vec![
                    (CLASS, vec![("Radius", Field::Float(30.0))]),
                    (
                        CHANNEL_REFERENCE,
                        vec![("ChannelAsset", Field::Import(channel.0, channel.1))],
                    ),
                ],
            ),
        };
        let channel = Asset {
            name: CHANNEL_ASSET.into(),
            partition: PartitionWriter::default().build(
                channel.0,
                CHANNEL,
                channel.1,
                vec![
                    ("Name", Field::CString(CHANNEL_ASSET.into())),
                    (
                        "Links",
                        Field::Structs(
                            "SchematicChannelLink",
                            vec![vec![
                                ("Id", Field::Int(declared as i32)),
                                ("LinkTypeHash", Field::Int(name_hash(link_type) as i32)),
                            ]],
                        ),
                    ),
                ],
            ),
        };
        let path = std::env::temp_dir().join(format!(
            "afterhours-proximity-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let (options, _) = synthetic::write(&path, &[prefab, channel]).unwrap();
        (path.clone(), AssetIndex::open(&path, &options).unwrap())
    }

    #[test]
    fn the_filtered_set_is_the_declared_channel_participant_set() {
        let (path, mut index) = index(DRIFT_PARTICIPANTS, DRIFT_PARTICIPANTS, PARTICIPANT_SET_TYPE);
        let nodes = index.scoring_proximities(PREFAB).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].radius, 30.0);
        assert_eq!(nodes[0].set, DRIFT_PARTICIPANTS);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn undeclared_or_mistyped_set_links_are_rejected() {
        for (set, declared, link_type) in [
            (
                DRIFT_PARTICIPANTS,
                HIGH_SPEED_PARTICIPANTS,
                PARTICIPANT_SET_TYPE,
            ),
            (
                DRIFT_PARTICIPANTS,
                DRIFT_PARTICIPANTS,
                &b"BoolParticipantPropertyEntityData"[..],
            ),
        ] {
            let (path, mut index) = index(set, declared, link_type);
            assert!(index.scoring_proximities(PREFAB).is_err());
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}
