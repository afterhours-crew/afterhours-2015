// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::items::*;
use nfs_services::ContentError;
use nfs_services::item_licenses::{Content, Error, License, body_limits, frame_limits};
use serde_json::json;

fn declaration(system: &[u8], licenses: &[(&[u8], &[u8])], corr: u32) -> Vec<u8> {
    let body = EnsurePlayerInventoryRequest {
        item_system_name: Some(system),
        available_licenses: Some(LicenseSources(
            licenses
                .iter()
                .map(|(license, source)| LicenseSourceData {
                    license: Some(license),
                    source: Some(source),
                    ..Default::default()
                })
                .collect(),
        )),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2052,
                routing_b: 19,
                correlation: corr,
                ..Default::default()
            },
            metadata: &[],
            body: &body,
        },
        frame_limits(),
    )
    .unwrap()
}
fn document() -> serde_json::Value {
    json!({"format":"nfs-item-licenses","version":1,
        "build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "item_system":"Items/GameItemSystem",
        "licenses":[{"license":"NFS12","source":"Entitlement"},{"license":"NFS13","source":"Entitlement"}]})
}
const LICENSES: &[(&[u8], &[u8])] = &[(b"NFS12", b"Entitlement"), (b"NFS13", b"Entitlement")];

#[test]
fn the_configured_declaration_is_acknowledged_with_an_empty_response() {
    let content = Content::from_json(&document()).unwrap();
    assert_eq!(content.item_system(), "Items/GameItemSystem");
    assert_eq!(content.licenses().len(), 2);
    let reply = content
        .reply(&declaration(b"Items/GameItemSystem", LICENSES, 20))
        .unwrap();
    let d = nfs_fire2::decode(&reply, frame_limits()).unwrap().unwrap();
    assert_eq!(d.consumed, reply.len());
    assert_eq!(
        (
            d.frame.fields.routing_a,
            d.frame.fields.routing_b,
            d.frame.fields.category,
            d.frame.fields.correlation
        ),
        (2052, 19, 1, 20)
    );
    assert!(d.frame.body.is_empty() && d.frame.metadata.is_empty());
    assert_eq!(
        content
            .reply(&declaration(b"Items/GameItemSystem", LICENSES, 20))
            .unwrap(),
        reply
    );
}

#[test]
fn other_declarations_and_malformed_frames_are_not_acknowledged() {
    let content = Content::from_json(&document()).unwrap();
    for (system, licenses) in [
        (b"Items/Other".as_slice(), LICENSES),
        (b"Items/GameItemSystem", &LICENSES[..1]),
        (
            b"Items/GameItemSystem",
            &[
                (b"NFS13".as_slice(), b"Entitlement".as_slice()),
                (b"NFS12", b"Entitlement"),
            ],
        ),
        (
            b"Items/GameItemSystem",
            &[
                (b"NFS12".as_slice(), b"Store".as_slice()),
                (b"NFS13", b"Entitlement"),
            ],
        ),
    ] {
        assert_eq!(
            content.reply(&declaration(system, licenses, 1)),
            Err(Error::Unsupported)
        );
    }
    let q = declaration(b"Items/GameItemSystem", LICENSES, 1);
    assert_eq!(
        content.reply(&[q.clone(), q.clone()].concat()),
        Err(Error::Ineligible)
    );
    for split in 0..q.len() {
        assert_eq!(content.reply(&q[..split]), Err(Error::Ineligible));
    }
    let foreign = nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2052,
                routing_b: 23,
                ..Default::default()
            },
            metadata: &[],
            body: &[],
        },
        frame_limits(),
    )
    .unwrap();
    assert_eq!(content.reply(&foreign), Err(Error::Ineligible));
    let empty = nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2052,
                routing_b: 19,
                ..Default::default()
            },
            metadata: &[],
            body: &[],
        },
        frame_limits(),
    )
    .unwrap();
    assert_eq!(content.reply(&empty), Err(Error::Unsupported));
}

fn frame(body: &[u8], corr: u32) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 2052,
                routing_b: 19,
                correlation: corr,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .unwrap()
}

#[test]
fn a_declaration_without_licenses_is_acknowledged_for_the_configured_system() {
    let content = Content::from_json(&document()).unwrap();
    let absent = EnsurePlayerInventoryRequest {
        item_system_name: Some(b"Items/GameItemSystem"),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    for q in [
        frame(&absent, 7),
        declaration(b"Items/GameItemSystem", &[], 7),
    ] {
        let reply = content.reply(&q).unwrap();
        let d = nfs_fire2::decode(&reply, frame_limits()).unwrap().unwrap();
        assert_eq!(
            (
                d.frame.fields.routing_a,
                d.frame.fields.routing_b,
                d.frame.fields.category,
                d.frame.fields.correlation
            ),
            (2052, 19, 1, 7)
        );
        assert!(d.frame.body.is_empty() && d.frame.metadata.is_empty());
    }
    let other = EnsurePlayerInventoryRequest {
        item_system_name: Some(b"Items/Other"),
        ..Default::default()
    }
    .encode(body_limits())
    .unwrap();
    assert_eq!(content.reply(&frame(&other, 7)), Err(Error::Unsupported));
    let unconfigured = Content::new("Items/GameItemSystem".into(), vec![]).unwrap();
    assert!(unconfigured.reply(&frame(&absent, 8)).is_ok());
    assert_eq!(
        unconfigured.reply(&declaration(b"Items/GameItemSystem", LICENSES, 8)),
        Err(Error::Unsupported)
    );
}

#[test]
fn content_is_validated_and_bounded() {
    assert!(Content::from_json(&document()).is_ok());
    let mut v = document();
    v["version"] = json!(2);
    assert_eq!(Content::from_json(&v), Err(ContentError::Invalid));
    let mut v = document();
    v["licenses"][0]["license"] = json!("x".repeat(17));
    assert_eq!(Content::from_json(&v), Err(ContentError::Invalid));
    let mut v = document();
    v["licenses"][0]["extra"] = json!(1);
    assert_eq!(Content::from_json(&v), Err(ContentError::Invalid));
    let mut v = document();
    v["item_system"] = json!("");
    assert_eq!(Content::from_json(&v), Err(ContentError::Invalid));
    let many: Vec<License> = (0..33)
        .map(|i| License {
            license: format!("L{i}"),
            source: "Entitlement".into(),
        })
        .collect();
    assert_eq!(
        Content::new("Items/GameItemSystem".into(), many).err(),
        Some(ContentError::Invalid)
    );
    assert!(Content::new("Items/GameItemSystem".into(), vec![]).is_ok());
}
