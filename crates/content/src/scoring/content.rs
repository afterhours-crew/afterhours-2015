// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Everything the server needs to run the two installed driving participants:
//! their compiled programs, replication layouts, the map from graph nodes to
//! serializer fields and the map from keeper link IDs to score categories.
use super::{
    events::Composition,
    program::{Sources, compile},
};
use crate::{
    Error,
    assets::AssetIndex,
    graph::bad,
    participant_logic::{Layout, Owner, Rules},
};
use nfs_world::replication::entity::Kind;
use nfs_world::scoring::{
    interpreter::Category,
    program::{Behavior, Program},
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const ACQUISITION: &str = "gameplay/drivingdataacquisition";
pub const SCORING: &str = "gameplay/scoring/drivingstylescoringparticipantlogic";

/// One graph node that owns a replicated serializer field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Field {
    pub node: u16,
    pub serializer: usize,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub struct Participant {
    pub blueprint: String,
    pub program: Arc<Program>,
    pub layout: Layout,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct ScoringContent {
    pub acquisition: Participant,
    pub scoring: Participant,
    /// Keeper link ID (from the installed channel) to the category it credits.
    pub categories: BTreeMap<u32, Category>,
}

/// Serializer fields in replication order: the composition's object rows in
/// traversal order, keeping the classes the participant rules mark as fields.
/// This is the same walk `participant_layout` uses, so index `i` here is
/// serializer `i` of the created entity.
pub fn serializer_fields(composition: &Composition, rules: &Rules) -> Result<Vec<Field>, Error> {
    let mut fields = Vec::new();
    for (index, node) in composition.nodes.iter().enumerate() {
        if node.class == "InterfaceDescriptorData" {
            continue;
        }
        let kind = rules
            .classes
            .get(&node.class)
            .ok_or_else(|| bad(&format!("unsupported participant class {}", node.class)))?;
        if let Some(kind) = kind {
            fields.push(Field {
                node: index as u16,
                serializer: fields.len(),
                kind: *kind,
            });
        }
    }
    Ok(fields)
}

/// Resolve each Score node's keeper link to a category from the installed
/// prefab that owns the node. Total scores live in the top scoring prefab.
pub fn keeper_categories(program: &Program) -> Result<BTreeMap<u32, Category>, Error> {
    let mut categories = BTreeMap::new();
    for node in &program.nodes {
        let Behavior::Score { context, .. } = node.behavior else {
            continue;
        };
        let Behavior::ScoreContext { keepers, .. } = &program
            .node(context)
            .map_err(|e| bad(&e.to_string()))?
            .behavior
        else {
            return Err(bad("score context behavior"));
        };
        let category = match node.identity.asset.rsplit('/').next().unwrap_or_default() {
            "speedcategoryscoringlogic" => Category::Speed,
            "stylecategoryscoringlogic" => Category::Style,
            "crewcategoryscoringlogic" => Category::Crew,
            "buildcategoryscoringlogic" => Category::Build,
            "outlawcategoryscoringlogic" => Category::Outlaw,
            "drivingstylescoringlogic" => Category::Total,
            other => return Err(bad(&format!("unknown score owner {other}"))),
        };
        for keeper in keepers {
            if categories
                .insert(keeper.field, category)
                .is_some_and(|previous| previous != category)
            {
                return Err(bad("keeper link credits two categories"));
            }
        }
    }
    if categories.len() != 6 {
        return Err(bad("expected six scoring keepers"));
    }
    Ok(categories)
}

impl AssetIndex {
    fn scoring_participant(&mut self, scene: &str, blueprint: &str) -> Result<Participant, Error> {
        let sources: Sources = self.scoring_sources(blueprint)?;
        let program = compile(&sources)?;
        let layout = self.participant_layout(scene, blueprint, Owner::Scene)?;
        let fields = serializer_fields(&sources.composition, &self.identity.participant_logic)?;
        if fields.len() != layout.fields.len()
            || fields
                .iter()
                .zip(&layout.fields)
                .any(|(field, layout)| field.kind != layout.kind)
        {
            return Err(bad(
                "scoring serializer fields disagree with the participant layout",
            ));
        }
        Ok(Participant {
            blueprint: blueprint.into(),
            program: Arc::new(program),
            layout,
            fields,
        })
    }

    /// Read both installed driving participants for one gameplay scene.
    pub fn scoring_content(&mut self, scene: &str) -> Result<ScoringContent, Error> {
        let acquisition = self.scoring_participant(scene, ACQUISITION)?;
        let scoring = self.scoring_participant(scene, SCORING)?;
        let categories = keeper_categories(&scoring.program)?;
        if keeper_categories(&acquisition.program).is_ok() {
            return Err(bad("acquisition participant must not award"));
        }
        Ok(ScoringContent {
            acquisition,
            scoring,
            categories,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::{
        events::{Identity, Node as EventNode},
        program::tests::fixture,
    };
    use nfs_world::scoring::program::{Keeper, Node};

    #[test]
    fn serializer_fields_follow_traversal_order_and_skip_interfaces() {
        let sources = fixture();
        let classes = [
            ("ParticipantLogicBlueprint", None),
            ("FloatEntityData", None),
            ("SyncedFloatEntityData", Some(Kind::FloatProperty)),
            ("LogicPrefabReferenceObjectData", None),
            ("LogicPrefabBlueprint", None),
            ("TelemetryHookEntityData", None),
            ("PlayerInfoAccessorEntityData", None),
            ("ParticipantLogicEntityData", Some(Kind::Root)),
        ];
        let rules = Rules {
            classes: classes
                .into_iter()
                .map(|(name, kind)| (name.to_string(), kind))
                .collect(),
            spawners: ["ParticipantSpawnerEntityData".to_string()]
                .into_iter()
                .collect(),
            root_channels: 1,
            spawner_channels: 2,
            boolean_ports: BTreeMap::new(),
            integer_ports: BTreeMap::new(),
        };
        let fields = serializer_fields(&sources.composition, &rules).unwrap();
        assert_eq!(
            fields,
            vec![
                Field {
                    node: 2,
                    serializer: 0,
                    kind: Kind::FloatProperty
                },
                Field {
                    node: 7,
                    serializer: 1,
                    kind: Kind::FloatProperty
                }
            ]
        );
        let mut unknown = sources.composition.clone();
        unknown.nodes.push(EventNode {
            identity: Identity {
                asset: "root".into(),
                index: 99,
                instances: vec![],
            },
            class: "MysteryEntityData".into(),
            listeners: None,
            forward: None,
        });
        assert!(serializer_fields(&unknown, &rules).is_err());
    }

    #[test]
    fn keeper_categories_resolve_from_owning_prefabs() {
        let identity = |asset: &str| nfs_world::scoring::program::Identity {
            asset: asset.into(),
            index: 1,
            instances: vec![],
        };
        let node = |asset: &str, behavior| Node {
            identity: identity(asset),
            class: "Test".into(),
            created: true,
            events: true,
            properties: true,
            forward: None,
            defaults: vec![],
            behavior,
        };
        let mut nodes = vec![node("x", Behavior::Inert)];
        let owners = [
            ("gameplay/scoring/speedcategoryscoringlogic", 10),
            ("gameplay/scoring/stylecategoryscoringlogic", 11),
            ("gameplay/scoring/crewcategoryscoringlogic", 12),
            ("gameplay/scoring/buildcategoryscoringlogic", 13),
            ("gameplay/scoring/outlawcategoryscoringlogic", 14),
            ("gameplay/scoring/drivingstylescoringlogic", 15),
        ];
        for (asset, keeper) in owners {
            let context = nodes.len() as u16 + 1;
            nodes.push(node(
                asset,
                Behavior::Score {
                    default: 0,
                    context,
                },
            ));
            nodes.push(node(
                asset,
                Behavior::ScoreContext {
                    participant_provider: 0,
                    keepers: vec![Keeper {
                        node: 0,
                        field: keeper,
                    }],
                },
            ));
        }
        let program = Program {
            nodes,
            cells: vec![],
            connections: vec![],
            startup: vec![],
            unmodeled: vec![],
        };
        let categories = keeper_categories(&program).unwrap();
        assert_eq!(categories[&10], Category::Speed);
        assert_eq!(categories[&15], Category::Total);
        assert_eq!(categories.len(), 6);
        let mut fewer = program.clone();
        fewer.nodes.truncate(11);
        assert!(keeper_categories(&fewer).is_err());
        let mut foreign = program;
        foreign.nodes[1].identity.asset = "gameplay/scoring/other".into();
        assert!(keeper_categories(&foreign).is_err());
    }
}
