// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Creation-realm eligibility, separately from listener and connection realms.
//! Eligibility does not prove that a factory exists or that a node is activated.
use super::{events::Identity, realms::Realm};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad},
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::scoring::events::MAX_NODES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub class: String,
    pub realm: Realm,
}

/// Read the creation realm for the supported scoring classes. Some classes
/// deliberately have a fixed realm despite also declaring an authored Realm.
/// Flags describe listeners and are never used as a construction filter here.
/// None identifies a known blueprint/interface, not an unknown entity class.
pub fn compile(object: &Object) -> Result<Option<Realm>, Error> {
    let realm = match object.class.as_str() {
        "LogicPrefabBlueprint" | "ParticipantLogicBlueprint" | "InterfaceDescriptorData" => {
            return Ok(None);
        }
        "VehicleProximitySetFilterEntityData"
        | "RaceVehicleDonutQueryEntityData"
        | "RaceVehicleDraftQueryEntityData"
        | "RaceVehicleNosBuildupEntityData"
        | "RaceVehicleNosQueryEntityData"
        | "PlayerInfoAccessorEntityData"
        | "BoolParticipantPropertyAccessorEntityData"
        | "IntParticipantPropertyAccessorEntityData"
        | "ParticipantSetCounterEntityData"
        | "AchievementEntityData" => Realm::Server,
        "BoolPresentationDataCollectorEntityData"
        | "FloatPresentationDataCollectorEntityData"
        | "IntPresentationDataCollectorEntityData"
        | "DirectPresentationContextEntityData"
        | "RaceVehicleAnyCollisionEvaluatorEntityData"
        | "RaceVehicleGroupedCollisionEvaluatorEntityData"
        | "RaceVehicleIndividualPropCollisionEvaluatorEntityData" => Realm::Client,
        "LogicPrefabReferenceObjectData"
        | "SyncedBoolEntityData"
        | "SyncedFloatEntityData"
        | "SyncedIntEntityData"
        | "LinkedSchematicChannelEntityData"
        | "LinkedSchematicChannelReferenceEntityData"
        | "LinkedSchematicChannelRelayEntityData"
        | "RaceVehicleAccumulatingCollisionsQueryEntityData"
        | "RaceVehicleRoadDrivingQueryEntityData"
        | "RaceVehicleSlamAParticipantCollisionEvaluatorEntityData"
        | "RaceVehicleCollisionQueryEntityData"
        | "RaceVehicleNearMissQueryEntityData"
        | "ParticipantLogicEntityData" => Realm::Both,
        "AndEntityData"
        | "BoolEntityData"
        | "EventGateEntityData"
        | "FloatCacheEntityData"
        | "FloatEntityData"
        | "IntEntityData"
        | "MathEntityData"
        | "MathOpEntityData"
        | "OrEntityData"
        | "PropertyCastEntityData"
        | "PropertyGateEntityData"
        | "StopWatchEntityData"
        | "VectorMathOpEntityData"
        | "CompareBoolEntityData"
        | "CompareFloatEntityData"
        | "CompareIntEntityData"
        | "EventSplitterEntityData"
        | "PropertyDebugEntityData"
        | "CurveEntityData"
        | "BoolCacheEntityData"
        | "IntCacheEntityData"
        | "FloatAccumulatorEntityData"
        | "PropertyEncodeEntityData"
        | "ConditionalFloatEntityData"
        | "RaceVehicleDriftQueryEntityData"
        | "RaceVehiclePhysicsQueryEntityData"
        | "ParticipantLogicParticipantProviderEntityData"
        | "ScoreContextEntityData"
        | "ScoreEntityData"
        | "VehicleStatsProviderEntityData"
        | "ParticipantLogicControllableProviderEntityData"
        | "TelemetryHookEntityData"
        | "DelayEntityData" => match object.fields.get("Realm") {
            Some(Value::Enum {
                value,
                name: Some(name),
            }) => match (*value, name.as_str()) {
                (0, "Realm_Client") => Realm::Client,
                (1, "Realm_Server") => Realm::Server,
                (2, "Realm_ClientAndServer") => Realm::Both,
                (3, "Realm_None") => Realm::Neither,
                _ => return Err(bad("unsupported scoring creation realm")),
            },
            _ => return Err(bad("scoring creation realm enum")),
        },
        _ => return Err(bad("unsupported scoring creation class")),
    };
    Ok(Some(realm))
}

pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    if blueprint.is_empty() || blueprint.len() > 512 {
        return Err(bad("scoring blueprint name"));
    }
    let rows = Graph::new(source).read(Site::root(blueprint))?;
    if rows.len() > MAX_NODES {
        return Err(bad("scoring creation node bound"));
    }
    let mut result = Vec::new();
    for row in rows {
        if let Some(realm) = compile(&row.object)
            .map_err(|e| bad(&format!("{}:{}: {e}", row.site.key.0, row.site.key.1)))?
        {
            result.push(Node {
                identity: Identity::from(&row.site),
                class: row.object.class,
                realm,
            });
        }
    }
    Ok(result)
}
impl AssetIndex {
    /// Read creation eligibility for every entity occurrence. The result keeps
    /// composition traversal order and repeated prefab instances. It is not a
    /// factory completion or activation schedule; those require node behavior.
    pub fn scoring_creation_realms(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        read(&mut self.assets, blueprint)
    }
}

#[cfg(test)]
mod tests;
