// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::authentication::entitlements::*;
use nfs_services::{
    ContentError,
    entitlements::{self, Error, State, body_limits, frame_limits},
};
use nfs_storage::AccountId;
use serde_json::{Value, json};

fn account(n: u8) -> AccountId {
    AccountId::from_owned_config([n; 16]).unwrap()
}
fn document(owner: AccountId) -> Value {
    let grant = |id, group| {
        json!({"device_uri":"","grant_date":"synthetic-grant","group_name":group,"id":id,
        "is_consumable":false,"persona_id":0,"project_id":"project","product_catalog":i32::MIN,"product_id":"product",
        "status":i32::MAX,"status_reason_code":-1,"entitlement_tag":"synthetic-tag","termination_date":"",
        "entitlement_type":5,"use_count":u32::MAX,"version":u32::MAX})
    };
    json!({"format":"nfs-entitlement-state","version":1,"build_sha256":nfs_services::SUPPORTED_BUILD_SHA256,
        "account":owner.hex(),"scopes":[["alpha","beta"],["beta"]],"grants":[grant(u64::MAX,"beta"),grant(123,"alpha")]})
}
fn model(persona: i64, groups: Vec<&[u8]>) -> ListUserEntitlements2Request<'_> {
    ListUserEntitlements2Request {
        user_id: Some(persona),
        end_grant_date: Some(b""),
        page_no: Some(0),
        page_size: Some(0),
        entitlement_tag: Some(b""),
        end_termination_date: Some(b""),
        group_name_list: Some(GroupNameList(groups)),
        has_authorized_persona: Some(false),
        project_id: Some(b""),
        product_id: Some(b""),
        recursive_search: Some(false),
        start_grant_date: Some(b""),
        status: Some(0),
        start_termination_date: Some(b""),
        entitlement_type: Some(0),
        ..Default::default()
    }
}
fn wire(body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: 1,
                routing_b: 29,
                correlation: 0x00ff_ffff,
                ..Default::default()
            },
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .unwrap()
}
fn query(persona: i64, groups: Vec<&[u8]>) -> Vec<u8> {
    wire(&model(persona, groups).encode(body_limits()).unwrap())
}
fn frame(wire: &[u8]) -> Frame<'_> {
    let d = nfs_fire2::decode(wire, frame_limits()).unwrap().unwrap();
    assert_eq!(d.consumed, wire.len());
    d.frame
}

#[test]
fn account_grants_filter_by_supported_scope_keep_order_widths_and_retry_exactly() {
    let owner = account(1);
    let state = State::from_json(&document(owner), owner).unwrap();
    let q = query(i64::MAX, vec![b"alpha", b"beta"]);
    let reply = state.reply(&q, owner, i64::MAX).unwrap();
    let f = frame(&reply);
    assert_eq!(f.fields.category, 1);
    assert_eq!(f.fields.correlation, 0x00ff_ffff);
    assert!(f.metadata.is_empty());
    let rows = Entitlements::decode(f.body, body_limits())
        .unwrap()
        .entitlements
        .unwrap()
        .0;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, Some(u64::MAX));
    assert_eq!(rows[1].id, Some(123));
    assert_eq!(rows[0].group_name, Some(b"beta".as_slice()));
    assert_eq!(rows[0].product_catalog, Some(i32::MIN));
    assert_eq!(rows[0].status, Some(i32::MAX));
    assert_eq!(rows[0].status_reason_code, Some(-1));
    assert_eq!(rows[0].version, Some(u32::MAX));
    assert_eq!(rows[0].use_count, Some(u32::MAX));
    assert_eq!(rows[0].persona_id, Some(0));
    assert_eq!(state.reply(&q, owner, i64::MAX).unwrap(), reply);
    let subset = state.reply(&query(42, vec![b"beta"]), owner, 42).unwrap();
    assert_eq!(
        Entitlements::decode(frame(&subset).body, body_limits())
            .unwrap()
            .entitlements
            .unwrap()
            .0
            .len(),
        1
    );
}

#[test]
fn concurrent_accounts_and_current_personas_cannot_select_each_others_grants() {
    let a = account(1);
    let b = account(2);
    let first = State::from_json(&document(a), a).unwrap();
    let mut other = document(b);
    other["grants"][0]["id"] = json!(77);
    let second = State::from_json(&other, b).unwrap();
    let q = query(42, vec![b"beta"]);
    assert_eq!(first.reply(&q, b, 42), Err(Error::Identity));
    assert_eq!(first.reply(&q, a, 43), Err(Error::Ineligible));
    assert_eq!(first.reply(&q, a, 0), Err(Error::Identity));
    assert!(matches!(
        State::from_json(&document(a), b),
        Err(ContentError::Invalid)
    ));
    std::thread::scope(|threads| {
        let one = threads.spawn(|| first.reply(&q, a, 42).unwrap());
        let two = threads.spawn(|| second.reply(&q, b, 42).unwrap());
        assert_ne!(one.join().unwrap(), two.join().unwrap());
    });
}

#[test]
fn partial_concatenated_unknown_and_malformed_frames_never_answer() {
    let owner = account(1);
    let state = State::from_json(&document(owner), owner).unwrap();
    let q = query(42, vec![b"beta"]);
    for end in 0..q.len() {
        assert_eq!(state.reply(&q[..end], owner, 42), Err(Error::Ineligible));
    }
    assert_eq!(
        state.reply(&[q.clone(), q.clone()].concat(), owner, 42),
        Err(Error::Ineligible)
    );
    let original = frame(&q);
    for change in 0..5 {
        let mut fields = original.fields;
        let metadata: &[u8] = if change == 4 { &[0] } else { &[] };
        match change {
            0 => fields.category = 1,
            1 => fields.routing_b = 30,
            2 => fields.slot = 1,
            3 => fields.reserved = [1, 0],
            _ => {}
        }
        let bad = nfs_fire2::encode(
            Frame {
                fields,
                metadata,
                body: original.body,
            },
            nfs_fire2::Limits::default(),
        )
        .unwrap();
        assert_eq!(state.reply(&bad, owner, 42), Err(Error::Ineligible));
    }
    let unknown = [original.body, &[0xff, 0xff, 0xff, 0, 1]].concat();
    assert_eq!(
        state.reply(&wire(&unknown), owner, 42),
        Err(Error::Ineligible)
    );
    let mut oversized = q.clone();
    oversized[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(state.reply(&oversized, owner, 42), Err(Error::Ineligible));
    assert_eq!(
        state.reply(&q, owner, 42).unwrap(),
        state.reply(&q, owner, 42).unwrap()
    );
}

#[test]
fn unsupported_search_filters_and_scope_variants_are_not_empty_successes() {
    let owner = account(1);
    let state = State::from_json(&document(owner), owner).unwrap();
    for groups in [
        vec![],
        vec![b"unknown".as_slice()],
        vec![b"beta", b"alpha"],
        vec![b"beta", b"beta"],
    ] {
        assert_eq!(
            state.reply(&query(42, groups), owner, 42),
            Err(Error::Ineligible)
        );
    }
    for change in 0..6 {
        let mut q = model(42, vec![b"beta"]);
        match change {
            0 => q.page_size = Some(1),
            1 => q.status = Some(1),
            2 => q.recursive_search = Some(true),
            3 => q.has_authorized_persona = Some(true),
            4 => q.product_id = Some(b"product"),
            _ => q.user_id = None,
        }
        assert_eq!(
            state.reply(&wire(&q.encode(body_limits()).unwrap()), owner, 42),
            Err(Error::Ineligible)
        );
    }
}

#[test]
fn malformed_state_duplicates_unowned_rows_and_bounds_are_rejected() {
    let owner = account(1);
    for change in 0..12 {
        let mut v = document(owner);
        match change {
            0 => v["version"] = json!(2),
            1 => v["extra"] = json!(0),
            2 => {
                v["grants"][0].as_object_mut().unwrap().remove("status");
            }
            3 => v["grants"][1]["id"] = v["grants"][0]["id"].clone(),
            4 => v["grants"][0]["persona_id"] = json!(7),
            5 => v["grants"][0]["group_name"] = json!("foreign"),
            6 => v["scopes"] = json!([["beta", "beta"]]),
            7 => v["scopes"] = json!([["beta"], ["beta"]]),
            8 => v["grants"][0]["version"] = json!(u64::MAX),
            9 => v["grants"][0]["device_uri"] = json!("x".repeat(1024)),
            10 => v["grants"][0]["id"] = json!(0),
            _ => v["grants"] = json!(vec![v["grants"][0].clone(); entitlements::MAX_GRANTS + 1]),
        }
        assert!(State::from_json(&v, owner).is_err(), "change {change}");
    }
    let mut large = document(owner);
    let mut grant = large["grants"][0].clone();
    grant["device_uri"] = json!("x".repeat(1023));
    large["grants"] = Value::Array(
        (1..=128)
            .map(|id| {
                let mut row = grant.clone();
                row["id"] = json!(id);
                row
            })
            .collect(),
    );
    assert!(matches!(
        State::from_json(&large, owner),
        Err(ContentError::TooLarge)
    ));
}

#[test]
fn explicit_changed_state_and_empty_grants_are_reflected_without_cached_replies() {
    let owner = account(1);
    let original = document(owner);
    let before = State::from_json(&original, owner).unwrap();
    let q = query(42, vec![b"beta"]);
    let reply = before.reply(&q, owner, 42).unwrap();
    let mut changed = original;
    changed["grants"][0]["use_count"] = json!(2);
    let after = State::from_json(&changed, owner).unwrap();
    let new_reply = after.reply(&q, owner, 42).unwrap();
    assert_ne!(reply, new_reply);
    assert_eq!(
        Entitlements::decode(frame(&new_reply).body, body_limits())
            .unwrap()
            .entitlements
            .unwrap()
            .0[0]
            .use_count,
        Some(2)
    );
    changed["grants"] = json!([]);
    let empty = State::from_json(&changed, owner)
        .unwrap()
        .reply(&q, owner, 42)
        .unwrap();
    assert!(
        Entitlements::decode(frame(&empty).body, body_limits())
            .unwrap()
            .entitlements
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn reopening_account_document_preserves_ids_and_does_not_rewrite_state() {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "nfs-entitlement-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let owner = account(1);
    let raw = serde_json::to_vec(&document(owner)).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(&mut file, &raw).unwrap();
    drop(file);
    let q = query(42, vec![b"beta"]);
    let first = State::load(&path, owner)
        .unwrap()
        .reply(&q, owner, 42)
        .unwrap();
    assert_eq!(
        State::load(&path, owner)
            .unwrap()
            .reply(&q, owner, 42)
            .unwrap(),
        first
    );
    assert!(State::load(&path, account(2)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), raw);
    std::fs::write(&path, vec![b' '; entitlements::MAX_STATE_BYTES + 1]).unwrap();
    assert!(matches!(
        State::load(&path, owner),
        Err(ContentError::TooLarge)
    ));
    std::fs::remove_file(path).unwrap();
}
