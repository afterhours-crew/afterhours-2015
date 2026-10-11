// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Explicit local unavailable-news policy for the supported menu query.
//! Other locales and debug dates remain unsupported. No feed data is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    IneligibleRequest,
    Reply,
}
use nfs_fire2::{Fields, Frame, Limits};
use nfs_protocol::{
    autolog::{COMPONENT, GET_NEWS, NewsRequest},
    metadata::Fire2Metadata,
};

/// The observed Blaze error code of the unavailable news answer.
pub const NEWS_UNAVAILABLE: i32 = 0x001b_0802;
/// Observed request body: two short strings.
pub const MAX_REQUEST_BODY: usize = 256;

fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: MAX_REQUEST_BODY,
        max_depth: 4,
        max_values: 64,
        max_collection: 16,
        max_byte_string: 128,
    }
}
fn frame_limits() -> Limits {
    Limits::new(MAX_REQUEST_BODY + 32, 16, MAX_REQUEST_BODY).expect("constant limits")
}

/// `Ok(None)` is "not mine": a well-formed `2050/74` request outside the
/// observed shape gets no reply, so nothing invents a news feed.
pub fn reply(wire: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Error::IneligibleRequest)?
        .ok_or(Error::IneligibleRequest)?;
    let f = d.frame;
    if d.consumed != wire.len()
        || (f.fields.routing_a, f.fields.routing_b) != (COMPONENT, GET_NEWS)
        || f.fields.category != 0
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || !f.metadata.is_empty()
    {
        return Err(Error::IneligibleRequest);
    }
    let q = NewsRequest::decode(f.body, body_limits()).map_err(|_| Error::IneligibleRequest)?;
    if q.debug_date != Some(b"".as_slice())
        || q.locale != Some(b"en_us".as_slice())
        || q.unknown_field_count() != 0
        || q.encode(body_limits())
            .map_err(|_| Error::IneligibleRequest)?
            != f.body
    {
        return Ok(None);
    }
    let metadata = Fire2Metadata {
        context: Some(0),
        error_code: Some(NEWS_UNAVAILABLE),
        ..Default::default()
    }
    .encode(body_limits())
    .map_err(|_| Error::Reply)?;
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                category: 3,
                ..f.fields
            },
            metadata: &metadata,
            body: &[],
        },
        frame_limits(),
    )
    .map(Some)
    .map_err(|_| Error::Reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(locale: &[u8], debug_date: &[u8], correlation: u32) -> Vec<u8> {
        let body = NewsRequest {
            debug_date: Some(debug_date),
            locale: Some(locale),
            ..Default::default()
        }
        .encode(body_limits())
        .unwrap();
        nfs_fire2::encode(
            Frame {
                fields: Fields {
                    routing_a: COMPONENT,
                    routing_b: GET_NEWS,
                    correlation,
                    ..Default::default()
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .unwrap()
    }

    #[test]
    fn observed_query_gets_the_unavailable_error_with_its_correlation() {
        let answer = reply(&request(b"en_us", b"", 40)).unwrap().unwrap();
        let d = nfs_fire2::decode(&answer, frame_limits()).unwrap().unwrap();
        assert_eq!(d.consumed, answer.len());
        assert_eq!(
            (
                d.frame.fields.routing_a,
                d.frame.fields.routing_b,
                d.frame.fields.category,
                d.frame.fields.correlation
            ),
            (COMPONENT, GET_NEWS, 3, 40)
        );
        assert!(d.frame.body.is_empty());
        let m = Fire2Metadata::decode(d.frame.metadata, body_limits()).unwrap();
        assert_eq!((m.context, m.error_code), (Some(0), Some(NEWS_UNAVAILABLE)));
        assert_eq!(m.session_key, None);
    }

    #[test]
    fn other_locales_or_debug_dates_and_foreign_frames_are_not_answered() {
        assert_eq!(reply(&request(b"de_de", b"", 1)).unwrap(), None);
        assert_eq!(reply(&request(b"en_us", b"2026-10-09", 1)).unwrap(), None);
        let foreign = nfs_fire2::encode(
            Frame {
                fields: Fields {
                    routing_a: COMPONENT,
                    routing_b: 73,
                    ..Default::default()
                },
                metadata: &[],
                body: &[],
            },
            frame_limits(),
        )
        .unwrap();
        assert_eq!(reply(&foreign), Err(Error::IneligibleRequest));
        assert_eq!(
            reply(&request(b"en_us", b"", 1)[..8]),
            Err(Error::IneligibleRequest)
        );
    }

    #[test]
    fn bounded_frames_reject_every_partial_and_invalid_header() {
        let query = request(b"en_us", b"", 0x00ff_ffff);
        let answer = reply(&query).unwrap().unwrap();
        assert_eq!(reply(&query).unwrap(), Some(answer));
        for end in 0..query.len() {
            assert_eq!(reply(&query[..end]), Err(Error::IneligibleRequest));
        }
        assert_eq!(
            reply(&[query.clone(), query.clone()].concat()),
            Err(Error::IneligibleRequest)
        );
        let original = nfs_fire2::decode(&query, frame_limits())
            .unwrap()
            .unwrap()
            .frame;
        for change in 0..4 {
            let mut fields = original.fields;
            let metadata: &[u8] = if change == 3 { &[0] } else { &[] };
            match change {
                0 => fields.slot = 1,
                1 => fields.reserved = [1, 0],
                2 => fields.category = 1,
                _ => {}
            }
            let invalid = nfs_fire2::encode(
                Frame {
                    fields,
                    metadata,
                    body: original.body,
                },
                frame_limits(),
            )
            .unwrap();
            assert_eq!(reply(&invalid), Err(Error::IneligibleRequest));
        }
        let oversized = NewsRequest {
            debug_date: Some(b""),
            locale: Some(&[b'x'; 129]),
            ..Default::default()
        }
        .encode(nfs_heat2::Limits::default())
        .unwrap();
        let invalid = nfs_fire2::encode(
            Frame {
                fields: original.fields,
                metadata: &[],
                body: &oversized,
            },
            nfs_fire2::Limits::default(),
        )
        .unwrap();
        assert_eq!(reply(&invalid), Err(Error::IneligibleRequest));
        let unknown = [original.body, &[0xff, 0xff, 0xff, 0, 1]].concat();
        let invalid = nfs_fire2::encode(
            Frame {
                fields: original.fields,
                metadata: &[],
                body: &unknown,
            },
            frame_limits(),
        )
        .unwrap();
        assert_eq!(reply(&invalid), Ok(None));
    }
}
