// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scripted spline goals. Path geometry remains in the client's static content.
//! Their asset catalog is distinct from the Entity asset catalog.
use super::entity::creation::{Asset, Catalog, MAX_CATALOGS};
use super::{Decoded, Error, Kind, Reader, Record, Update, Writer};
use nfs_protocol::world::BitSpan;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Definition {
    pub asset: Asset,
    pub catalog: Catalog,
    pub flag: bool,
}
impl Definition {
    pub fn validate(&self) -> Result<(), Error> {
        self.catalog.flatten(self.asset)?;
        Ok(())
    }
    pub fn initial(&self, allocation_serial: u8) -> Result<Initial, Error> {
        self.validate()?;
        if allocation_serial >= 64 {
            return Err(Error::Bound);
        }
        Ok(Initial {
            definition: self.clone(),
            allocation_serial,
            selector: 0,
            serial: 1,
            asset_mode: 2,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Initial {
    pub definition: Definition,
    /// Goal allocation counter, wrapping at 64; independent of the RPC serial.
    pub allocation_serial: u8,
    pub selector: u16,
    pub serial: u16,
    pub asset_mode: u8,
}
impl Initial {
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        self.definition.validate()?;
        if self.allocation_serial >= 64 {
            return Err(Error::Bound);
        }
        if !matches!(self.asset_mode, 2 | 3) {
            return Err(Error::Unsupported);
        }
        w.put(u64::from(self.selector), 9)?;
        w.put(u64::from(self.serial), 10)?;
        w.put(u64::from(self.allocation_serial), 6)?;
        w.put(u64::from(self.asset_mode), 2)?;
        w.put(u64::from(self.definition.asset.bundle), 11)?;
        w.put(
            u64::from(self.definition.catalog.flatten(self.definition.asset)?),
            usize::from(self.definition.catalog.width()),
        )?;
        w.bit(self.definition.flag)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Content {
    catalogs: BTreeMap<u16, Catalog>,
}
impl Content {
    pub(super) fn catalog(&self, bundle: u16) -> Option<&Catalog> {
        self.catalogs.get(&bundle)
    }
    pub fn new(catalogs: Vec<Catalog>) -> Result<Self, Error> {
        if catalogs.is_empty() || catalogs.len() > MAX_CATALOGS {
            return Err(Error::Bound);
        }
        let mut result = BTreeMap::new();
        for catalog in catalogs {
            if result.insert(catalog.bundle(), catalog).is_some() {
                return Err(Error::Shape);
            }
        }
        Ok(Self { catalogs: result })
    }
}

impl Record {
    /// Decode a goal with its own static catalog. Null/implicit asset modes
    /// are unsupported; their external context must never be guessed.
    pub fn decode_spline(
        input: BitSpan<'_>,
        bound: Option<Kind>,
        content: Option<&Content>,
    ) -> Result<Decoded, Error> {
        let mut r = Reader {
            span: input,
            pos: 0,
        };
        let id = r.reference()?;
        let create = r.bit()?;
        let kind = if create {
            Kind::from_index(r.take(5)? as u8)?
        } else {
            bound.ok_or(Error::UnknownObject)?
        };
        if kind != Kind::ScriptedSplineGoal {
            return Err(Error::TypeMismatch);
        }
        let initial = if create {
            let selector = r.take(9)? as u16;
            let serial = r.take(10)? as u16;
            let allocation_serial = r.take(6)? as u8;
            let asset_mode = r.take(2)? as u8;
            if !matches!(asset_mode, 2 | 3) {
                return Err(Error::Unsupported);
            }
            let bundle = r.take(11)? as u16;
            let catalog = content
                .and_then(|c| c.catalogs.get(&bundle))
                .ok_or(Error::Unsupported)?;
            let asset = catalog.resolve(r.take(catalog.width())? as u16)?;
            let flag = r.bit()?;
            Some(super::Initial::ScriptedSplineGoal(Initial {
                definition: Definition {
                    asset,
                    catalog: catalog.clone(),
                    flag,
                },
                allocation_serial,
                selector,
                serial,
                asset_mode,
            }))
        } else {
            None
        };
        Ok(Decoded {
            initial_bits: initial.as_ref().map(|_| r.pos),
            bits: r.pos,
            record: Self {
                id,
                initial,
                update: Update::ScriptedSplineGoal,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    fn sample() -> (Record, Content) {
        let catalog = Catalog::new(23, &[(5100, 3), (5101, 2)]).unwrap();
        let definition = Definition {
            asset: Asset {
                bundle: 23,
                type_id: 5101,
                local_index: 1,
            },
            catalog: catalog.clone(),
            flag: true,
        };
        (
            Record {
                id: 27,
                initial: Some(super::super::Initial::ScriptedSplineGoal(
                    definition.initial(37).unwrap(),
                )),
                update: Update::ScriptedSplineGoal,
            },
            Content::new(vec![catalog]).unwrap(),
        )
    }
    #[test]
    fn exact_fields_truncation_and_concatenation() {
        let (record, content) = sample();
        let wire = record.encode().unwrap();
        let mut expected = BitWriter::new();
        for (v, n) in [
            (27, 13),
            (1, 1),
            (20, 5),
            (0, 9),
            (1, 10),
            (37, 6),
            (2, 2),
            (23, 11),
            (4, 3),
            (1, 1),
        ] {
            expected.put(v, n);
        }
        assert_eq!(wire, expected);
        for n in 0..wire.len() {
            assert!(
                Record::decode_spline(wire.span().slice(0, n).unwrap(), None, Some(&content))
                    .is_err()
            );
        }
        let mut joined = wire.clone();
        joined.put_span(wire.span());
        let first = Record::decode_spline(joined.span(), None, Some(&content)).unwrap();
        assert_eq!(first.record, record);
        assert_eq!(first.bits, wire.len());
        assert_eq!(
            Record::decode_spline(
                joined.span().after(first.bits).unwrap(),
                None,
                Some(&content)
            )
            .unwrap()
            .record,
            record
        );
        let update = Record {
            id: 27,
            initial: None,
            update: Update::ScriptedSplineGoal,
        };
        assert_eq!(
            Record::decode(
                update.encode().unwrap().span(),
                Some(Kind::ScriptedSplineGoal)
            )
            .unwrap()
            .bits,
            14
        );
        assert!(Record::decode(wire.span(), None).is_err());
    }
    #[test]
    fn invalid_catalog_fields_and_modes_are_rejected() {
        let (record, content) = sample();
        let Some(super::super::Initial::ScriptedSplineGoal(initial)) = record.initial.clone()
        else {
            panic!()
        };
        for field in 0..5 {
            let mut bad = initial.clone();
            match field {
                0 => bad.selector = 512,
                1 => bad.serial = 1024,
                2 => bad.allocation_serial = 64,
                3 => bad.definition.asset.local_index = 2,
                _ => bad.asset_mode = 1,
            }
            let bad = Record {
                initial: Some(super::super::Initial::ScriptedSplineGoal(bad)),
                ..record.clone()
            };
            assert!(bad.encode().is_err());
        }
        let encoded = record.encode().unwrap();
        let mut bad = BitWriter::new();
        // Explicitly construct an out-of-range catalog index (5 of 5).
        bad.put_span(encoded.span().slice(0, 57).unwrap());
        bad.put(5, 3);
        bad.put(1, 1);
        assert!(Record::decode_spline(bad.span(), None, Some(&content)).is_err());
    }
}
