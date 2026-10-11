// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::{autolog, speedlist, stats};
use nfs_services::control_catalogs::{self as catalogs, Catalog, Error};
use serde_json::{Value, json};

fn config() -> Value {
    json!({"format":"nfs-control-catalogs","version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "time_limited_features":"disabled", "kill_switches":["example.disabled"],
        "key_scopes":[{"name":"ExampleScope","aggregate_key":-1,"aggregate":true,"values":[[-5,i64::MIN],[7,i64::MAX]]}],
        "speed_list_types":[{"id":u32::MAX,"description":"example.description","name":"example.name","texture":"example.texture"}]})
}
fn wire(route: (u16, u16), body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: route.0,
                routing_b: route.1,
                correlation: 42,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        catalogs::frame_limits(),
    )
    .unwrap()
}
fn queries(persona: i64) -> Vec<Vec<u8>> {
    vec![
        wire((7, 15), &[]),
        wire(
            (2050, 75),
            &autolog::KillSwitchRequest {
                blaze_id: Some(0),
                ..Default::default()
            }
            .encode(catalogs::body_limits())
            .unwrap(),
        ),
        wire(
            (2050, 78),
            &autolog::GetTimeLimitedFeaturesRequest {
                deda: Some(b""),
                ..Default::default()
            }
            .encode(catalogs::body_limits())
            .unwrap(),
        ),
        wire(
            (2055, 1),
            &speedlist::SpeedListTypeRequest {
                blaze_id: Some(persona),
                ..Default::default()
            }
            .encode(catalogs::body_limits())
            .unwrap(),
        ),
    ]
}
fn response_body(bytes: &[u8], route: (u16, u16)) -> &[u8] {
    let d = nfs_fire2::decode(bytes, catalogs::frame_limits())
        .unwrap()
        .unwrap();
    assert_eq!(d.consumed, bytes.len());
    assert_eq!(
        (
            d.frame.fields.routing_a,
            d.frame.fields.routing_b,
            d.frame.fields.category,
            d.frame.fields.correlation
        ),
        (route.0, route.1, 1, 42)
    );
    assert!(d.frame.metadata.is_empty());
    d.frame.body
}
#[test]
fn definitions_preserve_order_full_width_values_and_current_persona() {
    let catalog = Catalog::from_json(&config()).unwrap();
    for persona in [17, i64::MAX] {
        let answers: Vec<_> = queries(persona)
            .iter()
            .map(|q| catalog.reply(q, persona).unwrap())
            .collect();
        let scopes =
            stats::KeyScopes::decode(response_body(&answers[0], (7, 15)), catalogs::body_limits())
                .unwrap()
                .key_scopes_map
                .unwrap()
                .0;
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].0, b"ExampleScope");
        assert_eq!(scopes[0].1.aggregate_key_value, Some(-1));
        assert_eq!(scopes[0].1.enable_aggregation, Some(true));
        assert_eq!(
            scopes[0].1.key_scope_values.as_ref().unwrap().0,
            vec![(-5, i64::MIN), (7, i64::MAX)]
        );
        let switches = autolog::KillSwitchResponse::decode(
            response_body(&answers[1], (2050, 75)),
            catalogs::body_limits(),
        )
        .unwrap();
        assert_eq!(
            switches.kill_switch_list.unwrap().0,
            vec![b"example.disabled".as_slice()]
        );
        assert!(response_body(&answers[2], (2050, 78)).is_empty());
        let types = speedlist::SpeedListTypeResponse::decode(
            response_body(&answers[3], (2055, 1)),
            catalogs::body_limits(),
        )
        .unwrap();
        assert_eq!(types.blaze_id, Some(persona));
        let rows = types.speed_list_types.unwrap().0;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, u32::MAX);
        assert_eq!(rows[0].1.speed_list_type_id, Some(u32::MAX));
        assert_eq!(
            rows[0].1.type_description_string_id,
            Some(b"example.description".as_slice())
        );
        assert_eq!(
            rows[0].1.type_localised_name_string_id,
            Some(b"example.name".as_slice())
        );
        assert_eq!(
            rows[0].1.type_texture_id,
            Some(b"example.texture".as_slice())
        );
        for (q, answer) in queries(persona).iter().zip(answers) {
            assert_eq!(catalog.reply(q, persona).unwrap(), answer);
        }
    }
}
#[test]
fn shared_catalog_has_no_account_or_session_state() {
    let catalog = std::sync::Arc::new(Catalog::from_json(&config()).unwrap());
    let threads: Vec<_> = [11, 22]
        .into_iter()
        .map(|persona| {
            let catalog = catalog.clone();
            std::thread::spawn(move || {
                let query = queries(persona).pop().unwrap();
                for _ in 0..8 {
                    let answer = catalog.reply(&query, persona).unwrap();
                    let value = speedlist::SpeedListTypeResponse::decode(
                        response_body(&answer, (2055, 1)),
                        catalogs::body_limits(),
                    )
                    .unwrap();
                    assert_eq!(value.blaze_id, Some(persona));
                    assert_eq!(catalog.reply(&query, persona + 1), Err(Error::Ineligible));
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}
#[test]
fn unsupported_query_forms_and_unknown_fields_receive_no_answer() {
    let catalog = Catalog::from_json(&config()).unwrap();
    let bad = [
        wire((7, 15), &[0, 0, 0, 0, 0]),
        wire(
            (2050, 75),
            &autolog::KillSwitchRequest {
                blaze_id: Some(11),
                ..Default::default()
            }
            .encode(catalogs::body_limits())
            .unwrap(),
        ),
        wire(
            (2050, 78),
            &autolog::GetTimeLimitedFeaturesRequest {
                deda: Some(b"date"),
                ..Default::default()
            }
            .encode(catalogs::body_limits())
            .unwrap(),
        ),
        wire(
            (2055, 1),
            &speedlist::SpeedListTypeRequest {
                blaze_id: Some(11),
                list_of_speed_list_types_requested: Some(speedlist::TypeIds(vec![])),
                ..Default::default()
            }
            .encode(catalogs::body_limits())
            .unwrap(),
        ),
        wire((1, 29), &[]),
    ];
    for query in bad {
        assert_eq!(catalog.reply(&query, 11), Err(Error::Ineligible));
    }
    for query in queries(11) {
        let frame = nfs_fire2::decode(&query, catalogs::frame_limits())
            .unwrap()
            .unwrap()
            .frame;
        let mut body = frame.body.to_vec();
        let mut extra = nfs_heat2::Encoder::new(catalogs::body_limits());
        extra.integer([1, 2, 3], 1).unwrap();
        body.extend(extra.finish().unwrap());
        assert_eq!(
            catalog.reply(
                &wire((frame.fields.routing_a, frame.fields.routing_b), &body),
                11
            ),
            Err(Error::Ineligible)
        );
    }
}
#[test]
fn partial_concatenated_and_invalid_envelopes_are_rejected() {
    let catalog = Catalog::from_json(&config()).unwrap();
    for query in queries(11) {
        for end in 0..query.len() {
            assert_eq!(catalog.reply(&query[..end], 11), Err(Error::Ineligible));
        }
        let mut concat = query.clone();
        concat.extend(&query);
        assert_eq!(catalog.reply(&concat, 11), Err(Error::Ineligible));
        assert_eq!(catalog.reply(&query, 0), Err(Error::Ineligible));
        let frame = nfs_fire2::decode(&query, catalogs::frame_limits())
            .unwrap()
            .unwrap()
            .frame;
        for fields in [
            Fields {
                category: 1,
                ..frame.fields
            },
            Fields {
                slot: 1,
                ..frame.fields
            },
            Fields {
                reserved: [1, 0],
                ..frame.fields
            },
        ] {
            let bad =
                nfs_fire2::encode(Frame { fields, ..frame }, catalogs::frame_limits()).unwrap();
            assert_eq!(catalog.reply(&bad, 11), Err(Error::Ineligible));
        }
        let bad = nfs_fire2::encode(
            Frame {
                metadata: &[0],
                ..frame
            },
            nfs_fire2::Limits::new(40000, 16, 32768).unwrap(),
        )
        .unwrap();
        assert_eq!(catalog.reply(&bad, 11), Err(Error::Ineligible));
        let mut bad = query.clone();
        bad[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(catalog.reply(&bad, 11), Err(Error::Ineligible));
    }
}
#[test]
fn configuration_rejects_ambiguous_duplicates_unknown_fields_and_bad_bounds() {
    let original = config();
    let invalid = |v: Value| assert!(Catalog::from_json(&v).is_err());
    for key in [
        "format",
        "version",
        "build_sha256",
        "time_limited_features",
        "key_scopes",
        "kill_switches",
        "speed_list_types",
    ] {
        let mut v = original.clone();
        v.as_object_mut().unwrap().remove(key);
        invalid(v);
    }
    for key in ["key_scopes", "kill_switches", "speed_list_types"] {
        let mut v = original.clone();
        let duplicate = v[key][0].clone();
        v[key].as_array_mut().unwrap().push(duplicate);
        invalid(v);
    }
    let mut v = original.clone();
    v["request"] = json!({});
    invalid(v);
    let mut v = original.clone();
    v["version"] = json!(2);
    invalid(v);
    let mut v = original.clone();
    v["time_limited_features"] = json!("enabled");
    invalid(v);
    let mut v = original.clone();
    v["key_scopes"][0]["values"] = json!([[1, 2], [1, 3]]);
    invalid(v);
    let mut v = original.clone();
    v["key_scopes"][0]["aggregate"] = json!(0);
    invalid(v);
    let mut v = original.clone();
    v["key_scopes"][0]["values"] = json!([[1]]);
    invalid(v);
    let mut v = original.clone();
    v["key_scopes"][0]["values"] = json!([[1, 2, 3]]);
    invalid(v);
    let mut v = original.clone();
    v["key_scopes"][0]["name"] = json!("x".repeat(128));
    invalid(v);
    let mut v = original.clone();
    v["speed_list_types"][0]["id"] = json!(u64::MAX);
    invalid(v);
    let mut v = original.clone();
    v["speed_list_types"][0]["texture"] = json!("bad\0value");
    invalid(v);
    let mut v = original.clone();
    v["kill_switches"] = json!((0..65).map(|i| format!("switch{i}")).collect::<Vec<_>>());
    invalid(v);
    let mut v = original.clone();
    v["key_scopes"][0]["values"] = json!((0..257).map(|i| [i, i]).collect::<Vec<_>>());
    invalid(v);
    let mut v = original.clone();
    v["key_scopes"]=json!((0..5).map(|i|json!({"name":format!("scope{i}"),"aggregate_key":0,"aggregate":false,"values":(0..256).map(|j|[j,j]).collect::<Vec<_>>()})).collect::<Vec<_>>());
    invalid(v);
    let mut v = original;
    v["key_scopes"] = json!([]);
    v["kill_switches"] = json!([]);
    v["speed_list_types"] = json!([]);
    let catalog = Catalog::from_json(&v).unwrap();
    for q in queries(11) {
        assert!(catalog.reply(&q, 11).is_ok());
    }
}
