// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Player view ownership and its two ordered network serializers.
//! Quantized values retain their wire integers; unproven gameplay meanings
//! deliberately remain numeric fields.
use super::vehicle::{Quaternion25, Vector};
use super::{Error, Reader, Writer};
pub const MAX_PLAYERS: usize = 70;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Initial {
    pub player: u16,
    pub serial: u16,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Update {
    pub base: Option<Base>,
    pub view: Option<View>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Players {
    pub serial: u16,
    pub players: Vec<u16>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Seat {
    pub vehicle: u16,
    pub index: u8,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Base {
    pub active: bool,
    pub players: Option<Players>,
    pub value13: Option<u16>,
    pub flag_a: Option<bool>,
    pub target: Option<Option<u16>>,
    pub value8: Option<u8>,
    pub seat: Option<Option<Seat>>,
    pub flag_b: Option<bool>,
    pub value16: Option<u16>,
    pub motion: Option<Motion>,
    pub value6: Option<u8>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Motion {
    pub vectors: [Vector; 2],
    pub values6: [u8; 2],
    pub flags: [bool; 2],
    pub serial: u8,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct View {
    pub transform: Option<Transform>,
    pub mode: Option<u8>,
    pub value3: Option<u8>,
    pub vehicle_a: Option<u16>,
    pub vehicle_b: Option<(u16, Option<u8>)>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transform {
    pub rotation: Quaternion25,
    pub position: Vector,
    pub relative: Option<(Quaternion25, Vector)>,
}
impl Initial {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            player: r.reference()?,
            serial: r.take(16)? as u16,
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.reference(&self.player)?;
        w.put(u64::from(self.serial), 16)
    }
}
impl Update {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            base: r.optional(Base::read)?,
            view: r.optional(View::read)?,
        })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.optional(&self.base, |w, b| b.write(w))?;
        w.optional(&self.view, |w, b| b.write(w))
    }
    pub(super) fn merge(&mut self, newer: &Self) {
        if let Some(b) = &newer.base {
            let a = self.base.get_or_insert_with(Base::default);
            a.active |= b.active;
            replace(&mut a.players, &b.players);
            replace(&mut a.value13, &b.value13);
            replace(&mut a.flag_a, &b.flag_a);
            replace(&mut a.target, &b.target);
            replace(&mut a.value8, &b.value8);
            replace(&mut a.seat, &b.seat);
            replace(&mut a.flag_b, &b.flag_b);
            replace(&mut a.value16, &b.value16);
            replace(&mut a.motion, &b.motion);
            replace(&mut a.value6, &b.value6);
        }
        if let Some(b) = &newer.view {
            let a = self.view.get_or_insert_with(View::default);
            replace(&mut a.transform, &b.transform);
            replace(&mut a.mode, &b.mode);
            replace(&mut a.value3, &b.value3);
            replace(&mut a.vehicle_a, &b.vehicle_a);
            replace(&mut a.vehicle_b, &b.vehicle_b);
        }
    }
}
fn replace<T: Clone>(a: &mut Option<T>, b: &Option<T>) {
    if b.is_some() {
        *a = b.clone();
    }
}
impl Base {
    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            active: r.bit()?,
            players: r.optional(|r| {
                let count = r.take(7)? as usize;
                if count > MAX_PLAYERS {
                    return Err(Error::Bound);
                }
                let serial = r.take(16)? as u16;
                let players = (0..count)
                    .map(|_| r.reference())
                    .collect::<Result<_, _>>()?;
                Ok(Players { serial, players })
            })?,
            value13: r.optional(|r| Ok(r.take(13)? as u16))?,
            flag_a: r.optional(Reader::bit)?,
            target: r.optional(|r| r.optional(Reader::reference))?,
            value8: r.optional(|r| Ok(r.take(8)? as u8))?,
            seat: r.optional(|r| {
                r.optional(|r| {
                    Ok(Seat {
                        vehicle: r.reference()?,
                        index: r.take(4)? as u8,
                    })
                })
            })?,
            flag_b: r.optional(Reader::bit)?,
            value16: r.optional(|r| Ok(r.take(16)? as u16))?,
            motion: r.optional(|r| {
                Ok(Motion {
                    vectors: [Vector::read(r, 5)?, Vector::read(r, 5)?],
                    values6: [r.take(6)? as u8, r.take(6)? as u8],
                    flags: [r.bit()?, r.bit()?],
                    serial: r.take(5)? as u8,
                })
            })?,
            value6: r.optional(|r| Ok(r.take(6)? as u8))?,
        })
    }
    fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.bit(self.active)?;
        w.optional(&self.players, |w, p| {
            if p.players.len() > MAX_PLAYERS {
                return Err(Error::Bound);
            }
            w.put(p.players.len() as u64, 7)?;
            w.put(u64::from(p.serial), 16)?;
            for player in &p.players {
                w.reference(player)?;
            }
            Ok(())
        })?;
        w.optional(&self.value13, |w, v| w.put(u64::from(*v), 13))?;
        w.optional(&self.flag_a, |w, v| w.bit(*v))?;
        w.optional(&self.target, |w, v| w.optional(v, Writer::reference))?;
        w.optional(&self.value8, |w, v| w.put(u64::from(*v), 8))?;
        w.optional(&self.seat, |w, s| {
            w.optional(s, |w, s| {
                w.reference(&s.vehicle)?;
                w.put(u64::from(s.index), 4)
            })
        })?;
        w.optional(&self.flag_b, |w, v| w.bit(*v))?;
        w.optional(&self.value16, |w, v| w.put(u64::from(*v), 16))?;
        w.optional(&self.motion, |w, m| {
            for v in &m.vectors {
                v.write(w, 5)?;
            }
            for v in m.values6 {
                w.put(u64::from(v), 6)?;
            }
            for v in m.flags {
                w.bit(v)?;
            }
            w.put(u64::from(m.serial), 5)
        })?;
        w.optional(&self.value6, |w, v| w.put(u64::from(*v), 6))
    }
}
impl View {
    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            transform: r.optional(|r| {
                Ok(Transform {
                    rotation: Quaternion25::read(r)?,
                    position: Vector::read(r, 5)?,
                    relative: r.optional(|r| Ok((Quaternion25::read(r)?, Vector::read(r, 5)?)))?,
                })
            })?,
            mode: r.optional(|r| {
                let v = r.take(2)? as u8;
                if v > 2 { Err(Error::Shape) } else { Ok(v) }
            })?,
            value3: r.optional(|r| Ok(r.take(3)? as u8))?,
            vehicle_a: r.optional(Reader::reference)?,
            vehicle_b: r
                .optional(|r| Ok((r.reference()?, r.optional(|r| Ok(r.take(4)? as u8))?)))?,
        })
    }
    fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.optional(&self.transform, |w, t| {
            t.rotation.write(w)?;
            t.position.write(w, 5)?;
            w.optional(&t.relative, |w, (r, p)| {
                r.write(w)?;
                p.write(w, 5)
            })
        })?;
        w.optional(&self.mode, |w, v| {
            if *v > 2 {
                return Err(Error::Shape);
            }
            w.put(u64::from(*v), 2)
        })?;
        w.optional(&self.value3, |w, v| w.put(u64::from(*v), 3))?;
        w.optional(&self.vehicle_a, Writer::reference)?;
        w.optional(&self.vehicle_b, |w, (v, s)| {
            w.reference(v)?;
            w.optional(s, |w, s| w.put(u64::from(*s), 4))
        })
    }
}

impl Update {
    pub(crate) fn validate_references(
        &self,
        mut kind: impl FnMut(u16) -> Option<super::Kind>,
    ) -> Result<(), Error> {
        let mut require = |id, expected| {
            if kind(id) == Some(expected) {
                Ok(())
            } else {
                Err(Error::UnknownObject)
            }
        };
        if let Some(base) = &self.base {
            if let Some(players) = &base.players {
                for &p in &players.players {
                    require(p, super::Kind::Player)?;
                }
            }
            if let Some(Some(target)) = base.target {
                require(target, super::Kind::Entity)?;
            }
            if let Some(Some(seat)) = &base.seat {
                require(seat.vehicle, super::Kind::Entity)?;
            }
        }
        if let Some(view) = &self.view {
            for id in view
                .vehicle_a
                .into_iter()
                .chain(view.vehicle_b.map(|(id, _)| id))
            {
                if id != 0 {
                    require(id, super::Kind::Entity)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::replication::{Initial as Init, Kind, Record, Update as Delta};
    use nfs_protocol::world::BitSpan;
    fn record() -> Record {
        let vector = Vector::Packed {
            mode: 3,
            values: [32, 0, -96],
        };
        let rotation = Quaternion25::from_quaternion([0., 1., 0., 0.]).unwrap();
        Record {
            id: 7,
            initial: Some(Init::PlayerView(Initial {
                player: 3,
                serial: 17,
            })),
            update: Delta::PlayerView(Update {
                base: Some(Base {
                    active: true,
                    players: Some(Players {
                        serial: 18,
                        players: vec![3, 4],
                    }),
                    value13: Some(4000),
                    flag_a: Some(false),
                    target: Some(Some(12)),
                    value8: Some(90),
                    seat: Some(Some(Seat {
                        vehicle: 12,
                        index: 0,
                    })),
                    flag_b: Some(true),
                    value16: Some(71),
                    motion: Some(Motion {
                        vectors: [vector.clone(), vector.clone()],
                        values6: [7, 32],
                        flags: [true, false],
                        serial: 31,
                    }),
                    value6: Some(63),
                }),
                view: Some(View {
                    transform: Some(Transform {
                        rotation: rotation.clone(),
                        position: vector,
                        relative: Some((
                            rotation,
                            Vector::Packed {
                                mode: 1,
                                values: [0, 1, -1],
                            },
                        )),
                    }),
                    mode: Some(2),
                    value3: Some(5),
                    vehicle_a: Some(12),
                    vehicle_b: Some((12, Some(0))),
                }),
            }),
        }
    }
    #[test]
    fn both_serializers_roundtrip_every_truncation_and_adjacent_records() {
        let record = record();
        let encoded = record.encode().unwrap();
        let decoded = Record::decode(encoded.span(), None).unwrap();
        assert_eq!(decoded.record, record);
        assert_eq!(decoded.bits, encoded.len());
        for end in 0..encoded.len() {
            assert!(
                Record::decode(BitSpan::new(encoded.bytes(), 0, end).unwrap(), None).is_err(),
                "{end}"
            );
        }
        let mut both = BitWriter::new();
        both.put_span(encoded.span()).put_span(encoded.span());
        let first = Record::decode(both.span(), None).unwrap();
        let second = Record::decode(both.span().after(first.bits).unwrap(), None).unwrap();
        assert_eq!(second.record, record);
        assert_eq!(first.bits + second.bits, both.len());
    }
    #[test]
    fn independent_outer_presence_clear_and_merge() {
        for (base, view) in [
            (None, None),
            (Some(Base::default()), None),
            (None, Some(View::default())),
        ] {
            let record = Record {
                id: 2,
                initial: None,
                update: Delta::PlayerView(Update { base, view }),
            };
            let wire = record.encode().unwrap();
            assert_eq!(
                Record::decode(wire.span(), Some(Kind::PlayerView))
                    .unwrap()
                    .record,
                record
            );
        }
        let Delta::PlayerView(mut update) = record().update else {
            panic!()
        };
        update.merge(&Update {
            base: Some(Base {
                target: Some(None),
                seat: Some(None),
                players: Some(Players {
                    serial: 19,
                    players: vec![],
                }),
                ..Base::default()
            }),
            view: Some(View {
                vehicle_a: Some(0),
                vehicle_b: Some((0, None)),
                ..View::default()
            }),
        });
        let base = update.base.unwrap();
        assert_eq!(base.target, Some(None));
        assert_eq!(base.seat, Some(None));
        assert!(base.players.unwrap().players.is_empty());
        assert_eq!(base.value13, Some(4000));
        assert_eq!(update.view.unwrap().vehicle_b, Some((0, None)));
    }
    #[test]
    fn field_and_collection_bounds_reject_without_truncating() {
        let make = |base, view| Record {
            id: 1,
            initial: None,
            update: Delta::PlayerView(Update {
                base: Some(base),
                view: Some(view),
            }),
        };
        assert!(
            make(
                Base {
                    players: Some(Players {
                        serial: 0,
                        players: vec![1; MAX_PLAYERS + 1]
                    }),
                    ..Base::default()
                },
                View::default()
            )
            .encode()
            .is_err()
        );
        for base in [
            Base {
                value13: Some(8192),
                ..Base::default()
            },
            Base {
                seat: Some(Some(Seat {
                    vehicle: 12,
                    index: 16,
                })),
                ..Base::default()
            },
            Base {
                target: Some(Some(8192)),
                ..Base::default()
            },
            Base {
                value6: Some(64),
                ..Base::default()
            },
        ] {
            assert!(make(base, View::default()).encode().is_err());
        }
        for view in [
            View {
                mode: Some(3),
                ..View::default()
            },
            View {
                value3: Some(8),
                ..View::default()
            },
            View {
                vehicle_b: Some((1, Some(16))),
                ..View::default()
            },
        ] {
            assert!(make(Base::default(), view).encode().is_err());
        }
        let mut bad = BitWriter::new();
        bad.put(1, 13)
            .put(0, 1)
            .put(1, 1)
            .put(0, 1)
            .put(1, 1)
            .put(71, 7);
        assert_eq!(
            Record::decode(bad.span(), Some(Kind::PlayerView)),
            Err(Error::Bound)
        );
    }
}
