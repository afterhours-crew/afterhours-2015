// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed response construction. Catalog selection, time and account progression
//! belong to services; absent fields and unknown values are preserved here.
use super::*;
use nfs_heat2::Value;

schema!(ChallengeInstance {
    car_id: u32 => [0x8e,0x88,0xe1],
    count_one: u32 => [0x8e,0x88,0xef],
    count_two: u32 => [0x8e,0x88,0xf4],
    day: &'a [u8] => [0x8e,0x89,0x39],
    event_id: u32 => [0x8e,0x89,0x76],
    id: u32 => [0x8e,0x8a,0x64],
    kind: u32 => [0x8e,0x8d,0x39],
    weekly: bool => [0x8e,0x8d,0xe5],
    type_override_id: u32 => [0xd2,0xfa,0x64],
});
schema!(AwardData {
    id: u64 => [0x87,0x7a,0x64],
    item_guid: &'a [u8] => [0x87,0x7a,0x74],
    obtained: bool => [0x87,0x7b,0xe2],
    kind: u32 => [0x87,0x7d,0x39],
    unlock_value: u32 => [0x87,0x7d,0xa1],
});
schema!(MonthlyRankInstance {
    id: u32 => [0xb6,0xfc,0xa1],
    rank_unlock: u32 => [0xca,0x1d,0x6e],
});
schema!(ChallengeProgress {
    complete: bool => [0x8e,0x88,0xed],
    current_count_one: u32 => [0x8e,0x88,0xef],
    current_count_two: u32 => [0x8e,0x88,0xf4],
    id: u32 => [0x8e,0x8a,0x64],
});

macro_rules! struct_list {
    ($name:ident,$element:ident) => {
        #[derive(Default)]
        pub struct $name<'a>(pub Vec<$element<'a>>);
        impl fmt::Debug for $name<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name))
                    .field(&self.0.len())
                    .finish()
            }
        }
        impl<'a> Wire<'a> for $name<'a> {
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
                        .map(|i| $element::read(i?))
                        .collect::<Result<_, _>>()?,
                ))
            }
            fn write(&self, tag: [u8; 3], w: &mut Encoder) -> Result<(), nfs_heat2::Error> {
                w.struct_list(tag, &self.0, |w, v| v.write_fields(w))
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
                self.0.iter().map(Wire::unknown_count).sum()
            }
        }
    };
}
struct_list!(ChallengeInstances, ChallengeInstance);
struct_list!(MonthlyRanks, MonthlyRankInstance);
struct_list!(ChallengeProgressList, ChallengeProgress);

#[derive(Default)]
pub struct ChallengeAwards<'a>(pub Vec<(u32, Vec<AwardData<'a>>)>);
impl fmt::Debug for ChallengeAwards<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ChallengeAwards")
            .field(&self.0.len())
            .finish()
    }
}
impl<'a> Wire<'a> for ChallengeAwards<'a> {
    const KIND: Kind = Kind::Map;
    fn read(item: Item<'a>) -> Result<Self, Error> {
        if !matches!(
            item.value(),
            Value::Map {
                key: Kind::Integer,
                value: Kind::List,
                ..
            }
        ) {
            return Err(Error::WrongType { tag: None });
        }
        let mut elements = item.elements().ok_or(Error::WrongType { tag: None })?;
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();
        while let Some(key) = elements.next() {
            let key = u32::read(key?)?;
            if !seen.insert(key) {
                return Err(Error::DuplicateMapKey);
            }
            let value = elements.next().ok_or(Error::WrongType { tag: None })??;
            if !matches!(
                value.value(),
                Value::List {
                    element: Kind::Struct,
                    ..
                }
            ) {
                return Err(Error::WrongType { tag: None });
            }
            let awards = value
                .elements()
                .ok_or(Error::WrongType { tag: None })?
                .map(|i| AwardData::read(i?))
                .collect::<Result<_, _>>()?;
            entries.push((key, awards));
        }
        Ok(Self(entries))
    }
    fn write(&self, tag: [u8; 3], w: &mut Encoder) -> Result<(), nfs_heat2::Error> {
        w.integer_struct_list_map(tag, &self.0, |w, v| v.write_fields(w))
    }
    fn validate(&self, budget: &mut crate::Budget) -> Result<(), Error> {
        budget.take(1)?;
        budget.collection(self.0.len(), 2)?;
        let mut seen = BTreeSet::new();
        for (key, values) in &self.0 {
            if !seen.insert(*key) {
                return Err(Error::DuplicateMapKey);
            }
            budget.collection(values.len(), 0)?;
            for value in values {
                value.validate(budget)?;
            }
        }
        Ok(())
    }
    fn unknown_count(&self) -> usize {
        self.0
            .iter()
            .flat_map(|(_, values)| values)
            .map(Wire::unknown_count)
            .sum()
    }
}

fn decode_response(bytes: &[u8], limits: Limits) -> Result<Document<'_>, nfs_heat2::Error> {
    nfs_heat2::decode_with_schema(bytes, limits, response_schema(), ROOT)
}
fn finish_response(writer: Encoder) -> Result<Vec<u8>, nfs_heat2::Error> {
    writer.finish_with_schema(response_schema(), ROOT)
}
schema!(GeneratedChallengesDataResponse {
    blaze_id: i64 => BLID,
    awards: ChallengeAwards<'a> => [0x8e,0x88,0x77],
    day_id: u32 => [0x8e,0x89,0x21],
    monthly_ranks: MonthlyRanks<'a> => [0x8e,0x8b,0x72],
    progress: ChallengeProgressList<'a> => [0x8e,0x8c,0x32],
    monthly_rank: u32 => [0xb6,0xfc,0xa1],
    monthly_start: i64 => [0xb6,0xfc,0xf4],
    challenges: ChallengeInstances<'a> => [0xcf,0x08,0xe8],
}, decode_response, finish_response);
