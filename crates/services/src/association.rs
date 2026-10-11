// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Association lists (`25/6`, `getLists`): the client asks for its friend
//! list (type 1, subscribed) and its follow list (type 32) at startup. A local
//! account owns both lists, empty, bound to the authenticated persona. Any
//! other list, selector or persona is unsupported; list membership changes
//! are outside this read-only subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not a complete, metadata-free category-0 `25/6` request, or an
    /// unobserved selector.
    Ineligible,
    /// The persona is not a valid authenticated persona.
    Identity,
    Encode,
}
use nfs_fire2::Frame;
use nfs_protocol::{
    association::{
        COMPONENT, GET_LISTS, GetListsRequest, ListIdentification, ListInfo, ListMembers,
        ListMembersVector, Lists, MUTUAL_ACTION, SUBSCRIBED,
    },
    users::ObjectId,
};

pub const MAX_FRAME: usize = 64 * 1024;
/// Observed capacities of the two startup lists.
pub const FRIEND_LIST_CAPACITY: u32 = 100;
pub const FOLLOW_LIST_CAPACITY: u32 = 20;

pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(MAX_FRAME, 0, MAX_FRAME - 16).expect("constant limits")
}
pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: MAX_FRAME - 16,
        max_depth: 8,
        max_values: 2048,
        max_collection: 128,
        max_byte_string: 1024,
    }
}
pub fn owns(component: u16, command: u16) -> bool {
    (component, command) == (COMPONENT, GET_LISTS)
}

/// Answer one list query for the authenticated persona with its empty list.
pub fn reply(persona_id: i64, wire: &[u8]) -> Result<Vec<u8>, Error> {
    if persona_id <= 0 {
        return Err(Error::Identity);
    }
    let decoded = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Error::Ineligible)?
        .ok_or(Error::Ineligible)?;
    let request = decoded.frame;
    let fields = request.fields;
    if decoded.consumed != wire.len()
        || !request.metadata.is_empty()
        || !owns(fields.routing_a, fields.routing_b)
        || fields.category != 0
        || fields.slot != 0
        || fields.reserved != [0, 0]
    {
        return Err(Error::Ineligible);
    }
    let message =
        GetListsRequest::decode(request.body, body_limits()).map_err(|_| Error::Ineligible)?;
    if message.unknown_field_count() != 0
        || message.max_result_count != Some(u32::MAX)
        || message.offset != Some(0)
        || message
            .encode(body_limits())
            .map_err(|_| Error::Ineligible)?
            != request.body
    {
        return Err(Error::Ineligible);
    }
    let lists = message.lists.ok_or(Error::Ineligible)?;
    let [info] = lists.0.as_slice() else {
        return Err(Error::Ineligible);
    };
    if info.blaze_object_id != Some(ObjectId(0, 0, 0))
        || info.max_size != Some(0)
        || info.pair_name != Some(b"")
        || info.pair_id != Some(0)
        || info.pair_max_size != Some(0)
    {
        return Err(Error::Ineligible);
    }
    let id = info.id.as_ref().ok_or(Error::Ineligible)?;
    let (list_type, name, status_flags, max_size): (u16, &[u8], u32, u32) =
        match (id.list_type, id.list_name, info.status_flags) {
            (Some(1), Some(b""), Some(SUBSCRIBED)) => (
                1,
                b"friendList",
                SUBSCRIBED | MUTUAL_ACTION,
                FRIEND_LIST_CAPACITY,
            ),
            (Some(32), Some(b"followList"), Some(0)) => {
                (32, b"followList", 0, FOLLOW_LIST_CAPACITY)
            }
            _ => return Err(Error::Ineligible),
        };
    let body = Lists {
        lists: Some(ListMembersVector(vec![ListMembers {
            info: Some(ListInfo {
                blaze_object_id: Some(ObjectId(COMPONENT, list_type, persona_id)),
                status_flags: Some(status_flags),
                id: Some(ListIdentification {
                    list_name: Some(name),
                    list_type: Some(list_type),
                    ..Default::default()
                }),
                max_size: Some(max_size),
                pair_name: Some(b""),
                pair_id: Some(0),
                pair_max_size: Some(0),
                ..Default::default()
            }),
            offset: Some(0),
            total_count: Some(0),
            ..Default::default()
        }])),
        ..Default::default()
    }
    .encode(body_limits())
    .map_err(|_| Error::Encode)?;
    nfs_fire2::encode(
        Frame {
            fields: nfs_fire2::Fields {
                category: 1,
                ..fields
            },
            metadata: &[],
            body: &body,
        },
        frame_limits(),
    )
    .map_err(|_| Error::Encode)
}
