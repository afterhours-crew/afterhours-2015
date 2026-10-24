// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned free-roam scoring: the two installed driving participants as live
//! replicated entities whose state is an interpreter session. This module is
//! pure: it builds creation and update records, routes reports and commands,
//! and aggregates keeper awards into a category credit. The listener commits
//! records with its frame; the durable credit is applied by the storage path.
//! An owned race event runs its own driving-style participant on the entity
//! the event creates in its game mode (#138).
use nfs_content::scoring::{
    content::{EventScoring, Field, Participant, ScoringContent},
    participant_properties::Property,
};
use nfs_protocol::world::rpc::Serial;
use nfs_services::reputation::Scores;
use nfs_world::{
    participants::EntityCall,
    replication::{
        self, Initial, Update as RecordUpdate,
        entity::{
            Body, Initial as EntityInitial, Kind, Rpc, Update as EntityUpdate,
            creation::{Binding, Catalog, Content as CreationContent, Creation, Prefix},
        },
        players::Players,
    },
    scoring::{
        collision::Command,
        interpreter::{Award, Category, Inputs, Output, Session},
        nodes::FLOAT_VALUE,
        program::Program,
        properties::Port,
        reports::{Binding as ReportBinding, Kind as ReportKind},
        synced,
        value::Value,
    },
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const SUB_ID: u32 = 1;
/// Engine context time scale used for free roam. The native scale table is
/// not available statically (E893/E895); ordinary free roam runs at real time.
pub const TIME_SCALE: f32 = 1.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Which {
    Acquisition,
    Scoring,
    /// The event driving-style participant, on the event's entity.
    Event,
    /// A catalog event's own program and serializer binding.
    EventFor(u32),
}
impl Which {
    /// The participants free roam runs.
    pub const FREE_ROAM: [Which; 2] = [Which::Acquisition, Which::Scoring];
    pub fn is_event(self) -> bool {
        matches!(self, Self::Event | Self::EventFor(_))
    }
}

/// The event driving-style participant and its update binding.
#[derive(Clone, Debug)]
struct EventPart {
    scoring: EventScoring,
    binding: Arc<Binding>,
}

#[derive(Clone, Debug)]
pub struct Content {
    installed: Arc<ScoringContent>,
    creation: Arc<CreationContent>,
    bindings: [Arc<Binding>; 2],
    /// Installed road network for the road-driving query (E916); None keeps
    /// road inputs absent, which preserves the query state.
    road: Option<Arc<nfs_world::scoring::road_network::Network>>,
    /// Item definitions with the stock vehicle figures for the stats provider.
    items: Option<Arc<nfs_world::items::InventoryCatalog>>,
    nitrous: nfs_content::nitrous::Catalog,
    event: Option<Arc<EventPart>>,
    events: BTreeMap<u32, Arc<EventPart>>,
}

impl Content {
    pub fn new(installed: Arc<ScoringContent>) -> Result<Self, replication::Error> {
        let parts = [&installed.acquisition, &installed.scoring];
        let bundle = parts[0].layout.asset.bundle;
        if parts
            .iter()
            .any(|p| p.layout.asset.bundle != bundle || p.layout.catalog != parts[0].layout.catalog)
        {
            return Err(replication::Error::Shape);
        }
        let entries = parts[0]
            .layout
            .catalog
            .iter()
            .map(|&(kind, count)| Ok((u16::try_from(kind)?, u16::try_from(count)?)))
            .collect::<Result<Vec<_>, std::num::TryFromIntError>>()
            .map_err(|_| replication::Error::Bound)?;
        let catalog = Catalog::new(bundle, &entries)?;
        let profiles = parts
            .iter()
            .map(|p| {
                let profile = p.layout.profile().map_err(|_| replication::Error::Shape)?;
                Ok((p.layout.asset, profile))
            })
            .collect::<Result<Vec<_>, replication::Error>>()?;
        let creation = CreationContent::new(vec![catalog], profiles)?;
        let bindings = [
            Arc::new(creation.binding(parts[0].layout.asset)?),
            Arc::new(creation.binding(parts[1].layout.asset)?),
        ];
        Ok(Self {
            installed,
            creation: Arc::new(creation),
            bindings,
            road: None,
            items: None,
            nitrous: Default::default(),
            event: None,
            events: BTreeMap::new(),
        })
    }
    /// Attach the event driving-style participant. `creation` is the event's
    /// creation content (its entity lives in the event bundle) and `asset`
    /// the event layout's asset. Its keepers must credit the categories the
    /// free-roam participant credits.
    pub fn with_event(
        mut self,
        scoring: EventScoring,
        creation: &CreationContent,
        asset: nfs_world::replication::entity::creation::Asset,
    ) -> Result<Self, replication::Error> {
        if scoring.categories != self.installed.categories {
            return Err(replication::Error::Shape);
        }
        let binding = Arc::new(creation.binding(asset)?);
        self.event = Some(Arc::new(EventPart { scoring, binding }));
        Ok(self)
    }
    pub fn has_event(&self) -> bool {
        self.event.is_some() || !self.events.is_empty()
    }
    pub fn with_authored_event(
        mut self,
        id: u32,
        scoring: EventScoring,
        creation: &CreationContent,
        asset: nfs_world::replication::entity::creation::Asset,
    ) -> Result<Self, replication::Error> {
        if self.events.len() >= nfs_content::events::discovery::MAX_EVENTS {
            return Err(replication::Error::Bound);
        }
        if scoring.categories != self.installed.categories || self.events.contains_key(&id) {
            return Err(replication::Error::Shape);
        }
        let binding = Arc::new(creation.binding(asset)?);
        self.events
            .insert(id, Arc::new(EventPart { scoring, binding }));
        Ok(self)
    }
    pub fn event_kind(&self, id: u32) -> Option<Which> {
        if self.events.contains_key(&id) {
            Some(Which::EventFor(id))
        } else {
            self.event.as_ref().map(|_| Which::Event)
        }
    }
    fn event_part(&self, which: Which) -> Result<&EventPart, replication::Error> {
        match which {
            Which::Event => self.event.as_deref(),
            Which::EventFor(id) => self.events.get(&id).map(Arc::as_ref),
            _ => None,
        }
        .ok_or(replication::Error::Unsupported)
    }
    /// The compiled program of one participant kind.
    pub fn program(&self, which: Which) -> Result<&Arc<Program>, replication::Error> {
        Ok(match which {
            Which::Acquisition | Which::Scoring => &self.participant(which).program,
            Which::Event | Which::EventFor(_) => &self.event_part(which)?.scoring.program,
        })
    }
    fn fields(&self, which: Which) -> Result<&[Field], replication::Error> {
        Ok(match which {
            Which::Acquisition | Which::Scoring => &self.participant(which).fields,
            Which::Event | Which::EventFor(_) => &self.event_part(which)?.scoring.fields,
        })
    }
    /// Composition bus to client entity index, for remote event rows.
    pub fn bus_entities(&self, which: Which) -> Result<&[Option<u16>], replication::Error> {
        Ok(match which {
            Which::Acquisition | Which::Scoring => &self.participant(which).bus_entities,
            Which::Event | Which::EventFor(_) => &self.event_part(which)?.scoring.bus_entities,
        })
    }
    /// The integer participant properties an event session has written:
    /// each accessor's bound `Value`, the value it last wrote (the category
    /// totals the event results present, E1000). Nothing in the program reads
    /// the accessors' outputs, so those have no cell.
    pub fn event_properties(
        &self,
        session: &Session,
    ) -> Result<BTreeMap<Property, i32>, replication::Error> {
        self.event_properties_for(Which::Event, session)
    }
    pub fn event_properties_for(
        &self,
        which: Which,
        session: &Session,
    ) -> Result<BTreeMap<Property, i32>, replication::Error> {
        let mut values = BTreeMap::new();
        for (node, property) in &self.event_part(which)?.scoring.properties {
            match session.read(Port {
                node: *node,
                field: FLOAT_VALUE,
            }) {
                Ok(Some(Value::Integer(v))) => {
                    values.insert(property.clone(), v);
                }
                Ok(None) => {}
                _ => return Err(replication::Error::TypeMismatch),
            }
        }
        Ok(values)
    }
    /// Attach the item definitions (stock vehicle figures).
    pub fn with_items(mut self, items: Arc<nfs_world::items::InventoryCatalog>) -> Self {
        self.items = Some(items);
        self
    }
    pub fn items(&self) -> Option<&nfs_world::items::InventoryCatalog> {
        self.items.as_deref()
    }
    pub fn with_nitrous(mut self, nitrous: nfs_content::nitrous::Catalog) -> Self {
        self.nitrous = nitrous;
        self
    }
    pub fn nitrous(
        &self,
        guid: &nfs_world::items::Guid,
    ) -> Option<Arc<nfs_world::nitrous_fuel::Configuration>> {
        self.nitrous.get(guid).cloned()
    }
    /// Attach the installed road network.
    pub fn with_road(mut self, road: Arc<nfs_world::scoring::road_network::Network>) -> Self {
        self.road = Some(road);
        self
    }
    pub fn road(&self) -> Option<&nfs_world::scoring::road_network::Network> {
        self.road.as_deref()
    }
    pub fn installed(&self) -> &ScoringContent {
        &self.installed
    }
    /// A free-roam participant. The event participant has no installed
    /// scene layout of its own (its entity is the event's); its program,
    /// fields and buses come from the other accessors.
    pub fn participant(&self, which: Which) -> &Participant {
        match which {
            Which::Acquisition => &self.installed.acquisition,
            Which::Scoring | Which::Event | Which::EventFor(_) => &self.installed.scoring,
        }
    }
    pub fn binding(&self, which: Which) -> Result<Arc<Binding>, replication::Error> {
        Ok(match which {
            Which::Acquisition => self.bindings[0].clone(),
            Which::Scoring => self.bindings[1].clone(),
            Which::Event | Which::EventFor(_) => self.event_part(which)?.binding.clone(),
        })
    }
    pub fn creation_content(&self) -> &CreationContent {
        &self.creation
    }

    /// Start a fresh session for one participant blueprint.
    pub fn start(
        &self,
        which: Which,
        inputs: &Inputs,
    ) -> Result<(Session, Output), nfs_world::scoring::Error> {
        let program = self
            .program(which)
            .map_err(|_| nfs_world::scoring::Error::InvalidGraph)?;
        let mut session = Session::new(program.clone())?;
        let output = session.start(inputs)?;
        Ok((session, output))
    }

    fn field(&self, which: Which, node: u16) -> Option<Field> {
        let fields = self.fields(which).ok()?;
        fields
            .binary_search_by_key(&node, |f| f.node)
            .ok()
            .map(|i| fields[i])
    }

    /// The creation record for a started session, bound to its participant
    /// and gameplay scene. Synchronized values come from the session; rpc
    /// serializers follow the installed layout. The collision serializer's
    /// Boolean and the slam reference list use the official creation's
    /// values (true, empty) pending their own contracts.
    pub fn creation(
        &self,
        which: Which,
        session: &Session,
        participant: u16,
        players: &Players,
    ) -> Result<Creation, replication::Error> {
        if which.is_event() {
            // The event creates this entity with its game-mode logics.
            return Err(replication::Error::Unsupported);
        }
        let part = self.participant(which);
        let world = players.objects();
        let blueprint = world
            .scene(part.layout.scene_content_key)
            .ok_or(replication::Error::UnknownObject)?;
        let serial = Serial::new(0)
            .ok_or(replication::Error::Shape)?
            .next_initialization();
        let mut selector = 0u16;
        let mut next_rpc = || -> Result<Rpc, replication::Error> {
            let current = Rpc { selector, serial };
            selector = selector.checked_add(1).ok_or(replication::Error::Bound)?;
            Ok(current)
        };
        let mut initial = Vec::with_capacity(part.fields.len());
        let mut updates = Vec::with_capacity(part.fields.len());
        for (field, layout) in part.fields.iter().zip(&part.layout.fields) {
            let value = |port: u32| {
                session.read(Port {
                    node: field.node,
                    field: port,
                })
            };
            let (first, update) = match field.kind {
                Kind::Root => (
                    EntityInitial::Root {
                        value: part.layout.bus_count,
                        rpc: next_rpc()?,
                        reference: participant,
                    },
                    EntityUpdate::Noop,
                ),
                Kind::RpcReference => {
                    let Some(nfs_content::participant_logic::Channel::Scene {
                        content_key,
                        serializer,
                    }) = &layout.channel
                    else {
                        return Err(replication::Error::Unsupported);
                    };
                    let (reference, target_selector) = world
                        .scene_endpoint(*content_key, *serializer)
                        .ok_or(replication::Error::UnknownObject)?;
                    (
                        EntityInitial::RpcReference {
                            rpc: next_rpc()?,
                            reference,
                            target_selector,
                        },
                        EntityUpdate::Noop,
                    )
                }
                Kind::Rpc => (EntityInitial::Rpc(next_rpc()?), EntityUpdate::Noop),
                Kind::RpcBool => (
                    EntityInitial::RpcBool {
                        rpc: next_rpc()?,
                        value: true,
                    },
                    EntityUpdate::Noop,
                ),
                Kind::RpcReferences => (
                    EntityInitial::RpcReferences {
                        rpc: next_rpc()?,
                        references: Vec::new(),
                    },
                    EntityUpdate::Noop,
                ),
                Kind::StringProperty => {
                    let text = match value(synced::OUT) {
                        Ok(Some(Value::Text(v))) => Some(v.as_str().as_bytes().to_vec()),
                        Ok(None) => None,
                        _ => return Err(replication::Error::TypeMismatch),
                    };
                    (
                        EntityInitial::StringProperty(text),
                        EntityUpdate::StringProperty(None),
                    )
                }
                Kind::FloatProperty => {
                    let bits = match value(synced::OUT) {
                        Ok(Some(Value::Float(v))) => v.to_bits(),
                        Ok(None) => 0,
                        _ => return Err(replication::Error::TypeMismatch),
                    };
                    (
                        EntityInitial::FloatProperty(Some(bits)),
                        EntityUpdate::FloatProperty(None),
                    )
                }
                Kind::BoolProperty => {
                    let flag = match value(synced::OUT) {
                        Ok(Some(Value::Boolean(v))) => v,
                        Ok(None) => false,
                        _ => return Err(replication::Error::TypeMismatch),
                    };
                    (
                        EntityInitial::BoolProperty(Some(flag)),
                        EntityUpdate::BoolProperty(None),
                    )
                }
                Kind::I32Property => {
                    let number = match value(synced::OUT) {
                        Ok(Some(Value::Integer(v))) => v,
                        Ok(None) => 0,
                        _ => return Err(replication::Error::TypeMismatch),
                    };
                    (
                        EntityInitial::I32Property(Some(number)),
                        EntityUpdate::I32Property(None),
                    )
                }
                _ => return Err(replication::Error::Unsupported),
            };
            initial.push(first);
            updates.push(Some(update));
        }
        let creation = Creation {
            prefix: Prefix {
                parent: None,
                blueprint,
                sub_id: SUB_ID,
                owner: None,
                asset: part.layout.asset,
            },
            body: Body {
                initial: Some(initial),
                updates,
            },
        };
        creation.encode(&self.creation)?;
        Ok(creation)
    }

    /// Property updates for dirty synchronized values. Nodes without a
    /// serializer field (none in the installed layouts) are rejected.
    pub fn update(
        &self,
        which: Which,
        synced: &[(u16, Value)],
    ) -> Result<Option<RecordUpdate>, replication::Error> {
        if synced.is_empty() {
            return Ok(None);
        }
        let mut fields = vec![None; self.fields(which)?.len()];
        for (node, value) in synced {
            let field = self
                .field(which, *node)
                .ok_or(replication::Error::Unsupported)?;
            let update = match (field.kind, value) {
                (Kind::StringProperty, Value::Text(v)) => {
                    EntityUpdate::StringProperty(Some(v.as_str().as_bytes().to_vec()))
                }
                (Kind::FloatProperty, Value::Float(v)) => {
                    EntityUpdate::FloatProperty(Some(v.to_bits()))
                }
                (Kind::BoolProperty, Value::Boolean(v)) => EntityUpdate::BoolProperty(Some(*v)),
                (Kind::I32Property, Value::Integer(v)) => EntityUpdate::I32Property(Some(*v)),
                _ => return Err(replication::Error::TypeMismatch),
            };
            fields[field.serializer] = Some(update);
        }
        Ok(Some(RecordUpdate::Entity {
            binding: self.binding(which)?,
            fields,
        }))
    }

    /// Incoming report serializers of a created entity, keyed by graph node.
    pub fn report_bindings(
        &self,
        which: Which,
        players: &Players,
        ghost: u16,
    ) -> Result<Vec<(u16, ReportBinding)>, replication::Error> {
        let program = self.program(which)?;
        let mut bindings = Vec::new();
        for field in self.fields(which)? {
            let kind = match (
                field.kind,
                program
                    .node(field.node)
                    .map_err(|_| replication::Error::Shape)?
                    .class
                    .as_str(),
            ) {
                (Kind::Rpc, "RaceVehicleNearMissQueryEntityData") => ReportKind::NearMiss,
                (Kind::Rpc, "RaceVehicleAccumulatingCollisionsQueryEntityData") => {
                    ReportKind::AccumulatedCollisions
                }
                (Kind::RpcBool, "RaceVehicleCollisionQueryEntityData") => ReportKind::Collision,
                _ => continue,
            };
            bindings.push((
                field.node,
                ReportBinding::new(players, ghost, field.serializer, kind)?,
            ));
        }
        Ok(bindings)
    }

    /// Host control call for a collision query node of a created entity.
    pub fn command(
        &self,
        which: Which,
        players: &Players,
        ghost: u16,
        node: u16,
        command: Command,
    ) -> Result<EntityCall, replication::Error> {
        let field = self
            .field(which, node)
            .ok_or(replication::Error::Unsupported)?;
        let Some(Initial::Entity { fields, .. }) = players.objects().get(ghost).map(|o| &o.initial)
        else {
            return Err(replication::Error::UnknownObject);
        };
        let Some(EntityInitial::RpcBool { rpc, .. }) = fields.get(field.serializer) else {
            return Err(replication::Error::TypeMismatch);
        };
        Ok(EntityCall {
            entity: ghost,
            selector: rpc.selector,
            serial: rpc.serial,
            method: command.method(),
        })
    }

    /// Sum keeper awards into one category credit. A keeper the installed
    /// channel does not declare, or a negative total, rejects the whole batch.
    /// All-zero credits produce no transaction.
    pub fn credit(&self, awards: &[Award]) -> Result<Option<Scores>, replication::Error> {
        let mut totals: BTreeMap<Category, i64> = BTreeMap::new();
        for award in awards {
            let category = *self
                .installed
                .categories
                .get(&award.keeper)
                .ok_or(replication::Error::Unsupported)?;
            *totals.entry(category).or_default() += i64::from(award.amount);
        }
        let get = |category| -> Result<i32, replication::Error> {
            let value = totals.get(&category).copied().unwrap_or(0);
            if value < 0 {
                return Err(replication::Error::Shape);
            }
            i32::try_from(value).map_err(|_| replication::Error::Bound)
        };
        let scores = Scores {
            total: get(Category::Total)?,
            build: get(Category::Build)?,
            crew: get(Category::Crew)?,
            outlaw: get(Category::Outlaw)?,
            speed: get(Category::Speed)?,
            style: get(Category::Style)?,
        };
        let any = [
            scores.total,
            scores.build,
            scores.crew,
            scores.outlaw,
            scores.speed,
            scores.style,
        ]
        .iter()
        .any(|v| *v != 0);
        Ok(any.then_some(scores))
    }
}

/// Integer field updates for live garage-logic entities that present REP.
pub fn reputation_updates(
    logic: &crate::garage_logic::GarageLogic,
    values: &nfs_services::reputation::Values,
) -> Vec<(u16, usize, i32)> {
    logic
        .reputation_fields()
        .into_iter()
        .map(|(capture, index, field)| (capture, index, values.get(field)))
        .collect()
}

#[cfg(test)]
mod tests;
