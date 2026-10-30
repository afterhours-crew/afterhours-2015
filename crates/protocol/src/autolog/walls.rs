// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! getInGameSpeedWalls (`2050/20`): the client's SpeedWall query for events it
//! presents (`InGameSpeedWallsRequest`: Blaze ID, speed-wall IDs and two integer
//! options) and `InGameSpeedWallResponse`: for each SpeedWall stat type a list
//! of walls, each with its rows and ID. The response map's value header is 3
//! for its lists, so decoding needs the layout below. No ranking policy.
use super::*;
use nfs_heat2::{Document, Member, Schema, Type, TypeId, Value};

pub const GET_IN_GAME_SPEED_WALLS: u16 = 20;
/// Most walls one request or stat type may list (the client asks for the walls
/// of the events it presents; E997 requests named one).
pub const MAX_SPEED_WALLS: usize = 16;

const BLID: [u8; 3] = [0x8a, 0xca, 0x64];
const SWIS: [u8; 3] = [0xcf, 0x7a, 0x73];
const USGE: [u8; 3] = [0xd7, 0x39, 0xe5];
const USPG: [u8; 3] = [0xd7, 0x3c, 0x27];
const SPWA: [u8; 3] = [0xcf, 0x0d, 0xe1];
const ROWS: [u8; 3] = [0xca, 0xfd, 0xf3];
const SWID: [u8; 3] = [0xcf, 0x7a, 0x64];

#[derive(Default)]
pub struct SpeedWallIds(pub Vec<u32>);
impl fmt::Debug for SpeedWallIds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SpeedWallIdsLength")
            .field(&self.0.len())
            .finish()
    }
}
impl<'a> Wire<'a> for SpeedWallIds {
    const KIND: Kind = Kind::List;
    fn read(item: Item<'a>) -> Result<Self, Error> {
        if !matches!(
            item.value(),
            Value::List {
                element: Kind::Integer,
                ..
            }
        ) {
            return Err(Error::WrongType { tag: None });
        }
        Ok(Self(
            item.elements()
                .ok_or(Error::WrongType { tag: None })?
                .map(|i| u32::read(i?))
                .collect::<Result<_, _>>()?,
        ))
    }
    fn write(&self, tag: [u8; 3], w: &mut Encoder) -> Result<(), nfs_heat2::Error> {
        w.integer_list(tag, self.0.iter().map(|v| i64::from(*v)))
    }
    fn validate(&self, budget: &mut crate::Budget) -> Result<(), Error> {
        budget.take(1)?;
        budget.collection(self.0.len(), 1)
    }
}
schema!(InGameSpeedWallsRequest {
    blaze_id: i64 => BLID,
    speed_wall_ids: SpeedWallIds => SWIS,
    usge: i32 => USGE,
    uspg: i32 => USPG,
});

#[derive(Default)]
pub struct SpeedWallRows<'a>(pub Vec<InGameSpeedWallResponseRow<'a>>);
impl fmt::Debug for SpeedWallRows<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SpeedWallRowsLength")
            .field(&self.0.len())
            .finish()
    }
}
impl<'a> Wire<'a> for SpeedWallRows<'a> {
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
                .map(|i| InGameSpeedWallResponseRow::read(i?))
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
schema!(InGameSpeedWallResponseSpeedWall {
    rows: SpeedWallRows<'a> => ROWS,
    speed_wall_id: u32 => SWID,
});

/// Walls by SpeedWall stat type (the client's ranked and secondary ranked stats).
#[derive(Default)]
pub struct SpeedWallsByStat<'a>(pub Vec<(u32, Vec<InGameSpeedWallResponseSpeedWall<'a>>)>);
impl fmt::Debug for SpeedWallsByStat<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SpeedWallsByStatLength")
            .field(&self.0.len())
            .finish()
    }
}
impl<'a> Wire<'a> for SpeedWallsByStat<'a> {
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
            let walls = value
                .elements()
                .ok_or(Error::WrongType { tag: None })?
                .map(|i| InGameSpeedWallResponseSpeedWall::read(i?))
                .collect::<Result<_, _>>()?;
            entries.push((key, walls));
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

const fn member(tag: [u8; 3], ty: usize) -> Member {
    Member {
        tag,
        ty: TypeId(ty),
    }
}
/// The response layout: scalars, BlazeUser, the three row stat maps, the row,
/// the row list, the wall, the wall list, the map by stat type and the root.
const TYPES: &[Type<'static>] = &[
    Type::Scalar(Kind::Integer),
    Type::Scalar(Kind::String),
    Type::Scalar(Kind::FloatBits),
    Type::Struct(&[
        member([0x8a, 0xca, 0x73], 0),
        member([0xc2, 0x5b, 0xa1], 1),
        member([0xd7, 0x2d, 0x39], 0),
    ]),
    Type::Map {
        key: TypeId(1),
        value: TypeId(2),
    },
    Type::Map {
        key: TypeId(1),
        value: TypeId(0),
    },
    Type::Map {
        key: TypeId(1),
        value: TypeId(1),
    },
    Type::Struct(&[
        member([0x8a, 0xcd, 0x73], 3),
        member([0xcf, 0x48, 0x66], 4),
        member([0xcf, 0x48, 0x69], 5),
        member([0xcf, 0x48, 0x73], 6),
    ]),
    Type::List(TypeId(7)),
    Type::Struct(&[member(ROWS, 8), member(SWID, 0)]),
    Type::List(TypeId(9)),
    Type::Map {
        key: TypeId(0),
        value: TypeId(10),
    },
    Type::Struct(&[member(SPWA, 11)]),
];
const ROOT: TypeId = TypeId(12);
fn response_schema() -> Schema<'static> {
    Schema::new(TYPES).expect("constant SpeedWall descriptor graph")
}
fn decode_response(bytes: &[u8], limits: Limits) -> Result<Document<'_>, nfs_heat2::Error> {
    nfs_heat2::decode_with_schema(bytes, limits, response_schema(), ROOT)
}
fn finish_response(writer: Encoder) -> Result<Vec<u8>, nfs_heat2::Error> {
    writer.finish_with_schema(response_schema(), ROOT)
}
schema!(InGameSpeedWallResponse {
    speed_walls: SpeedWallsByStat<'a> => SPWA,
}, decode_response, finish_response);

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            max_bytes: 4096,
            max_depth: 6,
            max_values: 256,
            max_collection: 64,
            max_byte_string: 256,
        }
    }
    fn hex(text: &str) -> Vec<u8> {
        let text: String = text.split_whitespace().collect();
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }
    fn row() -> InGameSpeedWallResponseRow<'static> {
        InGameSpeedWallResponseRow {
            blaze_user: Some(BlazeUser {
                blaze_id: Some(7),
                persona_name: Some(b"A"),
                relation_type: Some(1),
                ..Default::default()
            }),
            stats_flt: Some(SettingsFloatBits(vec![(b"T", 1.5f32.to_bits())])),
            stats_int: Some(RowIntegers(vec![(b"N", 2)])),
            stats_str: Some(RowStrings(vec![(b"V", b"C")])),
            ..Default::default()
        }
    }
    fn wall(
        rows: Option<Vec<InGameSpeedWallResponseRow<'static>>>,
    ) -> InGameSpeedWallResponseSpeedWall<'static> {
        InGameSpeedWallResponseSpeedWall {
            rows: rows.map(SpeedWallRows),
            speed_wall_id: Some(5),
            ..Default::default()
        }
    }
    /// Independently assembled: SPWA map (integer keys, header 3 values) of two
    /// stat types; stat 0 lists a wall without rows, stat 1 a wall with one row.
    const RESPONSE: &str = "cf0de1 05 00 03 02
        00 03 01 cf7a64 00 05 00
        01 03 01 cafdf3 04 03 01
            8acd73 03 8aca73 00 07 c25ba1 01 02 41 00 d72d39 00 01 00
            cf4866 05 01 0a 01 02 54 00 3fc00000
            cf4869 05 01 00 01 02 4e 00 02
            cf4873 05 01 01 01 02 56 00 02 43 00
            00
          cf7a64 00 05 00";

    #[test]
    fn response_layout_matches_independent_bytes_and_round_trips() {
        let response = InGameSpeedWallResponse {
            speed_walls: Some(SpeedWallsByStat(vec![
                (0, vec![wall(None)]),
                (1, vec![wall(Some(vec![row()]))]),
            ])),
            ..Default::default()
        };
        let bytes = response.encode(limits()).unwrap();
        assert_eq!(bytes, hex(RESPONSE));
        let decoded = InGameSpeedWallResponse::decode(&bytes, limits()).unwrap();
        assert_eq!(decoded.unknown_field_count(), 0);
        assert_eq!(decoded.encode(limits()).unwrap(), bytes);
        let walls = decoded.speed_walls.as_ref().unwrap();
        assert_eq!(walls.0.len(), 2);
        assert!(walls.0[0].1[0].rows.is_none());
        assert_eq!(walls.0[1].1[0].rows.as_ref().unwrap().0.len(), 1);
        // An empty body is an empty response; every other prefix is truncated.
        for end in 1..bytes.len() {
            assert!(InGameSpeedWallResponse::decode(&bytes[..end], limits()).is_err());
        }
    }

    #[test]
    fn request_decodes_and_rejects_bad_shapes() {
        // BLID 9, SWIS [1761118161], USGE 0, USPG 0.
        let mut list = Encoder::new(limits());
        list.integer(BLID, 9).unwrap();
        list.integer_list(SWIS, [1_761_118_161i64].into_iter())
            .unwrap();
        list.integer(USGE, 0).unwrap();
        list.integer(USPG, 0).unwrap();
        let bytes = list.finish().unwrap();
        assert_eq!(
            bytes,
            hex("8aca64 00 09 cf7a73 04 00 01 91 9f c4 8f 0d d739e5 00 00 d73c27 00 00")
        );
        let request = InGameSpeedWallsRequest::decode(&bytes, limits()).unwrap();
        assert_eq!(request.blaze_id, Some(9));
        assert_eq!(
            request.speed_wall_ids.as_ref().unwrap().0,
            vec![1_761_118_161]
        );
        assert_eq!((request.usge, request.uspg), (Some(0), Some(0)));
        assert_eq!(request.encode(limits()).unwrap(), bytes);
        // A string where the ID list belongs is the wrong type.
        let mut wrong = Encoder::new(limits());
        wrong.string(SWIS, b"x").unwrap();
        assert!(InGameSpeedWallsRequest::decode(&wrong.finish().unwrap(), limits()).is_err());
    }

    #[test]
    fn duplicate_stat_types_are_refused() {
        let response = InGameSpeedWallResponse {
            speed_walls: Some(SpeedWallsByStat(vec![
                (0, vec![wall(None)]),
                (0, vec![wall(None)]),
            ])),
            ..Default::default()
        };
        assert_eq!(
            response.encode(limits()).err(),
            Some(Error::DuplicateMapKey)
        );
    }
}
