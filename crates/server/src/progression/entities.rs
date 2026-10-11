// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Branch, BranchState};
use crate::Failure;
use nfs_protocol::world::rpc::Serial;
use nfs_world::replication::entity::{
    Body, Initial, Kind, MAX_SERIALIZERS, Profile, Rpc, Update,
    creation::{Asset, Catalog, Content, Creation, MAX_CATALOGS, MAX_PROFILES, MAX_TYPES, Prefix},
};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

pub const SUB_ID: u32 = 1;
const MAX_SELECTORS: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Root,
    Channel {
        scene_content_key: u32,
        scene_serializer: usize,
    },
    Objective {
        id: u64,
    },
    Rpc,
    Launcher,
    Bool,
    Int,
}
impl Role {
    fn kind(self) -> Kind {
        match self {
            Self::Root => Kind::Root,
            Self::Channel { .. } => Kind::RpcReference,
            Self::Objective { .. } => Kind::RpcFlags,
            Self::Rpc | Self::Launcher => Kind::Rpc,
            Self::Bool => Kind::BoolProperty,
            Self::Int => Kind::I32Property,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Blueprint {
    asset: Asset,
    bus_count: u16,
    roles: Vec<Role>,
}
impl Blueprint {
    pub fn asset(&self) -> Asset {
        self.asset
    }
    pub fn bus_count(&self) -> u16 {
        self.bus_count
    }
    pub fn roles(&self) -> &[Role] {
        &self.roles
    }
    pub fn objective_ids(&self) -> BTreeSet<u64> {
        self.roles
            .iter()
            .filter_map(|role| match role {
                Role::Objective { id } => Some(*id),
                _ => None,
            })
            .collect()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneEndpoint {
    pub ghost: u16,
    pub selector: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Construction {
    content: Content,
    branches: BTreeMap<Branch, Blueprint>,
}

fn fields(v: &Json, names: &[&str]) -> Result<(), Failure> {
    let o = v.as_object().ok_or(Failure::ProfileConfig)?;
    if o.len() != names.len() || !names.iter().all(|n| o.contains_key(*n)) {
        return Err(Failure::ProfileConfig);
    }
    Ok(())
}
fn word(v: &Json) -> Result<u16, Failure> {
    u16::try_from(v.as_u64().ok_or(Failure::ProfileConfig)?).map_err(|_| Failure::ProfileConfig)
}
fn asset(v: &Json) -> Result<Asset, Failure> {
    fields(v, &["bundle", "type_id", "local_index"])?;
    Ok(Asset {
        bundle: word(&v["bundle"])?,
        type_id: word(&v["type_id"])?,
        local_index: word(&v["local_index"])?,
    })
}
fn kind(v: &Json) -> Result<Kind, Failure> {
    match v.as_str() {
        Some("root") => Ok(Kind::Root),
        Some("rpc") => Ok(Kind::Rpc),
        Some("rpc_reference") => Ok(Kind::RpcReference),
        Some("rpc_flags") => Ok(Kind::RpcFlags),
        Some("bool_property") => Ok(Kind::BoolProperty),
        Some("i32_property") => Ok(Kind::I32Property),
        Some("rpc_variant") => Ok(Kind::RpcVariant),
        Some("rpc_ghost_reference") => Ok(Kind::RpcGhostReference),
        _ => Err(Failure::ProfileConfig),
    }
}
pub fn content(v: &Json) -> Result<Content, Failure> {
    fields(v, &["catalogs", "profiles"])?;
    let catalogs = v["catalogs"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= MAX_CATALOGS)
        .ok_or(Failure::ProfileConfig)?;
    let profiles = v["profiles"]
        .as_array()
        .filter(|a| a.len() <= MAX_PROFILES)
        .ok_or(Failure::ProfileConfig)?;
    let catalogs = catalogs
        .iter()
        .map(|c| {
            fields(c, &["bundle", "entries"])?;
            let rows = c["entries"]
                .as_array()
                .filter(|a| a.len() <= MAX_TYPES)
                .ok_or(Failure::ProfileConfig)?;
            let entries = rows
                .iter()
                .map(|r| {
                    let pair = r
                        .as_array()
                        .filter(|a| a.len() == 2)
                        .ok_or(Failure::ProfileConfig)?;
                    Ok((word(&pair[0])?, word(&pair[1])?))
                })
                .collect::<Result<Vec<_>, Failure>>()?;
            Catalog::new(word(&c["bundle"])?, &entries).map_err(|_| Failure::ProfileConfig)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let profiles = profiles
        .iter()
        .map(|p| {
            fields(p, &["asset", "serializers"])?;
            let kinds = p["serializers"]
                .as_array()
                .filter(|a| a.len() <= MAX_SERIALIZERS)
                .ok_or(Failure::ProfileConfig)?
                .iter()
                .map(kind)
                .collect::<Result<Vec<_>, _>>()?;
            Ok((
                asset(&p["asset"])?,
                Profile::new(&kinds).map_err(|_| Failure::ProfileConfig)?,
            ))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    Content::new(catalogs, profiles).map_err(|_| Failure::ProfileConfig)
}
fn role(v: &Json) -> Result<Role, Failure> {
    let name = v["kind"].as_str().ok_or(Failure::ProfileConfig)?;
    Ok(match name {
        "root" => {
            fields(v, &["kind"])?;
            Role::Root
        }
        "channel" => {
            fields(v, &["kind", "scene_content_key", "scene_serializer"])?;
            let key = v["scene_content_key"]
                .as_u64()
                .and_then(|k| u32::try_from(k).ok())
                .filter(|k| *k != 0)
                .ok_or(Failure::ProfileConfig)?;
            let index = v["scene_serializer"]
                .as_u64()
                .and_then(|i| usize::try_from(i).ok())
                .filter(|i| (1..MAX_SELECTORS).contains(i))
                .ok_or(Failure::ProfileConfig)?;
            Role::Channel {
                scene_content_key: key,
                scene_serializer: index,
            }
        }
        "objective" => {
            fields(v, &["kind", "id"])?;
            Role::Objective {
                id: v["id"]
                    .as_u64()
                    .filter(|id| *id != 0)
                    .ok_or(Failure::ProfileConfig)?,
            }
        }
        "rpc" => {
            fields(v, &["kind"])?;
            Role::Rpc
        }
        "launcher" => {
            fields(v, &["kind"])?;
            Role::Launcher
        }
        "bool" => {
            fields(v, &["kind"])?;
            Role::Bool
        }
        "int" => {
            fields(v, &["kind"])?;
            Role::Int
        }
        _ => return Err(Failure::ProfileConfig),
    })
}

impl Construction {
    pub fn from_json(v: &Json) -> Result<Self, Failure> {
        fields(v, &["content", "branches"])?;
        let content = content(&v["content"])?;
        let raw = v["branches"]
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or(Failure::ProfileConfig)?;
        let mut branches = BTreeMap::new();
        let mut objective_ids = BTreeSet::new();
        for b in raw {
            fields(b, &["name", "asset", "bus_count", "roles"])?;
            let branch = match b["name"].as_str() {
                Some("main") => Branch::Main,
                Some("intro") => Branch::Intro,
                _ => return Err(Failure::ProfileConfig),
            };
            let asset = asset(&b["asset"])?;
            let profile = content.profile(asset).map_err(|_| Failure::ProfileConfig)?;
            let roles = b["roles"]
                .as_array()
                .filter(|a| a.len() == profile.kinds().len())
                .ok_or(Failure::ProfileConfig)?
                .iter()
                .map(role)
                .collect::<Result<Vec<_>, _>>()?;
            let mut rpcs = 0;
            for (role, kind) in roles.iter().zip(profile.kinds()) {
                if role.kind() != *kind {
                    return Err(Failure::ProfileConfig);
                }
                if !matches!(role, Role::Bool | Role::Int) {
                    rpcs += 1;
                }
                if let Role::Objective { id } = role
                    && !objective_ids.insert(*id)
                {
                    return Err(Failure::ProfileConfig);
                }
            }
            if rpcs > MAX_SELECTORS {
                return Err(Failure::ProfileConfig);
            }
            let blueprint = Blueprint {
                asset,
                bus_count: word(&b["bus_count"])?,
                roles,
            };
            if branches.insert(branch, blueprint).is_some() {
                return Err(Failure::ProfileConfig);
            }
        }
        Ok(Self { content, branches })
    }
    pub fn content(&self) -> &Content {
        &self.content
    }
    pub fn blueprint(&self, branch: Branch) -> Option<&Blueprint> {
        self.branches.get(&branch)
    }
    pub fn check_objectives(
        &self,
        branch: Branch,
        expected: &BTreeSet<u64>,
    ) -> Result<(), Failure> {
        let blueprint = self.branches.get(&branch).ok_or(Failure::ProfileConfig)?;
        if blueprint.objective_ids() != *expected {
            return Err(Failure::ProfileConfig);
        }
        Ok(())
    }
    pub fn build(
        &self,
        branch: Branch,
        state: &BranchState,
        participant: u16,
        blueprint_scene: u16,
        scene: impl Fn(u32, usize) -> Option<SceneEndpoint>,
    ) -> Result<Creation, Failure> {
        let blueprint = self.branches.get(&branch).ok_or(Failure::ProfileConfig)?;
        if participant == 0 || blueprint_scene == 0 {
            return Err(Failure::ProfileConfig);
        }
        let serial = Serial::new(0)
            .ok_or(Failure::ProfileConfig)?
            .next_initialization();
        let mut selector = 0u16;
        let mut initial = Vec::with_capacity(blueprint.roles.len());
        let mut updates = Vec::with_capacity(blueprint.roles.len());
        for role in &blueprint.roles {
            let mut rpc = || {
                let current = Rpc { selector, serial };
                selector = selector.checked_add(1).ok_or(Failure::BodyLimit)?;
                Ok::<_, Failure>(current)
            };
            let (value, update) = match role {
                Role::Root => (
                    Initial::Root {
                        value: blueprint.bus_count,
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
                asset: blueprint.asset,
            },
            body: Body {
                initial: Some(initial),
                updates,
            },
        };
        creation
            .encode(&self.content)
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
