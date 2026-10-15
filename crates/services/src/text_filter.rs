// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local text policy for garage plate/name checks. Valid UTF-8 without control
//! characters passes unchanged. This is not an implementation of EA moderation.
//! Requests retain no text, mutate no account state and make no external calls.
use nfs_fire2::{Fields, Frame, HEADER_LEN};
use nfs_protocol::util::{
    self, FilterResult, FilterUserTextResponse, FilteredUserText, FilteredUserTextList,
};

/// The native filtered-text field allows 513 bytes including its terminator.
pub const MAX_TEXT_BYTES: usize = 512;
/// Local ingress bounds, including room to return StringTooLong per entry.
pub const MAX_INPUT_BYTES: usize = 1024;
pub const MAX_ENTRIES: usize = 32;
pub const MAX_BODY_BYTES: usize = 40 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Ineligible,
    Encode,
}

pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: MAX_BODY_BYTES,
        max_depth: 3,
        max_values: 1 + 5 * MAX_ENTRIES,
        max_collection: MAX_ENTRIES,
        max_byte_string: MAX_INPUT_BYTES + 1,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(MAX_BODY_BYTES + HEADER_LEN, 0, MAX_BODY_BYTES).expect("constant limits")
}
pub fn owns(component: u16, command: u16) -> bool {
    (component, command) == (util::COMPONENT, util::FILTER_FOR_PROFANITY)
}

fn classify(text: &[u8]) -> FilterResult {
    if text.len() > MAX_TEXT_BYTES {
        return FilterResult::StringTooLong;
    }
    if !std::str::from_utf8(text).is_ok_and(|s| !s.chars().any(char::is_control)) {
        return FilterResult::Other;
    }
    FilterResult::Passed
}

pub fn reply(wire: &[u8]) -> Result<Vec<u8>, Error> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Error::Ineligible)?
        .ok_or(Error::Ineligible)?;
    let f = d.frame;
    if d.consumed != wire.len()
        || !owns(f.fields.routing_a, f.fields.routing_b)
        || f.fields.category != 0
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
        || !f.metadata.is_empty()
    {
        return Err(Error::Ineligible);
    }
    let request =
        FilterUserTextResponse::decode(f.body, body_limits()).map_err(|_| Error::Ineligible)?;
    let entries = request.filtered_text_list.ok_or(Error::Ineligible)?;
    let mut filtered = Vec::with_capacity(entries.0.len());
    for entry in entries.0 {
        if !matches!(entry.result, Some(0..=4)) {
            return Err(Error::Ineligible);
        }
        let text = entry.filtered_text.ok_or(Error::Ineligible)?;
        // Assess the text regardless of any status supplied by the caller.
        let result = classify(text);
        filtered.push(FilteredUserText {
            result: Some(result as i32),
            filtered_text: Some(if result == FilterResult::Passed {
                text
            } else {
                b""
            }),
            ..Default::default()
        });
    }
    let body = FilterUserTextResponse {
        filtered_text_list: Some(FilteredUserTextList(filtered)),
        ..Default::default()
    }
    .encode(body_limits())
    .map_err(|_| Error::Encode)?;
    nfs_fire2::encode(
        Frame {
            fields: Fields {
                category: 1,
                ..f.fields
            },
            metadata: &[],
            body: &body,
        },
        frame_limits(),
    )
    .map_err(|_| Error::Encode)
}

#[cfg(test)]
mod tests;
