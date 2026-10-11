// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Branch, BranchState, parse_error};
use crate::Failure;
use nfs_content::progression::entities as data;
pub use nfs_content::progression::entities::{Blueprint, Role};
use nfs_protocol::world::rpc::Serial;
use nfs_world::replication::entity::{
    Body, Initial, Rpc, Update,
    creation::{Content, Creation, Prefix},
};
use serde_json::Value as Json;
use std::collections::BTreeSet;
pub const SUB_ID: u32 = 1;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneEndpoint {
    pub ghost: u16,
    pub selector: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Construction {
    definition: data::Construction,
}
pub fn content(v: &Json) -> Result<Content, Failure> {
    data::content(v).map_err(parse_error)
}
impl Construction {
    pub(crate) fn from_definition(definition: data::Construction) -> Self {
        Self { definition }
    }
    pub fn from_json(v: &Json) -> Result<Self, Failure> {
        Ok(Self::from_definition(
            data::Construction::from_json(v).map_err(parse_error)?,
        ))
    }
    pub fn content(&self) -> &Content {
        self.definition.content()
    }
    pub fn blueprint(&self, branch: Branch) -> Option<&Blueprint> {
        self.definition.blueprint(branch)
    }
    pub fn check_objectives(
        &self,
        branch: Branch,
        expected: &BTreeSet<u64>,
    ) -> Result<(), Failure> {
        self.definition
            .check_objectives(branch, expected)
            .map_err(parse_error)
    }
    pub fn build(
        &self,
        branch: Branch,
        state: &BranchState,
        participant: u16,
        blueprint_scene: u16,
        scene: impl Fn(u32, usize) -> Option<SceneEndpoint>,
    ) -> Result<Creation, Failure> {
        let blueprint = self.blueprint(branch).ok_or(Failure::ProfileConfig)?;
        if participant == 0 || blueprint_scene == 0 {
            return Err(Failure::ProfileConfig);
        }
        let serial = Serial::new(0)
            .ok_or(Failure::ProfileConfig)?
            .next_initialization();
        let mut selector = 0u16;
        let mut initial = Vec::with_capacity(blueprint.roles().len());
        let mut updates = Vec::with_capacity(blueprint.roles().len());
        for role in blueprint.roles() {
            let mut rpc = || {
                let current = Rpc { selector, serial };
                selector = selector.checked_add(1).ok_or(Failure::BodyLimit)?;
                Ok::<_, Failure>(current)
            };
            let (value, update) = match role {
                Role::Root => (
                    Initial::Root {
                        value: blueprint.bus_count(),
                        rpc: rpc()?,
                        reference: participant,
                    },
                    Update::Noop,
                ),
                Role::Channel {
                    scene_content_key,
                    scene_serializer,
                } => {
                    let endpoint = scene(*scene_content_key, *scene_serializer)
                        .ok_or(Failure::ProfileConfig)?;
                    if endpoint.ghost == 0 {
                        return Err(Failure::ProfileConfig);
                    }
                    (
                        Initial::RpcReference {
                            rpc: rpc()?,
                            reference: endpoint.ghost,
                            target_selector: endpoint.selector,
                        },
                        Update::Noop,
                    )
                }
                Role::Objective { id } => {
                    let objective = state.objectives().get(id).ok_or(Failure::ProfileConfig)?;
                    (
                        Initial::RpcFlags {
                            rpc: rpc()?,
                            flags: [objective.active, objective.completed],
                        },
                        Update::Noop,
                    )
                }
                Role::Rpc | Role::Launcher => (Initial::Rpc(rpc()?), Update::Noop),
                Role::Bool => (Initial::BoolProperty(None), Update::BoolProperty(None)),
                Role::Int => (Initial::I32Property(None), Update::I32Property(None)),
            };
            initial.push(value);
            updates.push(Some(update));
        }
        let creation = Creation {
            prefix: Prefix {
                parent: None,
                blueprint: blueprint_scene,
                sub_id: SUB_ID,
                owner: None,
                asset: blueprint.asset(),
            },
            body: Body {
                initial: Some(initial),
                updates,
            },
        };
        creation
            .encode(self.content())
            .map_err(|_| Failure::BodyLimit)?;
        Ok(creation)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    const PROGRESSION_SCENE: u32 = 105;
    use crate::progression::{Catalog, Settings, tests as restore_tests};
    use nfs_world::replication::{Initial as RecordInitial, players::Players};
    use serde_json::json;
    pub(crate) fn construction_json() -> Json {
        json!({
            "content":{"catalogs":[{"bundle":3,"entries":[[10,2],[12,1]]}],
                "profiles":[
                    {"asset":{"bundle":3,"type_id":10,"local_index":1},"serializers":["root","rpc_reference","rpc_flags","bool_property","i32_property"]},
                    {"asset":{"bundle":3,"type_id":12,"local_index":0},"serializers":["root","rpc_flags","rpc_flags","rpc"]}]},
            "branches":[
                {"name":"main","asset":{"bundle":3,"type_id":10,"local_index":1},"bus_count":169,
                 "roles":[{"kind":"root"},{"kind":"channel","scene_content_key":PROGRESSION_SCENE,"scene_serializer":2},{"kind":"objective","id":30},{"kind":"bool"},{"kind":"int"}]},
                {"name":"intro","asset":{"bundle":3,"type_id":12,"local_index":0},"bus_count":11,
                 "roles":[{"kind":"root"},{"kind":"objective","id":20},{"kind":"objective","id":10},{"kind":"launcher"}]}]
        })
    }
    fn restored() -> crate::progression::Restored {
        let (_, _, snapshot) = restore_tests::local();
        Catalog::from_json(&restore_tests::content())
            .unwrap()
            .restore(
                &restore_tests::tables().view(&snapshot).unwrap(),
                Settings {
                    consumer_beta: false,
                },
            )
            .unwrap()
    }
    #[test]
    fn construction_validates_roles_against_static_profiles_and_catalog_objectives() {
        let construction = Construction::from_json(&construction_json()).unwrap();
        let catalog = Catalog::from_json(&restore_tests::content()).unwrap();
        for branch in [Branch::Main, Branch::Intro] {
            construction
                .check_objectives(branch, &catalog.objective_ids(branch))
                .unwrap();
        }
        assert!(
            construction
                .check_objectives(Branch::Main, &BTreeSet::from([30, 31]))
                .is_err()
        );
        for case in 0..8 {
            let mut v = construction_json();
            match case {
                0 => v["branches"][0]["roles"][2] = json!({"kind":"rpc"}),
                1 => {
                    v["branches"][0]["roles"][1] =
                        json!({"kind":"channel","scene_content_key":0,"scene_serializer":2})
                }
                2 => v["branches"][1]["roles"][2] = json!({"kind":"objective","id":20}),
                3 => v["branches"][0]["bus_count"] = json!(70000),
                4 => v["branches"][1]["name"] = json!("main"),
                5 => v["branches"][0]["roles"] = json!([{"kind":"root"}]),
                6 => v["branches"][0]["asset"]["local_index"] = json!(5),
                _ => v["branches"][0]["roles"][0] = json!({"kind":"root","value":169}),
            }
            assert!(Construction::from_json(&v).is_err(), "case{case}");
        }
    }
    #[test]
    fn built_creations_use_current_references_sequential_selectors_and_first_serial() {
        let construction = Construction::from_json(&construction_json()).unwrap();
        let restored = restored();
        let scene = |key: u32, index: usize| {
            (key == PROGRESSION_SCENE && index == 2).then_some(SceneEndpoint {
                ghost: 7,
                selector: 1,
            })
        };
        let main = construction
            .build(Branch::Main, restored.branch(Branch::Main), 9, 7, scene)
            .unwrap();
        assert_eq!(main.prefix.blueprint, 7);
        assert_eq!(main.prefix.sub_id, SUB_ID);
        assert!(main.prefix.parent.is_none() && main.prefix.owner.is_none());
        let initial = main.body.initial.as_ref().unwrap();
        let serial = Serial::new(1).unwrap();
        assert_eq!(
            initial[0],
            Initial::Root {
                value: 169,
                rpc: Rpc {
                    selector: 0,
                    serial
                },
                reference: 9
            }
        );
        assert_eq!(
            initial[1],
            Initial::RpcReference {
                rpc: Rpc {
                    selector: 1,
                    serial
                },
                reference: 7,
                target_selector: 1
            }
        );
        assert_eq!(
            initial[2],
            Initial::RpcFlags {
                rpc: Rpc {
                    selector: 2,
                    serial
                },
                flags: [false, false]
            }
        );
        assert_eq!(initial[3], Initial::BoolProperty(None));
        assert_eq!(initial[4], Initial::I32Property(None));
        assert_eq!(main.body.updates.len(), 5);
        let intro = construction
            .build(Branch::Intro, restored.branch(Branch::Intro), 9, 7, scene)
            .unwrap();
        let initial = intro.body.initial.as_ref().unwrap();
        assert!(matches!(initial[0], Initial::Root { value: 11, .. }));
        assert!(matches!(
            initial[1],
            Initial::RpcFlags {
                flags: [false, false],
                ..
            }
        ));
        assert!(matches!(
            initial[2],
            Initial::RpcFlags {
                flags: [true, false],
                ..
            }
        ));
        assert!(matches!(initial[3], Initial::Rpc(Rpc { selector: 3, .. })));
        assert!(
            construction
                .build(Branch::Main, restored.branch(Branch::Main), 9, 7, |_, _| {
                    None
                })
                .is_err()
        );
        assert!(
            construction
                .build(Branch::Main, restored.branch(Branch::Main), 0, 7, scene)
                .is_err()
        );
        assert!(
            construction
                .build(Branch::Main, restored.branch(Branch::Intro), 9, 7, scene)
                .is_err()
        );
        let mut players = Players::default();
        let creations = (1..=7)
            .map(|level| {
                use nfs_world::replication::sublevel::{Kind, Profile};
                let profile = Profile::new(false, &[Kind::Noop, Kind::Rpc, Kind::Rpc]).unwrap();
                let content = nfs_world::replication::sublevel::Content::new(vec![(
                    PROGRESSION_SCENE + u32::from(level) - 7,
                    profile.clone(),
                )])
                .unwrap();
                (level, content)
            })
            .collect::<Vec<_>>();
        for (level, content) in &creations {
            use nfs_world::replication::sublevel::ordinary;
            let key = PROGRESSION_SCENE + u32::from(*level) - 7;
            let creation = ordinary::creation(
                *level,
                key,
                None,
                Serial::new(0).unwrap(),
                &ordinary::unpopulated(content.profile(key).unwrap()).unwrap(),
                content,
            )
            .unwrap();
            players
                .create_scenes(&[*level], vec![creation], content)
                .unwrap();
        }
        assert_eq!(players.objects().scene(PROGRESSION_SCENE), Some(7));
        assert_eq!(
            players.objects().scene_endpoint(PROGRESSION_SCENE, 2),
            Some((7, 1))
        );
        let player = players
            .create(
                1,
                5,
                nfs_world::replication::players::Request {
                    name: b"Local".to_vec(),
                    flag: false,
                    slot: 0,
                },
            )
            .unwrap()
            .unwrap();
        let participant = players.join(1, 5, player.id).unwrap().unwrap();
        assert_eq!(participant.id, 9);
        let records = players
            .spawn_entities(vec![main, intro], construction.content())
            .unwrap();
        assert_eq!(records.iter().map(|r| r.id).collect::<Vec<_>>(), [10, 11]);
        assert!(matches!(
            &records[0].initial,
            Some(RecordInitial::Entity { .. })
        ));
        assert_eq!(players.objects().len(), 11);
        let bad = construction
            .build(Branch::Main, restored.branch(Branch::Main), 9, 7, |_, _| {
                Some(SceneEndpoint {
                    ghost: 99,
                    selector: 1,
                })
            })
            .unwrap();
        assert!(
            players
                .spawn_entities(vec![bad], construction.content())
                .is_err()
        );
        assert_eq!(players.objects().len(), 11);
    }
}
