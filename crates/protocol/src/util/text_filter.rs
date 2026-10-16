// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Utility text-filter payload. The client sends and receives TLST entries with
//! DIRT/UTXT fields. Content policy and resource bounds belong to the service.
use crate::{Error, Wire, schema};
use nfs_heat2::{Encoder, Field, Fields, Item, Kind, Limits, Value};
use std::{collections::BTreeSet, fmt};

pub const FILTER_FOR_PROFANITY: u16 = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum FilterResult {
    Passed = 0,
    Offensive = 1,
    Unprocessed = 2,
    StringTooLong = 3,
    Other = 4,
}

schema!(FilteredUserText {
    // Keep raw enum values so unknown variants survive codec round trips.
    result: i32 => [0x92, 0x9c, 0xb4],
    filtered_text: &'a [u8] => [0xd7, 0x4e, 0x34],
});

#[derive(Default)]
pub struct FilteredUserTextList<'a>(pub Vec<FilteredUserText<'a>>);
impl fmt::Debug for FilteredUserTextList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FilteredUserTextListLength")
            .field(&self.0.len())
            .finish()
    }
}
impl<'a> Wire<'a> for FilteredUserTextList<'a> {
    const KIND: Kind = Kind::List;
    fn read(item: Item<'a>) -> Result<Self, Error> {
        if !matches!(
            item.value(),
            Value::List {
                element: Kind::Struct,
                ..
            }
        ) {
            return Err(Error::WrongType { tag: None });
        }
        Ok(Self(
            item.elements()
                .ok_or(Error::WrongType { tag: None })?
                .map(|i| FilteredUserText::read(i?))
                .collect::<Result<_, _>>()?,
        ))
    }
    fn write(&self, tag: [u8; 3], w: &mut Encoder) -> Result<(), nfs_heat2::Error> {
        w.struct_list(tag, &self.0, |w, value| value.write_fields(w))
    }
    fn validate(&self, budget: &mut crate::Budget) -> Result<(), Error> {
        budget.take(1)?;
        budget.collection(self.0.len(), 0)?;
        for value in &self.0 {
            value.validate(budget)?;
        }
        Ok(())
    }
    fn unknown_count(&self) -> usize {
        self.0
            .iter()
            .map(FilteredUserText::unknown_field_count)
            .sum()
    }
}

schema!(FilterUserTextResponse {
    filtered_text_list: FilteredUserTextList<'a> => [0xd2, 0xcc, 0xf4],
});
