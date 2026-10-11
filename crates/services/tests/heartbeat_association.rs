// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_fire2::{Fields, Frame};
use nfs_protocol::{association::*, users::ObjectId};
use nfs_services::{association, heartbeat};

fn wire(a: u16, b: u16, category: u8, correlation: u32, metadata: &[u8], body: &[u8]) -> Vec<u8> {
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                routing_a: a,
                routing_b: b,
                category,
                correlation,
                ..Default::default()
            },
            metadata,
            body,
        },
        nfs_fire2::Limits::new(70_000, 1024, 69_000).unwrap(),
    )
    .unwrap()
}

#[test]
fn heartbeats_echo_as_category_five_and_nothing_else_is_a_heartbeat() {
    let beat = wire(0, 0, 4, 3, &[], &[]);
    let reply = heartbeat::reply(&beat).unwrap().unwrap();
    let f = nfs_fire2::decode(&reply, heartbeat::frame_limits())
        .unwrap()
        .unwrap()
        .frame;
    assert_eq!(
        (f.fields.category, f.fields.correlation, f.fields.routing_a),
        (5, 3, 0)
    );
    assert!(f.body.is_empty() && f.metadata.is_empty());
    assert_eq!(heartbeat::reply(&beat).unwrap(), Some(reply));
    assert_eq!(
        heartbeat::reply(&wire(25, 6, 0, 1, &[], &[])).unwrap(),
        None
    );
    for bad in [
        wire(0, 1, 4, 1, &[], &[]),
        wire(0, 0, 4, 1, &[], &[1]),
        wire(0, 0, 4, 1, &[0x8a, 0xca, 0x64, 0, 1], &[]),
        [beat.clone(), beat.clone()].concat(),
    ] {
        assert_eq!(heartbeat::reply(&bad), Err(heartbeat::Error::Ineligible));
    }
    for split in 0..beat.len() {
        assert_eq!(
            heartbeat::reply(&beat[..split]),
            Err(heartbeat::Error::Ineligible)
        );
    }
}

fn list_query(list_type: u16, name: &'static [u8], flags: u32) -> Vec<u8> {
    let body = GetListsRequest {
        lists: Some(ListInfoVector(vec![ListInfo {
            blaze_object_id: Some(ObjectId(0, 0, 0)),
            status_flags: Some(flags),
            id: Some(ListIdentification {
                list_name: Some(name),
                list_type: Some(list_type),
                ..Default::default()
            }),
            max_size: Some(0),
            pair_name: Some(b""),
            pair_id: Some(0),
            pair_max_size: Some(0),
            ..Default::default()
        }])),
        max_result_count: Some(u32::MAX),
        offset: Some(0),
        ..Default::default()
    }
    .encode(association::body_limits())
    .unwrap();
    wire(25, 6, 0, 11, &[], &body)
}

#[test]
fn both_startup_lists_are_empty_and_bound_to_the_persona() {
    for (query, list_type, name, flags, capacity) in [
        (
            list_query(1, b"", SUBSCRIBED),
            1u16,
            b"friendList".as_slice(),
            SUBSCRIBED | MUTUAL_ACTION,
            100u32,
        ),
        (list_query(32, b"followList", 0), 32, b"followList", 0, 20),
    ] {
        let reply = association::reply(42, &query).unwrap();
        let f = nfs_fire2::decode(&reply, association::frame_limits())
            .unwrap()
            .unwrap()
            .frame;
        assert_eq!(
            (f.fields.routing_b, f.fields.category, f.fields.correlation),
            (6, 1, 11)
        );
        let lists = Lists::decode(f.body, association::body_limits()).unwrap();
        assert_eq!(lists.unknown_field_count(), 0);
        let members = lists.lists.unwrap().0;
        assert_eq!(members.len(), 1);
        let info = members[0].info.as_ref().unwrap();
        assert_eq!(info.blaze_object_id, Some(ObjectId(25, list_type, 42)));
        assert_eq!(info.status_flags, Some(flags));
        assert_eq!(info.max_size, Some(capacity));
        assert_eq!(info.id.as_ref().unwrap().list_name, Some(name));
        assert_eq!(members[0].total_count, Some(0));
        assert_eq!(association::reply(42, &query).unwrap(), reply);
        assert_ne!(association::reply(43, &query).unwrap(), reply);
    }
    assert_eq!(
        association::reply(0, &list_query(1, b"", SUBSCRIBED)),
        Err(association::Error::Identity)
    );
    assert_eq!(
        association::reply(42, &list_query(2, b"", SUBSCRIBED)),
        Err(association::Error::Ineligible)
    );
    assert_eq!(
        association::reply(42, &list_query(1, b"", 0)),
        Err(association::Error::Ineligible)
    );
    assert_eq!(
        association::reply(42, &wire(25, 7, 0, 1, &[], &[])),
        Err(association::Error::Ineligible)
    );
    let q = list_query(32, b"followList", 0);
    assert_eq!(
        association::reply(42, &[q.clone(), q.clone()].concat()),
        Err(association::Error::Ineligible)
    );
    for split in 0..q.len() {
        assert_eq!(
            association::reply(42, &q[..split]),
            Err(association::Error::Ineligible)
        );
    }
}
