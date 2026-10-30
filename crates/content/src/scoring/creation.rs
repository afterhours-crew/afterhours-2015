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

/// Creation eligibility and an available server factory are separate gates.
/// The supported build has no registered factory for PropertyDebug or
/// PrintDebugText in their two-slot tables (02c0e050 / 02c0df40). The adjacent
/// slots belong to WheelComponent and LensFlare, not a third realm. Retain the
/// authored realm for inspection rather than relabeling these as client nodes.
pub(crate) fn server_factory(class: &str) -> bool {
    !matches!(
        class,
        "PropertyDebugEntityData" | "PrintDebugTextEntityData"
    )
}

/// Read the creation realm for the supported scoring classes. Some classes
/// deliberately have a fixed realm despite also declaring an authored Realm.
/// Flags describe listeners and are never used as a construction filter here.
/// None identifies a known blueprint/interface, not an unknown entity class.
pub fn compile(object: &Object) -> Result<Option<Realm>, Error> {
    let realm = match object.class.as_str() {
        "LogicPrefabBlueprint"
        | "ParticipantLogicBlueprint"
        | "GameModeBlueprint"
        | "ParticipantStateSequenceBlueprint"
        | "UIWidgetBlueprint"
        | "InterfaceDescriptorData" => {
            return Ok(None);
        }
        "ControlAiInGameModeStateEntityData"
        | "ItemSelectorEntityData"
        | "VehicleSlotLoaderEntityData"
        | "ParticipantEventEntityData"
        | "ParticipantProviderStateReporterEntityData"
        | "BoolParticipantPropertyEntityData"
        | "BooleanOperationPredicateEntityData"
        | "CompoundStatesReporterEntityData"
        | "ControlAiInParticipantStateEntityData"
        | "DistanceReporterEntityData"
        | "FloatParticipantPropertyEntityData"
        | "GameModeParticipantContextEntityData"
        | "IntParticipantPropertyEntityData"
        | "IntParticipantPropertyMirrorProviderEntityData"
        | "IntParticipantPropertySorterEntityData"
        | "PacingScheduleProviderEntityData"
        | "ParticipantEventChannelEntityData"
        | "ParticipantEventToParticipantProviderEntityData"
        | "ParticipantSetBoolPropertyAdapterEntityData"
        | "ParticipantSetEntityData"
        | "ParticipantSetCopyEntityData"
        | "ParticipantSetPlayerAdapterEntityData"
        | "ParticipantSetStateAdapterEntityData"
        | "ParticipantStateListBoolParticipantPropertyEntityData"
        | "ParticipantStateRangeBoolParticipantPropertyEntityData"
        | "ResultEntityData"
        | "ServerUpdateRecentPlayersEntityData"
        | "TriggeredPredicateEntityData"
        | "WanderCommandEntityData"
        | "WaypointChannelGotoWaypointCommandEntityData"
        | "WaypointSeriesPacingGroupCommandEntityData"
        | "WaypointTelemetryEntityData"
        | "EnableAiOnParticipantEntityData"
        | "FloatParticipantPropertyAccessorEntityData"
        | "GameIdProviderEntityData"
        | "GotoWaypointCommandEntityData"
        | "ItemInfoAccessorEntityData"
        | "ParticipantEventChannelReferenceEntityData"
        | "ParticipantLogicParticipantTransitionEntityData"
        | "ParticipantProviderEventFilterEntityData"
        | "ParticipantProviderToParticipantEventEntityData"
        | "ParticipantSetAccessorEntityData"
        | "ParticipantSetAdapterEntityData"
        | "ParticipantSetPredicateEntityData"
        | "RaceVehicleBurnoutQueryEntityData"
        | "RaceVehicleInfoQueryEntityData"
        | "ScriptedInputCommandEntityData"
        | "SetTeleportLocationEntityData"
        | "VehicleTuningQueryEntityData"
        | "VehicleProximitySetFilterEntityData"
        | "RaceVehicleDonutQueryEntityData"
        | "RaceVehicleDraftQueryEntityData"
        | "RaceVehicleNosBuildupEntityData"
        | "RaceVehicleNosQueryEntityData"
        | "PlayerInfoAccessorEntityData"
        | "BoolParticipantPropertyAccessorEntityData"
        | "IntParticipantPropertyAccessorEntityData"
        | "SortAndRankIntParticipantPropertyEntityData"
        | "ParticipantSetCounterEntityData"
        | "AchievementEntityData" => Realm::Server,
        "BoolPresentationDataProviderEntityData"
        | "ContextInterfaceEntityData"
        | "MixerEntityData"
        | "BasicControllerEntityData"
        | "CinebotCameraModeEntityData"
        | "CinebotCameraProxyEntityData"
        | "CinebotStateLogicEntityData"
        | "EventPresentationDataCollectorEntityData"
        | "FadeEntityData"
        | "FixedArmTransposerEntityData"
        | "FixedTargetComposerEntityData"
        | "MenuContainerWidgetData"
        | "MenuControllerEntityData"
        | "MovieCompositorEntityData"
        | "MovieEntityData"
        | "NoiseControllerEntityData"
        | "PlayVideoEntityData"
        | "UIInputActionHandlerEntityData"
        | "UIScreenRenderEntityData"
        | "UIWidgetEntityData"
        | "CheckNewEventEntityData"
        | "EventOutroPresentationDataCollectorEntityData"
        | "ParticipantPlayerTypeFilterEntityData"
        | "RecommendationDataSupplierEntityData"
        | "RecommendationPresentationDataCollectorEntityData"
        | "StringBuilderLocalizationEntityData"
        | "TimerPresentationDataCollectorEntityData"
        | "WaypointsGameModeStateReporterEntityData"
        | "WaypointsPresentationDataCollectorEntityData"
        | "EventStartPromptSenderEntityData"
        | "IntegerLocalizationEntityData"
        | "LocalPlayerVehicleInterfaceEntityData"
        | "PresentationContextOutputEntityData"
        | "PresentationTreeResolverEntityData"
        | "RaceVehicleModifierEntityData"
        | "RaceVehicleSolidWorldCollisionEvaluatorEntityData"
        | "RestrictedRaceNotificationSenderEntityData"
        | "SimpleLocalizationEntityData"
        | "SimplePromptSenderEntityData"
        | "SoundEntityData"
        | "TelemetryUtilEntityData"
        | "TextPresentationDataCollectorEntityData"
        | "TimeLocalizationEntityData"
        | "BoolPresentationDataCollectorEntityData"
        | "FloatPresentationDataCollectorEntityData"
        | "IntPresentationDataCollectorEntityData"
        | "DirectPresentationContextEntityData"
        | "RaceVehicleAnyCollisionEvaluatorEntityData"
        | "RaceVehicleGroupedCollisionEvaluatorEntityData"
        | "RaceVehicleIndividualPropCollisionEvaluatorEntityData" => Realm::Client,
        "EventSyncEntityData"
        | "ParticipantStateSequenceEntityData"
        | "AiParticipantFilterTransitionEntityData"
        | "BoolPropertyClientNotificationEntityData"
        | "ChannelWaypointListProviderEntityData"
        | "CountdownGameModeStateEntityData"
        | "DNFGameModeStateTransitionEntityData"
        | "DNFTriggeredGameModeStateTransitionEntityData"
        | "EventDetailsChannelReferenceEntityData"
        | "EventInfoPresentationDataCollectorEntityData"
        | "EventOutroPlayerProviderEntityData"
        | "FinishLineGameModeStateTransitionEntityData"
        | "GameModeEntityData"
        | "GameModeStateEntityData"
        | "IntParticipantPropertyClientNotificationEntityData"
        | "NotificationCollectorChannelReferenceEntityData"
        | "ParticipantLogicSpawnerEntityData"
        | "ParticipantLogicSpawnerSetControlledEntityData"
        | "ParticipantSequenceSpawnerGameModeStateEntityData"
        | "ParticipantStateParticipantContextEntityData"
        | "ParticipantTransitionEntityData"
        | "SimpleGameModeStateEntityData"
        | "SpawnAiForGameModeStateEntityData"
        | "StateReporterEntityData"
        | "StateSequenceChannelReferenceEntityData"
        | "UpdateSetEntityData"
        | "UpdateableManagerEntityData"
        | "UpdateableParticipantStateEntityData"
        | "VolumeParticipantSetFilterEntityData"
        | "WaitForAllAiToSpawnStateEntityData"
        | "WaypointListChannelReferenceEntityData"
        | "WaypointsGameModeStateEntityData"
        | "GotoWaypointCommandPrecisionEntityData"
        | "IntParticipantPropertySorterQueryByIndexEntityData"
        | "ParticipantNameProviderEntityData"
        | "ParticipantStateEntityData"
        | "SyncedStringEntityData"
        | "TriggeredParticipantTransitionEntityData"
        | "LogicPrefabReferenceObjectData"
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
        "BankTransactionEntityData"
        | "AwardItemTransactionEntityData"
        | "StateSequenceParticipantProviderEntityData"
        | "BoolHubEntityData"
        | "ConsoleCommandEntityData"
        | "PropertyStackEntryEntityData"
        | "SelectEventEntityData"
        | "SequenceEntityData"
        | "SettingEntityData"
        | "TransformBuilderEntityData"
        | "TransformHubEntityData"
        | "Vec2HubEntityData"
        | "Vector3EntityData"
        | "CompareStringEntityData"
        | "DurangoEventListenerEntityData"
        | "VehicleResetRouteEntityData"
        | "ClampEntityData"
        | "CompareFloatRangeEntityData"
        | "CompareIntRangeEntityData"
        | "CompareUInt64EntityData"
        | "ConsoleCommandTriggerEntityData"
        | "CurveWrapperEntityData"
        | "EventDetailsInfoAccessorEntityData"
        | "FloatHubEntityData"
        | "FloatSelectEntityData"
        | "IntHubEntityData"
        | "IntegerDeltaEntityData"
        | "NotEntityData"
        | "ParticipantContextReporterEntityData"
        | "PrintDebugTextEntityData"
        | "RaceTimeProviderEntityData"
        | "RaceVehicleBurnoutTendencyOverrideEntityData"
        | "RandomFloatEntityData"
        | "ScaleClampEntityData"
        | "StringBuilderEntityData"
        | "StringEntityData"
        | "SwitchPropertyFloatEntityData"
        | "TimelineEntityData"
        | "TransformSplitterEntityData"
        | "UnsignedPropertyCastEntityData"
        | "Vec2BuilderEntityData"
        | "AndEntityData"
        | "BoolEntityData"
        | "EventGateEntityData"
        | "EventAndGateEntityData"
        | "FloatCacheEntityData"
        | "FloatEntityData"
        | "IntEntityData"
        | "MathEntityData"
        | "MathOpEntityData"
        | "MathIntOpEntityData"
        | "RankIntegersEntityData"
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
        | "ConditionalIntEntityData"
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
