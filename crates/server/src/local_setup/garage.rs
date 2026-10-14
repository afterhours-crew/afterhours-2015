// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! AllDrive lifecycle policy composed with installed layouts and event bindings.
//! IDs below are local role handles; the world allocates fresh live entity IDs.
use nfs_content::{
    assets::AssetIndex,
    participant_logic::{Channel, Layout, Owner, boolean::Input},
};
use nfs_world::replication::entity::Kind;
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const GAMEPLAY: &str = "levels/genesis01/gameplay";
const FADE: &str = "presentation/fadecontrolpresentationblueprint";
const GARAGE: &str = "gameplay/garageloadlogic";
const ALIVE: &str = "gameplay/alivestateplb";
const REP: &str = "gameplay/scoring/replevelcalculationlogic";
const FREE: &str = "gameplay/driverdetails/freeroamstatslogic";
const DRIVER: &str = "gameplay/driverdetails/driverdetailslogic";
const SESSION: &str = "gameplay/stats/sessionstatslogic";

fn boolean_state(name: &str, input: &Input) -> Option<bool> {
    match (name, input) {
        (GARAGE, Input::ParticipantSetContains(p)) if p.serializer == 1 && p.field == 733758294 => {
            Some(true)
        }
        // A new local AllDrive participant is outside racing, respawn and busted
        // contexts. This is the initial policy; later garage membership is owned
        // by the lifecycle's boolean updates, never a captured value.
        (ALIVE, Input::ChannelProperty(p)) if p.serializer == 2 && p.field == 3650958166 => {
            Some(false)
        }
        (ALIVE, Input::ParticipantSetContains(p)) if p.serializer == 2 && p.field == 188316878 => {
            Some(false)
        }
        (ALIVE, Input::ParticipantProperty(p))
            if p.serializer == 2 && [3602916778, 1545470794].contains(&p.field) =>
        {
            Some(false)
        }
        (
            ALIVE,
            Input::Context {
                channel,
                categories,
                ..
            },
        ) if channel.serializer == 1
            && channel.field == 3054616982
            && categories == &["gameplay/gamemodes/competitivemode/speedlistrace"] =>
        {
            Some(false)
        }
        _ => None,
    }
}
fn kind(k: Kind) -> Result<&'static str> {
    Ok(match k {
        Kind::Root => "root",
        Kind::RpcReference => "rpc_reference",
        Kind::Rpc => "rpc",
        Kind::BoolProperty => "bool_property",
        Kind::I32Property => "i32_property",
        Kind::FloatProperty => "float_property",
        Kind::RpcVariant => "rpc_variant",
        _ => return Err("unsupported local participant serializer".into()),
    })
}
fn reputation(program: &Json) -> Result<&'static str> {
    let ops = program.as_array().ok_or("integer program")?;
    let last = ops.last().ok_or("empty integer program")?;
    if let Some(output) = last["output"].as_str() {
        return match output {
            "level" => Ok("level"),
            "next_threshold" => Ok("next_threshold"),
            "current_threshold" => Ok("current_threshold"),
            _ => Err("unsupported reputation output".into()),
        };
    }
    match last["participant_property"]["port"].as_u64() {
        Some(193464962) => Ok("total"),
        Some(907756217) => Ok("build"),
        Some(3428983116) => Ok("crew"),
        Some(3289217787) => Ok("outlaw"),
        Some(1850256520) => Ok("speed"),
        Some(3705884152) => Ok("style"),
        _ => Err("unsupported reputation property".into()),
    }
}
fn roles(index: &mut AssetIndex, name: &str, layout: &Layout) -> Result<Vec<Json>> {
    let booleans = index
        .participant_boolean_values(name)?
        .into_iter()
        .map(|field| {
            let value = field
                .initial
                .as_ref()
                .map(|p| p.evaluate(|i| boolean_state(name, i)))
                .transpose()?;
            Ok((field.serializer, value))
        })
        .collect::<std::result::Result<BTreeMap<_, _>, nfs_content::Error>>()?;
    let ints = if name == REP {
        index
            .participant_integer_values(name)?
            .into_iter()
            .map(|f| {
                Ok((
                    f.serializer,
                    reputation(&f.initial.ok_or("missing reputation expression")?.to_json())?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?
    } else {
        BTreeMap::new()
    };
    layout.fields.iter().enumerate().map(|(i,field)|Ok(match field.kind {
        Kind::Root=>json!({"kind":"root","bus_count":layout.bus_count}),
        Kind::RpcReference=>match field.channel {
            Some(Channel::Scene{content_key,serializer})=>json!({"kind":"channel","scene_content_key":content_key,"scene_serializer":serializer}),
            _=>return Err("unexpected participant-owned channel".into()),
        },
        Kind::Rpc=>json!({"kind":"rpc"}),
        Kind::BoolProperty=>json!({"kind":"bool","value":booleans.get(&i).ok_or("missing Boolean expression")?}),
        Kind::I32Property if name==REP=>json!({"kind":"reputation","field":ints.get(&i).ok_or("missing reputation output")?}),
        // Event-gated session statistics have no owned sample at creation.
        // Absence is encoded as absence; a zero or captured score is not invented.
        Kind::I32Property=>json!({"kind":"int","value":null}),
        Kind::FloatProperty=>json!({"kind":"float","bits":null}),
        Kind::RpcVariant if name==REP=>json!({"kind":"reputation_level_stat"}),
        Kind::RpcVariant=>json!({"kind":"variant","tag":null,"value":null}),
        _=>return Err("unsupported local participant role".into()),
    })).collect()
}
fn batch() -> Json {
    json!({"scene_updates":[],"entities":[],"events":[],"entity_updates":[],"delete_capture_ids":[]})
}
fn push(doc: &mut Json, phase: &str, key: &str, value: Json) {
    doc[phase][key]
        .as_array_mut()
        .expect("authored batch")
        .push(value);
}
fn event(id: u16, event: &nfs_content::logic_events::Event) -> Json {
    json!({"target":{"capture_id":id},"entity":event.bus,"event":event.ordinal})
}

pub(super) fn build(index: &mut AssetIndex, thresholds: &Json) -> Result<Json> {
    let definitions = [
        (1, GAMEPLAY, FADE),
        (2, GAMEPLAY, REP),
        (3, GAMEPLAY, SESSION),
        (4, GAMEPLAY, "gameplay/tutorial/tutoriallogic_dragracevoice"),
        (5, GAMEPLAY, "gameplay/tutorial/tutoriallogic_dragrace"),
        (6, GAMEPLAY, "gameplay/tutorial/tutoriallogic_prestigemode"),
        (7, GAMEPLAY, "gameplay/tutorial/tutoriallogic_kickbacks"),
        (8, GAMEPLAY, "gameplay/tutorial/tutoriallogic_cops"),
        (
            9,
            GAMEPLAY,
            "gameplay/achievements/achievementlogic_update2",
        ),
        (
            10,
            "levels/genesis01/progression",
            "gameplay/progression/unlocksparticipantlogic",
        ),
        (
            11,
            "levels/genesis01/challenges",
            "gameplay/challenges/logic/rootchallengelogic",
        ),
        (12, "levels/genesis01/garage", GARAGE),
        (13, GAMEPLAY, "gameplay/lockvehicleinputparticipantlogic"),
        (
            14,
            GAMEPLAY,
            "presentation/loadingscreenstatepresentationblueprint",
        ),
        (15, GAMEPLAY, "presentation/suppressvoipblueprint"),
        (16, GAMEPLAY, "gameplay/stats/garagestatslogic"),
        (
            17,
            GAMEPLAY,
            "presentation/customizationstatepresentationblueprint",
        ),
        (18, GAMEPLAY, "gameplay/lockvehicleinputparticipantlogic"),
        (
            19,
            GAMEPLAY,
            "presentation/loadingscreenstatepresentationblueprint",
        ),
        (20, GAMEPLAY, "presentation/suppressvoipblueprint"),
        (21, GAMEPLAY, FREE),
        (22, "levels/genesis01/participantstatemachine", ALIVE),
        (23, GAMEPLAY, "gameplay/scoring/eventresultrep"),
        (24, GAMEPLAY, DRIVER),
    ];
    let mut doc = json!({"format":"nfs-garage-logic","version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "reputation_thresholds":thresholds});
    for phase in [
        "spawn",
        "vehicles",
        "entry",
        "exit",
        "arrive",
        "arrived",
        "world_ready",
        "world_spawn",
        "world_settled",
        "free_roam",
        "return_requested",
        "returning",
        "returned",
    ] {
        doc[phase] = batch();
    }
    let mut catalogs = BTreeMap::new();
    let mut profiles = BTreeMap::new();
    let mut entities = BTreeMap::new();
    let mut garage_field = None;
    for (id, scene, name) in definitions {
        let layout = index.participant_layout(scene, name, Owner::Scene)?;
        let asset = json!({"bundle":layout.asset.bundle,"type_id":layout.asset.type_id,"local_index":layout.asset.local_index});
        let values = roles(index, name, &layout)?;
        if name == GARAGE {
            garage_field = values.iter().position(|r| r["kind"] == "bool");
        }
        let entity = json!({"capture_id":id,"asset":asset,"blueprint":{"scene_content_key":layout.scene_content_key},"roles":values});
        catalogs.insert(
            layout.asset.bundle,
            json!({"bundle":layout.asset.bundle,"entries":layout.catalog}),
        );
        profiles.insert((layout.asset.bundle,layout.asset.type_id,layout.asset.local_index),json!({"asset":asset,"serializers":layout.fields.iter().map(|f|kind(f.kind)).collect::<Result<Vec<_>>>()?}));
        entities.insert(id, entity);
    }
    for (phase, ids) in [
        ("spawn", (1..=16).collect::<Vec<_>>()),
        ("entry", vec![17]),
        ("exit", vec![18, 19, 20, 21]),
        ("arrived", vec![22, 23, 24]),
        ("returning", vec![20]),
        ("returned", vec![13, 14, 15, 16]),
    ] {
        for id in ids {
            push(&mut doc, phase, "entities", entities[&id].clone());
        }
    }
    for (phase, ids) in [
        ("entry", vec![13, 14, 15]),
        ("exit", vec![16, 17]),
        ("arrive", vec![18, 19]),
        ("arrived", vec![20]),
        ("returning", vec![23, 22, 21, 24]),
        ("returned", vec![20]),
    ] {
        doc[phase]["delete_capture_ids"] = json!(ids);
    }
    for (phase, value) in [("exit", false), ("returning", true)] {
        push(
            &mut doc,
            phase,
            "entity_updates",
            json!({"capture_id":12,"field":garage_field.ok_or("garage membership field")?,"bool":value}),
        );
    }
    let fade = index.network_events(FADE)?;
    let select_fade = |key: &str, active: bool| -> Result<Json> {
        let matches = fade
            .iter()
            .filter(|e| {
                e.target.presentation_key.as_deref() == Some(key)
                    && e.target.port == if active { 3735685596 } else { 1928776733 }
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err("ambiguous fade context event".into());
        }
        Ok(event(1, matches[0]))
    };
    for (phase, events) in [
        ("spawn", vec![("BlackScreen", true)]),
        (
            "vehicles",
            vec![("BlackScreen", false), ("BlackScreen", true)],
        ),
        (
            "entry",
            vec![
                ("BlackScreen", false),
                ("ViewWorld", true),
                ("ViewWorld", false),
                ("ViewWorld", true),
            ],
        ),
        ("arrive", vec![("BlackScreen", false)]),
        (
            "world_ready",
            vec![
                ("ViewWorld", false),
                ("BlackScreen", true),
                ("BlackScreen", false),
            ],
        ),
        ("world_spawn", vec![("BlackScreen", true)]),
        ("returning", vec![("ViewWorld", false)]),
    ] {
        for (key, active) in events {
            push(&mut doc, phase, "events", select_fade(key, active)?);
        }
    }
    for (phase, id, name) in [
        ("spawn", 3, SESSION),
        ("world_settled", 21, FREE),
        ("free_roam", 24, DRIVER),
    ] {
        let mut events = index
            .network_events(name)?
            .into_iter()
            .filter(|e| e.target.class == "PropertyDebugEntityData" && e.target.port == 2089152613)
            .collect::<Vec<_>>();
        if events.is_empty() {
            return Err("missing statistics client events".into());
        }
        events.sort_by_key(|e| (e.bus, std::cmp::Reverse(e.ordinal)));
        for e in events {
            push(&mut doc, phase, "events", event(id, &e));
        }
    }
    push(
        &mut doc,
        "free_roam",
        "events",
        select_fade("ViewWorld", true)?,
    );
    let vehicle_ready = index
        .network_events(GAMEPLAY)?
        .into_iter()
        .filter(|e| {
            e.source.asset == "levels/genesis01/gameplay/layer0_gameplay_schematic"
                && e.source.class == "PropertyChangedEntityData"
                && e.source.port == 613089920
                && e.target.class == "CompareBoolEntityData"
                && e.target.port == 5862146
        })
        .collect::<Vec<_>>();
    if vehicle_ready.len() != 1 {
        return Err("ambiguous vehicle readiness event".into());
    }
    push(
        &mut doc,
        "vehicles",
        "events",
        json!({"target":{"scene_content_key":nfs_content::assets::scene_key(GAMEPLAY)},"entity":vehicle_ready[0].bus,"event":vehicle_ready[0].ordinal}),
    );
    let alive = index
        .network_events(ALIVE)?
        .into_iter()
        .filter(|e| {
            e.target.blueprint.as_deref() == Some("gameplay/clienttriggeredbooltransition")
                && e.target.port == 2606354109
        })
        .collect::<Vec<_>>();
    if alive.len() != 1 {
        return Err("ambiguous garage return event".into());
    }
    push(&mut doc, "return_requested", "events", event(22, &alive[0]));
    doc["content"] = json!({"catalogs":catalogs.into_values().collect::<Vec<_>>(),"profiles":profiles.into_values().collect::<Vec<_>>()});
    crate::garage_logic::GarageLogic::from_json(&doc)
        .map_err(|_| "generated garage lifecycle failed validation")?;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_state_and_reputation_inputs_are_not_replaced_with_defaults() {
        let input = Input::ParticipantSetContains(nfs_content::participant_logic::boolean::Port {
            serializer: 1,
            field: 733758294,
        });
        assert_eq!(boolean_state(GARAGE, &input), Some(true));
        assert_eq!(boolean_state(ALIVE, &input), None);
        assert!(reputation(&json!([{"participant_property":{"channel":1,"port":99}}])).is_err());
        assert_eq!(
            reputation(&json!([{"participant_property":{"channel":1,"port":193464962}}])).unwrap(),
            "total"
        );
    }
}
