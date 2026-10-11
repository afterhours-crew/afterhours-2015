// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Challenge read schema. Catalog rotation and progression are
//! service policy. This borrowed response preserves exact known/unknown fields;
//! it does not synthesize missing daily challenges or award state.
use crate::{Error, Wire, schema};
use nfs_heat2::{
    Document, Encoder, Field, Fields, Item, Kind, Limits, Member, Schema, Type, TypeId,
};
use std::{collections::BTreeSet, fmt};

pub const COMPONENT: u16 = 2054;
pub const GET_DAILY_CHALLENGES: u16 = 1;
const BLID: [u8; 3] = [0x8a, 0xca, 0x64];

mod generated;
pub use generated::*;

schema!(GetDailyChallengesRequest {
    blaze_id: i64 => BLID,
    // Native TimeValue, tagged integer. Units/epoch are deliberately unassigned.
    debug_start_day: i64 => [0x92, 0x5c, 0xe4],
    force_debug_start_day: bool => [0x9a, 0x4c, 0xe4],
});

const fn member(tag: [u8; 3], ty: usize) -> Member {
    Member {
        tag,
        ty: TypeId(ty),
    }
}
// Scalar IDs 0=i64, 1=u32, 2=u64, 3=bool, 4=string, 5=TimeValue.
// Width checks below supplement Heat2's wire-kind/collection-layout validation.
const TYPES: &[Type<'static>] = &[
    Type::Scalar(Kind::Integer),
    Type::Scalar(Kind::Integer),
    Type::Scalar(Kind::Integer),
    Type::Scalar(Kind::Integer),
    Type::Scalar(Kind::String),
    Type::TimeValue,
    // ChallengeInstance.
    Type::Struct(&[
        member([0x8e, 0x88, 0xe1], 1),
        member([0x8e, 0x88, 0xef], 1),
        member([0x8e, 0x88, 0xf4], 1),
        member([0x8e, 0x89, 0x39], 4),
        member([0x8e, 0x89, 0x76], 1),
        member([0x8e, 0x8a, 0x64], 1),
        member([0x8e, 0x8d, 0x39], 1),
        member([0x8e, 0x8d, 0xe5], 3),
        member([0xd2, 0xfa, 0x64], 1),
    ]),
    // AwardData, MonthlyRankInstance and ChallengeProgress.
    Type::Struct(&[
        member([0x87, 0x7a, 0x64], 2),
        member([0x87, 0x7a, 0x74], 4),
        member([0x87, 0x7b, 0xe2], 3),
        member([0x87, 0x7d, 0x39], 1),
        member([0x87, 0x7d, 0xa1], 1),
    ]),
    Type::Struct(&[member([0xb6, 0xfc, 0xa1], 1), member([0xca, 0x1d, 0x6e], 1)]),
    Type::Struct(&[
        member([0x8e, 0x88, 0xed], 3),
        member([0x8e, 0x88, 0xef], 1),
        member([0x8e, 0x88, 0xf4], 1),
        member([0x8e, 0x8a, 0x64], 1),
    ]),
    Type::List(TypeId(7)),
    // Map<u32,List<AwardData>>: the on-wire value header is 3, not 4.
    Type::Map {
        key: TypeId(1),
        value: TypeId(10),
    },
    Type::List(TypeId(8)),
    Type::List(TypeId(9)),
    Type::List(TypeId(6)),
    // ChallengesDataResponse.
    Type::Struct(&[
        member(BLID, 0),
        member([0x8e, 0x88, 0x77], 11),
        member([0x8e, 0x89, 0x21], 1),
        member([0x8e, 0x8b, 0x72], 12),
        member([0x8e, 0x8c, 0x32], 13),
        member([0xb6, 0xfc, 0xa1], 1),
        member([0xb6, 0xfc, 0xf4], 5),
        member([0xcf, 0x08, 0xe8], 14),
    ]),
];
const ROOT: TypeId = TypeId(15);
fn response_schema() -> Schema<'static> {
    Schema::new(TYPES).expect("constant Challenge descriptor graph")
}

fn check_fields(fields: Fields<'_>, ty: TypeId) -> Result<usize, Error> {
    let Type::Struct(members) = TYPES[ty.0] else {
        unreachable!("constant struct")
    };
    let mut seen = BTreeSet::new();
    let mut unknown = 0;
    for f in fields {
        let f = f?;
        if !seen.insert(f.tag()) {
            return Err(Error::DuplicateTag(f.tag()));
        }
        if let Some(m) = members.iter().find(|m| m.tag == f.tag()) {
            unknown += check_item(f.item(), m.ty).map_err(|e| e.with_tag(f.tag()))?;
        } else {
            unknown += 1;
        }
    }
    Ok(unknown)
}
fn check_item(item: Item<'_>, ty: TypeId) -> Result<usize, Error> {
    match ty.0 {
        0 | 5 => {
            i64::read(item)?;
        }
        1 => {
            u32::read(item)?;
        }
        2 => {
            u64::read(item)?;
        }
        3 => {
            bool::read(item)?;
        }
        4 => {
            <&[u8]>::read(item)?;
        }
        _ => match TYPES[ty.0] {
            Type::Struct(_) => {
                return check_fields(item.fields().ok_or(Error::WrongType { tag: None })?, ty);
            }
            Type::List(element) => {
                let mut unknown = 0;
                for e in item.elements().ok_or(Error::WrongType { tag: None })? {
                    unknown += check_item(e?, element)?;
                }
                return Ok(unknown);
            }
            Type::Map { key, value } => {
                let mut entries = item.elements().ok_or(Error::WrongType { tag: None })?;
                let mut keys = BTreeSet::new();
                let mut unknown = 0;
                while let Some(k) = entries.next() {
                    let k = k?;
                    check_item(k, key)?;
                    if !keys.insert(u32::read(k)?) {
                        return Err(Error::DuplicateMapKey);
                    }
                    unknown += check_item(
                        entries.next().ok_or(Error::WrongType { tag: None })??,
                        value,
                    )?;
                }
                return Ok(unknown);
            }
            _ => unreachable!("constant Challenge graph"),
        },
    }
    Ok(0)
}

/// A validated immutable snapshot, including account progress and catalog data.
/// Absence remains absence. No requirement for all fields is imposed by the codec.
pub struct ChallengesDataResponse<'a> {
    document: Document<'a>,
    unknown: usize,
}
impl fmt::Debug for ChallengesDataResponse<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChallengesDataResponse")
            .field("unknown_fields", &self.unknown)
            .finish_non_exhaustive()
    }
}
impl<'a> ChallengesDataResponse<'a> {
    pub fn decode(bytes: &'a [u8], limits: Limits) -> Result<Self, Error> {
        let document = nfs_heat2::decode_with_schema(bytes, limits, response_schema(), ROOT)?;
        let unknown = check_fields(document.fields(), ROOT)?;
        Ok(Self { document, unknown })
    }
    pub fn unknown_field_count(&self) -> usize {
        self.unknown
    }
    pub fn fields(&self) -> Fields<'a> {
        self.document.fields()
    }
    pub fn has_all_fields(&self) -> bool {
        self.fields().count() == 8 && self.unknown == 0
    }
    pub fn blaze_id(&self) -> Option<i64> {
        self.fields().find_map(|f| {
            let f = f.ok()?;
            (f.tag() == BLID)
                .then(|| i64::read(f.item()).ok())
                .flatten()
        })
    }
    /// Exact retention; unlike generated typed models this does not canonicalize.
    pub fn encode(&self, limits: Limits) -> Result<Vec<u8>, Error> {
        Self::decode(self.document.as_bytes(), limits)?;
        Ok(self.document.as_bytes().to_vec())
    }
    /// Replace the account identity, retaining the entire catalog/progress snapshot.
    /// The caller is responsible for assigning this snapshot to that account.
    pub fn encode_with_blaze_id(&self, persona: i64, limits: Limits) -> Result<Vec<u8>, Error> {
        let mut w = Encoder::new(limits);
        w.integer(BLID, persona)?;
        for f in self.fields() {
            let f = f?;
            if f.tag() != BLID {
                w.raw_field(f)?;
            }
        }
        let bytes = w.finish_with_schema(response_schema(), ROOT)?;
        ChallengesDataResponse::decode(&bytes, limits)?;
        Ok(bytes)
    }
}
