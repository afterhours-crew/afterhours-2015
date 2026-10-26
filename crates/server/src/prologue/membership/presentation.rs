// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_content::{
    events::{
        Binding,
        lifecycle::{
            Scope,
            bindings::{Controller, Link, Observer, PredicateKind, Spawner, StateProperty},
        },
    },
    participant_logic::{Channel, Layout, Owner, activation::Predicate},
    scoring::events::Identity,
};
use nfs_world::replication::{
    Record,
    entity::{
        self,
        creation::{Asset, Catalog, Content as Codec},
    },
};

// Free-roam statistics still receives event-gated sample/debug updates through
// GarageLogic. Keep that object's complete lifecycle with its current owner
// until those updates also move; transferring only allocation would orphan them.
pub(super) const BLUEPRINTS: [&str; 4] = [
    "gameplay/lockvehicleinputparticipantlogic",
    "presentation/loadingscreenstatepresentationblueprint",
    "presentation/suppressvoipblueprint",
    "gameplay/stats/garagestatslogic",
];

#[derive(Clone, Debug)]
pub(super) struct Definition {
    layout: Layout,
    codec: Codec,
}
impl Definition {
    pub(super) fn prepare(
        index: &mut AssetIndex,
        scene: &str,
        blueprint: &str,
    ) -> Result<Self, nfs_content::Error> {
        Self::new(index.participant_layout(scene, blueprint, Owner::Scene)?).map_err(|e| {
            nfs_content::Error::Content(format!("scene presentation layout {blueprint}: {e:?}"))
        })
    }
    pub(super) fn new(layout: Layout) -> Result<Self, Error> {
        if layout.owner != Owner::Scene
            || layout.fields.iter().enumerate().any(|(i, f)| {
                if i == 0 {
                    f.kind != entity::Kind::Root || f.channel.is_some()
                } else {
                    match f.kind {
                        entity::Kind::RpcReference => {
                            !matches!(f.channel, Some(Channel::Scene { .. }))
                        }
                        entity::Kind::I32Property | entity::Kind::FloatProperty => {
                            f.channel.is_some()
                        }
                        _ => true,
                    }
                }
            })
        {
            return Err(Error::Unsupported);
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
        let codec = Codec::new(
            vec![Catalog::new(layout.asset.bundle, &entries)?],
            vec![(layout.asset, layout.profile().map_err(|_| Error::Shape)?)],
        )?;
        Ok(Self { layout, codec })
    }
    pub(super) fn asset(&self) -> Asset {
        self.layout.asset
    }
    pub(super) fn spawn(&self, players: &mut Players, participant: u16) -> Result<Record, Error> {
        let world = players.objects();
        let scene = world
            .scene(self.layout.scene_content_key)
            .ok_or(Error::UnknownObject)?;
        let fields = self.layout.fields.iter().map(|f| {
            let binding = f.channel.as_ref().map(|channel| match *channel {
                Channel::Scene {
                    content_key,
                    serializer,
                } => Binding::Scene {
                    content_key,
                    serializer,
                },
                Channel::Participant { serializer } => Binding::Participant { serializer },
            });
            (f.kind, binding, None)
        });
        let creation = crate::event_logic::logic_creation_fields(
            self.layout.asset,
            (1, self.layout.bus_count),
            scene,
            participant,
            fields,
            |binding| match binding {
                Binding::Scene {
                    content_key,
                    serializer,
                } => world
                    .scene_endpoint(content_key, serializer)
                    .ok_or(Error::UnknownObject),
                _ => Err(Error::Unsupported),
            },
        )?;
        players
            .spawn_entities(vec![creation], &self.codec)?
            .pop()
            .ok_or(Error::Shape)
    }
}

/// Compile the resolved expression into the existing observer engine. These
/// identities are private compiler symbols, never wire IDs or invented assets.
/// Leaves retain their installed state endpoints and authored list order.
pub(super) fn compile(
    scope: &mut Scope,
    predicate: &Predicate,
    key: u32,
    blueprint: &str,
    counter: &mut usize,
) -> Result<Link, Error> {
    if *counter >= 128 {
        return Err(Error::Bound);
    }
    let identity = Identity {
        asset: format!("scene-controller:{blueprint}"),
        index: *counter,
        instances: vec![],
    };
    *counter += 1;
    match predicate {
        Predicate::Property(definition) => {
            let definition = definition.try_map(
                &mut |endpoint: &nfs_content::events::discovery::Endpoint| {
                    if endpoint.scene_key != key
                        || !scope
                            .states
                            .iter()
                            .any(|state| state.serializer == endpoint.serializer)
                    {
                        return Err(Error::UnknownObject);
                    }
                    Ok(nfs_content::events::lifecycle::Endpoint {
                        serializer: endpoint.serializer,
                        port: 0,
                    })
                },
            )?;
            let index = scope.bindings.properties.len();
            scope.bindings.properties.push(StateProperty {
                identity: identity.clone(),
                definition,
            });
            scope.bindings.order.push(Observer::Property(index));
        }
        Predicate::Boolean {
            operation,
            operands,
        } => {
            let operands = operands
                .iter()
                .map(|operand| compile(scope, operand, key, blueprint, counter))
                .collect::<Result<Vec<_>, _>>()?;
            let index = scope.bindings.predicates.len();
            scope
                .bindings
                .predicates
                .push(nfs_content::events::lifecycle::bindings::Predicate {
                    identity: identity.clone(),
                    kind: PredicateKind::Boolean {
                        operation: *operation,
                        operands,
                    },
                });
            scope.bindings.order.push(Observer::Predicate(index));
        }
    }
    Ok(Link {
        identity,
        port: 0,
        class: String::new(),
        channel: None,
    })
}
pub(super) fn install(
    scope: &mut Scope,
    predicate: &Predicate,
    key: u32,
    blueprint: &str,
) -> Result<(), Error> {
    let source = compile(scope, predicate, key, blueprint, &mut 0)?;
    let index = scope.bindings.spawners.len();
    scope.bindings.spawners.push(Spawner {
        identity: Identity {
            asset: format!("scene-spawner:{blueprint}"),
            index: 0,
            instances: vec![],
        },
        blueprint: blueprint.into(),
        occurrence: 0,
        controller: Some(Controller::Boolean(source)),
    });
    scope.bindings.order.push(Observer::Spawner(index));
    Ok(())
}
