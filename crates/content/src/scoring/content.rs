// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Everything the server needs to run the two installed driving participants:
//! their compiled programs, replication layouts, the map from graph nodes to
//! serializer fields and the map from keeper link IDs to score categories.
//! The event driving-style participant compiles the same way against the
//! event's game-mode logic layout.
use super::{
    events::Composition,
    participant_properties::Property,
    program::{Sources, compile, compile_with_properties},
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
/// The race events' driving-style participant: the free-roam scoring prefab
/// plus the event tracking logic (E994 ghost 1605).
pub const EVENT_SCORING: &str = "gameplay/scoring/eventdrivingstylescoringparticipantlogic";

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
    /// Composition bus to the client's entity index for that bus (see
    /// `Composition::bus_entities`); remote event rows are fired on it.
    pub bus_entities: Vec<Option<u16>>,
}

#[derive(Clone, Debug)]
pub struct ScoringContent {
    pub acquisition: Participant,
    pub scoring: Participant,
    /// Keeper link ID (from the installed channel) to the category it credits.
    pub categories: BTreeMap<u32, Category>,
}

/// Any authored event participant logic, including setup, scoring, controls,
/// results and cleanup. It need not own REP keepers. The event host supplies
/// the instance's channels and lifetime through its actual spawner.
#[derive(Clone, Debug)]
pub struct EventParticipant {
    pub blueprint: String,
    pub program: Arc<Program>,
    pub fields: Vec<Field>,
    pub bus_entities: Vec<Option<u16>>,
    pub event_sync: Vec<super::event_sync::Binding>,
}

impl EventParticipant {
    /// Reusing a compiled blueprint across events is valid only when each
    /// spawner's serializer layout still matches its graph identities.
    pub fn validate_layout(&self, layout: &crate::events::Layout) -> Result<(), Error> {
        if self.blueprint != layout.blueprint || self.fields.len() != layout.fields.len() {
            return Err(bad("event participant layout identity"));
        }
        for (ordinal, (field, layout)) in self.fields.iter().zip(&layout.fields).enumerate() {
            let node = self
                .program
                .nodes
                .get(usize::from(field.node))
                .ok_or_else(|| bad("event serializer graph node"))?;
            if field.serializer != ordinal
                || field.kind != layout.kind
                || node.identity.asset != layout.asset
                || node.identity.index as usize != layout.object
                || node.class != layout.class
            {
                return Err(bad("event graph and serializer identities disagree"));
            }
        }
        Ok(())
    }
}

/// The event driving-style participant (#138). The event creates it in its
/// game mode; `fields` map graph nodes to that layout's serializers.
#[derive(Clone, Debug)]
pub struct EventScoring {
    pub blueprint: String,
    pub program: Arc<Program>,
    pub fields: Vec<Field>,
    pub bus_entities: Vec<Option<u16>>,
    /// Keeper link ID to the category it credits.
    pub categories: BTreeMap<u32, Category>,
    /// The integer participant properties the program writes, by graph node
    /// (the category totals the event results present, and award data).
    pub properties: Vec<(u16, Property)>,
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

/// Longest authored value path from a Score node back to its category source.
const MAX_SCORE_PATH: usize = 8;

/// The category prefab a node belongs to, by its blueprint name.
fn category_prefab(asset: &str) -> Option<Category> {
    match asset.rsplit('/').next().unwrap_or_default() {
        "speedcategoryscoringlogic" => Some(Category::Speed),
        "stylecategoryscoringlogic" => Some(Category::Style),
        "crewcategoryscoringlogic" => Some(Category::Crew),
        "buildcategoryscoringlogic" => Some(Category::Build),
        "outlawcategoryscoringlogic" => Some(Category::Outlaw),
        _ => None,
    }
}

/// The blueprint a prefab reference instantiates: the composition flattens
/// prefabs, so the reference's children carry it as the last instance step.
fn referenced_prefab(
    composition: &Composition,
    reference: &super::events::Identity,
) -> Result<String, Error> {
    let key = (reference.asset.clone(), reference.index);
    let assets: std::collections::BTreeSet<&str> = composition
        .nodes
        .iter()
        .filter(|node| node.identity.instances.last() == Some(&key))
        .map(|node| node.identity.asset.as_str())
        .collect();
    let mut assets = assets.into_iter();
    match (assets.next(), assets.next()) {
        (Some(blueprint), None) => Ok(blueprint.to_string()),
        (None, _) => Err(bad("score prefab reference has no children")),
        (Some(_), Some(_)) => Err(bad("score prefab reference children disagree")),
    }
}

/// Follow one Score node's authored value path backwards through the
/// composition's property rows (E887 topology: Score, PropertyCast,
/// FloatCache, FloatAccumulator, then the prefab reference whose blueprint is
/// the category; the total's cache is fed by the MathOp sum of the
/// categories). Every node on the path has one upstream row.
fn score_category(
    program: &Program,
    composition: &Composition,
    score: u16,
) -> Result<Category, Error> {
    let mut current = score;
    for _ in 0..MAX_SCORE_PATH {
        let mut upstream = composition
            .properties
            .iter()
            .filter(|row| row.target.node == current)
            .map(|row| row.source.node);
        let (Some(source), None) = (upstream.next(), upstream.next()) else {
            return Err(bad("score value path is not singular"));
        };
        let node = composition
            .nodes
            .get(usize::from(source))
            .ok_or_else(|| bad("score value path node"))?;
        program.node(source).map_err(|e| bad(&e.to_string()))?;
        match node.class.as_str() {
            "MathOpEntityData" => return Ok(Category::Total),
            "LogicPrefabReferenceObjectData" => {
                let blueprint = referenced_prefab(composition, &node.identity)?;
                return category_prefab(&blueprint)
                    .ok_or_else(|| bad(&format!("score source prefab {blueprint}")));
            }
            _ => current = source,
        }
    }
    Err(bad("score value path depth bound"))
}

/// Resolve each Score node's keeper link to the category whose value it
/// awards. All six Score nodes live in the top scoring prefab; the category
/// is the prefab that produces the score, not the prefab that owns the node.
pub fn keeper_categories(
    program: &Program,
    composition: &Composition,
) -> Result<BTreeMap<u32, Category>, Error> {
    if program.nodes.len() != composition.nodes.len() {
        return Err(bad("scoring program and composition disagree"));
    }
    let mut categories = BTreeMap::new();
    for (id, node) in program.nodes.iter().enumerate() {
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
        let category = score_category(program, composition, id as u16)?;
        for keeper in keepers {
            if categories
                .insert(keeper.field, category)
                .is_some_and(|previous| previous != category)
            {
                return Err(bad("keeper link credits two categories"));
            }
        }
    }
    let distinct: std::collections::BTreeSet<Category> = categories.values().copied().collect();
    if categories.len() != 6 || distinct.len() != 6 {
        return Err(bad("expected six scoring keepers, one per category"));
    }
    Ok(categories)
}

impl AssetIndex {
    fn scoring_participant(
        &mut self,
        scene: &str,
        blueprint: &str,
    ) -> Result<(Participant, Composition), Error> {
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
        Ok((
            Participant {
                blueprint: blueprint.into(),
                program: Arc::new(program),
                layout,
                fields,
                bus_entities: sources.composition.bus_entities.clone(),
            },
            sources.composition,
        ))
    }

    fn compile_event_participant(
        &mut self,
        layout: &crate::events::Layout,
        owned_properties: bool,
    ) -> Result<(EventParticipant, Composition), Error> {
        let sources: Sources = self.scoring_sources(&layout.blueprint)?;
        let program = if owned_properties {
            let properties = self
                .owned_participant_properties(&layout.blueprint)?
                .into_iter()
                .collect();
            compile_with_properties(&sources, &properties)?
        } else {
            compile(&sources)?
        };
        let event_sync = self.scoring_event_sync(&layout.blueprint, &sources.composition)?;
        let fields = serializer_fields(&sources.composition, &self.identity.participant_logic)?;
        if fields.len() != layout.fields.len()
            || fields
                .iter()
                .zip(&layout.fields)
                .any(|(field, layout)| field.kind != layout.kind)
        {
            return Err(bad(
                "event scoring serializer fields disagree with the event layout",
            ));
        }
        let participant = EventParticipant {
            blueprint: layout.blueprint.clone(),
            program: Arc::new(program),
            fields,
            bus_entities: sources.composition.bus_entities.clone(),
            event_sync,
        };
        participant.validate_layout(layout)?;
        Ok((participant, sources.composition))
    }

    /// Compile setup/scoring/results/cleanup logic without requiring REP
    /// keepers or selecting one special driving-style blueprint.
    pub fn event_participant(
        &mut self,
        layout: &crate::events::Layout,
    ) -> Result<EventParticipant, Error> {
        self.compile_event_participant(layout, true).map(|(p, _)| p)
    }

    /// Compile the event driving-style participant for its event layout.
    pub fn event_scoring(&mut self, layout: &crate::events::Layout) -> Result<EventScoring, Error> {
        let (compiled, composition) = self.compile_event_participant(layout, false)?;
        let program = &compiled.program;
        let categories = keeper_categories(program, &composition)?;
        let mut properties = Vec::new();
        for (identity, property) in self.integer_participant_properties(&layout.blueprint)? {
            let node = composition
                .nodes
                .iter()
                .position(|n| n.identity == identity)
                .ok_or_else(|| bad("participant property accessor outside the program"))?;
            match program.nodes.get(node).map(|n| &n.behavior) {
                Some(Behavior::Scalar(
                    nfs_world::scoring::nodes::Definition::IntegerParticipantProperty,
                )) => properties.push((node as u16, property)),
                _ => return Err(bad("participant property accessor behavior")),
            }
        }
        Ok(EventScoring {
            blueprint: compiled.blueprint,
            program: compiled.program,
            fields: compiled.fields,
            bus_entities: compiled.bus_entities,
            categories,
            properties,
        })
    }

    /// Read both installed driving participants for one gameplay scene.
    pub fn scoring_content(&mut self, scene: &str) -> Result<ScoringContent, Error> {
        let (acquisition, acquisition_composition) =
            self.scoring_participant(scene, ACQUISITION)?;
        let (scoring, composition) = self.scoring_participant(scene, SCORING)?;
        let categories = keeper_categories(&scoring.program, &composition)?;
        if keeper_categories(&acquisition.program, &acquisition_composition).is_ok() {
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
    fn compiled_event_logic_rejects_same_kind_but_different_serializer_identity() {
        let sources = fixture();
        let program = Arc::new(compile(&sources).unwrap());
        let compiled = EventParticipant {
            blueprint: "root".into(),
            program,
            fields: vec![Field {
                node: 2,
                serializer: 0,
                kind: Kind::FloatProperty,
            }],
            bus_entities: vec![Some(1)],
            event_sync: vec![],
        };
        let mut layout = crate::events::Layout {
            blueprint: "root".into(),
            asset: nfs_world::replication::entity::creation::Asset {
                bundle: 1,
                type_id: 1,
                local_index: 0,
            },
            catalog: vec![],
            bus_count: 1,
            sub_id: 1,
            occurrence: 0,
            fields: vec![crate::events::Field {
                kind: Kind::FloatProperty,
                class: "SyncedFloatEntityData".into(),
                asset: "root".into(),
                object: 2,
                binding: None,
                mirrors: None,
            }],
        };
        compiled.validate_layout(&layout).unwrap();
        layout.fields[0].object = 3;
        assert!(compiled.validate_layout(&layout).is_err());
        layout.fields[0].object = 2;
        layout.fields[0].class = "SyncedBoolEntityData".into();
        assert!(compiled.validate_layout(&layout).is_err());
        layout.fields[0].class = "SyncedFloatEntityData".into();
        layout.blueprint = "other".into();
        assert!(compiled.validate_layout(&layout).is_err());
    }

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
    fn keeper_categories_follow_the_score_value_path() {
        use crate::scoring::events::DataConnection;
        use nfs_world::scoring::properties::Port;
        let top = "gameplay/scoring/drivingstylescoringlogic";
        let identity = |asset: &str, index: usize| Identity {
            asset: asset.into(),
            index,
            instances: vec![],
        };
        let mut nodes = vec![];
        let mut composition_nodes = vec![];
        let mut properties = vec![];
        let push = |nodes: &mut Vec<Node>,
                    composition_nodes: &mut Vec<EventNode>,
                    asset: &str,
                    class: &str,
                    behavior| {
            let id = nodes.len() as u16;
            let identity = identity(asset, nodes.len());
            nodes.push(Node {
                identity: nfs_world::scoring::program::Identity {
                    asset: asset.into(),
                    index: nodes.len() as u32,
                    instances: vec![],
                },
                class: class.into(),
                created: true,
                events: true,
                properties: true,
                forward: None,
                defaults: vec![],
                behavior,
            });
            composition_nodes.push(EventNode {
                identity,
                class: class.into(),
                listeners: None,
                forward: None,
            });
            id
        };
        let channel = push(
            &mut nodes,
            &mut composition_nodes,
            top,
            "LinkedSchematicChannelReferenceEntityData",
            Behavior::Inert,
        );
        let mut score_node = |asset: &str, class: &str, keeper: u32| {
            // Score <- cast <- cache <- source: a prefab reference whose
            // children come from the category blueprint, or the MathOp sum.
            let source = push(
                &mut nodes,
                &mut composition_nodes,
                top,
                class,
                Behavior::Inert,
            );
            if class == "LogicPrefabReferenceObjectData" {
                let child = push(
                    &mut nodes,
                    &mut composition_nodes,
                    asset,
                    "FloatEntityData",
                    Behavior::Inert,
                );
                composition_nodes[usize::from(child)].identity.instances =
                    vec![(top.to_string(), usize::from(source))];
            }
            let cache = push(
                &mut nodes,
                &mut composition_nodes,
                top,
                "FloatCacheEntityData",
                Behavior::Inert,
            );
            let cast = push(
                &mut nodes,
                &mut composition_nodes,
                top,
                "PropertyCastEntityData",
                Behavior::Inert,
            );
            let context = push(
                &mut nodes,
                &mut composition_nodes,
                top,
                "ScoreContextEntityData",
                Behavior::ScoreContext {
                    participant_provider: 0,
                    keepers: vec![Keeper {
                        node: channel,
                        field: keeper,
                    }],
                },
            );
            let score = push(
                &mut nodes,
                &mut composition_nodes,
                top,
                "ScoreEntityData",
                Behavior::Score {
                    default: 0,
                    context,
                },
            );
            for (from, to) in [(source, cache), (cache, cast), (cast, score)] {
                properties.push(DataConnection {
                    source: Port {
                        node: from,
                        field: 1,
                    },
                    target: Port { node: to, field: 2 },
                    bus: 0,
                    index: properties.len() as u16,
                    flags: None,
                });
            }
            (source, score)
        };
        let mut sources = vec![];
        for (asset, keeper) in [
            ("gameplay/scoring/speedcategoryscoringlogic", 10),
            ("gameplay/scoring/stylecategoryscoringlogic", 11),
            ("gameplay/scoring/crewcategoryscoringlogic", 12),
            ("gameplay/scoring/buildcategoryscoringlogic", 13),
            ("gameplay/scoring/outlawcategoryscoringlogic", 14),
        ] {
            sources.push(score_node(asset, "LogicPrefabReferenceObjectData", keeper).0);
        }
        let (sum, total_score) = score_node(top, "MathOpEntityData", 15);
        // The total sums the five category values; the walk stops at the sum.
        for source in sources {
            properties.push(DataConnection {
                source: Port {
                    node: source,
                    field: 1,
                },
                target: Port {
                    node: sum,
                    field: 3,
                },
                bus: 0,
                index: properties.len() as u16,
                flags: None,
            });
        }
        let program = Program {
            nodes,
            cells: vec![],
            connections: vec![],
            startup: vec![],
            unmodeled: vec![],
        };
        let composition = Composition {
            nodes: composition_nodes,
            buses: vec![],
            bus_entities: vec![],
            connections: vec![],
            properties,
            links: vec![],
        };
        let categories = keeper_categories(&program, &composition).unwrap();
        assert_eq!(categories[&10], Category::Speed);
        assert_eq!(categories[&14], Category::Outlaw);
        assert_eq!(categories[&15], Category::Total);
        assert_eq!(categories.len(), 6);

        // A second upstream row makes the path ambiguous.
        let mut ambiguous = composition.clone();
        ambiguous.properties.push(DataConnection {
            source: Port {
                node: sum,
                field: 1,
            },
            target: Port {
                node: total_score,
                field: 2,
            },
            bus: 0,
            index: 99,
            flags: None,
        });
        assert!(keeper_categories(&program, &ambiguous).is_err());

        // A path that never reaches a category prefab or the sum fails.
        let mut unresolved = composition.clone();
        unresolved.nodes[usize::from(sum)].class = "FloatEntityData".into();
        assert!(keeper_categories(&program, &unresolved).is_err());

        // Two keepers on one category are rejected: the second reference's
        // child (node 8) now comes from the speed blueprint.
        let mut duplicated = composition.clone();
        duplicated.nodes[8].identity.asset = "gameplay/scoring/speedcategoryscoringlogic".into();
        assert!(keeper_categories(&program, &duplicated).is_err());

        // A reference without children cannot name its blueprint.
        let mut orphan = composition.clone();
        orphan.nodes[2].identity.instances.clear();
        assert!(keeper_categories(&program, &orphan).is_err());

        // Program and composition must describe the same nodes.
        let mut shorter = composition.clone();
        shorter.nodes.pop();
        assert!(keeper_categories(&program, &shorter).is_err());
    }
}
