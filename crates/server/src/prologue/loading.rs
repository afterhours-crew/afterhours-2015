// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The shared, participant-owned garage streaming reporter. Its scene spawner
//! and channels come from installed content; the intro keeps it alive outside
//! the garage, so early streaming reports survive until the waiting sequence.
use nfs_content::{
    assets::AssetIndex,
    events::Binding,
    participant_logic::{Channel, Layout, Owner as Source},
};
use nfs_world::{
    participants::{Lifecycle, Stage},
    replication::{
        self, Error, Record,
        entity::{
            self,
            creation::{Asset, Catalog, Content},
        },
        players::Players,
    },
    sequences::Owner,
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Layout", into = "Layout")]
pub struct Loading {
    layout: Layout,
    content: Content,
}
impl From<Loading> for Layout {
    fn from(value: Loading) -> Self {
        value.layout
    }
}
impl TryFrom<Layout> for Loading {
    type Error = String;
    fn try_from(value: Layout) -> Result<Self, Self::Error> {
        Self::new(value).map_err(|e| format!("prepared garage loading: {e:?}"))
    }
}
impl Loading {
    pub fn prepare(index: &mut AssetIndex) -> Result<Self, nfs_content::Error> {
        let layout = nfs_content::garage::loading(index)?;
        let values = index.participant_boolean_values("gameplay/garageloadlogic")?;
        let [value] = values.as_slice() else {
            return Err(nfs_content::Error::Content(
                "garage streaming property count".into(),
            ));
        };
        let expression = value.initial.as_ref().ok_or_else(|| {
            nfs_content::Error::Content("garage streaming property source".into())
        })?;
        for member in [false, true] {
            let actual = expression.evaluate(|input| match input {
                nfs_content::participant_logic::boolean::Input::ParticipantSetContains(port)
                    if port.field == 733_758_294
                        && layout.fields.get(port.serializer).is_some_and(|f| {
                            f.kind == entity::Kind::RpcReference
                                && matches!(f.channel, Some(Channel::Scene { .. }))
                        }) =>
                {
                    Some(member)
                }
                _ => None,
            })?;
            if actual != member
                || layout
                    .fields
                    .get(value.serializer)
                    .is_none_or(|f| f.kind != entity::Kind::BoolProperty)
            {
                return Err(nfs_content::Error::Content(
                    "garage streaming membership expression".into(),
                ));
            }
        }
        Self::new(layout).map_err(|e| nfs_content::Error::Content(format!("garage loading: {e:?}")))
    }
    pub(crate) fn new(layout: Layout) -> Result<Self, Error> {
        if layout.owner != Source::Scene
            || layout
                .fields
                .iter()
                .filter(|f| f.kind == entity::Kind::BoolProperty)
                .count()
                != 1
        {
            return Err(Error::Shape);
        }
        let entries = layout
            .catalog
            .iter()
            .map(|&(a, b)| {
                Ok((
                    u16::try_from(a).map_err(|_| Error::Bound)?,
                    u16::try_from(b).map_err(|_| Error::Bound)?,
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let content = Content::new(
            vec![Catalog::new(layout.asset.bundle, &entries)?],
            vec![(layout.asset, layout.profile().map_err(|_| Error::Shape)?)],
        )?;
        Ok(Self { layout, content })
    }
    pub fn asset(&self) -> Asset {
        self.layout.asset
    }
    /// Reflect the authored garage participant set into its synchronized
    /// streaming property. Entering-from-world/boot adds membership; leaving
    /// the compound garage states removes it. This drives the client's
    /// property stack; it does not manufacture a streaming-loaded response.
    /// The caller stages these records with the same lifecycle transaction.
    pub fn sync(
        &self,
        players: &mut Players,
        lifecycle: &Lifecycle,
        connection: u8,
        persona: u64,
    ) -> Result<Vec<Record>, Error> {
        let owners: Vec<_> = players.player_participants().collect();
        let field = self
            .layout
            .fields
            .iter()
            .position(|f| f.kind == entity::Kind::BoolProperty)
            .ok_or(Error::Shape)?;
        let mut records = Vec::new();
        for participant in owners {
            if !players.owns_participant(connection, persona, participant) {
                continue;
            }
            let mut found = players.objects().iter().filter_map(|(id, o)| {
                matches!(&o.initial, replication::Initial::Entity { prefix, fields }
                    if prefix.asset == self.layout.asset
                    && matches!(fields.first(), Some(entity::Initial::Root { reference, .. })
                        if *reference == participant))
                .then_some(id)
            });
            let Some(ghost) = found.next() else { continue };
            if found.next().is_some() {
                return Err(Error::DuplicateObject);
            }
            drop(found);
            let enabled = matches!(
                lifecycle.stage(participant),
                Some(
                    Stage::IntroEnteringGarage
                        | Stage::IntroLoadingGarage
                        | Stage::IntroCarSelection
                        | Stage::IntroComposite
                        | Stage::ReturningToGarage
                        | Stage::LoadingGarage
                        | Stage::EntryBlocked
                        | Stage::Customization
                )
            );
            let object = players.objects().get(ghost).ok_or(Error::UnknownObject)?;
            let replication::Update::Entity { binding, fields } = &object.current else {
                return Err(Error::TypeMismatch);
            };
            let current = match fields.get(field) {
                Some(Some(entity::Update::BoolProperty(Some(value)))) => *value,
                _ => match &object.initial {
                    replication::Initial::Entity { fields, .. } => match fields.get(field) {
                        Some(entity::Initial::BoolProperty(Some(value))) => *value,
                        _ => return Err(Error::TypeMismatch),
                    },
                    _ => return Err(Error::TypeMismatch),
                },
            };
            if current == enabled {
                continue;
            }
            let binding = binding.clone();
            let mut fields = vec![None; self.layout.fields.len()];
            fields[field] = Some(entity::Update::BoolProperty(Some(enabled)));
            if let Some(record) =
                players.change(ghost, replication::Update::Entity { binding, fields })?
            {
                records.push(record);
            }
        }
        Ok(records)
    }
    pub fn spawn(&self, players: &mut Players, owner: Owner) -> Result<Option<Record>, Error> {
        if !players.owns_participant(owner.connection, owner.persona, owner.participant) {
            return Err(Error::UnknownObject);
        }
        let world = players.objects();
        let count = world.iter().filter(|(_, o)| matches!(&o.initial, replication::Initial::Entity {prefix, fields} if prefix.asset == self.layout.asset && matches!(fields.first(),Some(entity::Initial::Root{reference,..}) if *reference == owner.participant))).count();
        match count {
            0 => {}
            1 => return Ok(None),
            _ => return Err(Error::DuplicateObject),
        }
        let scene = world
            .scene(self.layout.scene_content_key)
            .ok_or(Error::UnknownObject)?;
        let fields = self.layout.fields.iter().map(|f| {
            let binding = f.channel.as_ref().map(|c| match *c {
                Channel::Scene {
                    content_key,
                    serializer,
                } => Binding::Scene {
                    content_key,
                    serializer,
                },
                Channel::Participant { serializer } => Binding::Participant { serializer },
            });
            // The shared compound-state counter starts empty. Its Boolean cast
            // is false before any garage loading state has been entered.
            let value = (f.kind == entity::Kind::BoolProperty)
                .then_some(crate::event_logic::Value::Bool(false));
            (f.kind, binding, value)
        });
        let creation = crate::event_logic::logic_creation_fields(
            self.layout.asset,
            (1, self.layout.bus_count),
            scene,
            owner.participant,
            fields,
            |b| match b {
                Binding::Scene {
                    content_key,
                    serializer,
                } => world
                    .scene_endpoint(content_key, serializer)
                    .ok_or(Error::UnknownObject),
                _ => Err(Error::Unsupported),
            },
        )?;
        Ok(players.spawn_entities(vec![creation], &self.content)?.pop())
    }
}
