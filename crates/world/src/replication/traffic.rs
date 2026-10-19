// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Traffic goals refer to paths in the client's static spline catalog.
//! Their creation carries an allocation counter and asset reference only;
//! subsequent updates have no body. Vehicle spawning and authority are separate.
use super::entity::creation::{Asset, Catalog};
use super::{Decoded, Error, Kind, Reader, Record, Update, Writer};
use nfs_protocol::world::BitSpan;

pub use super::spline::Content;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Initial {
    pub asset: Asset,
    pub catalog: Catalog,
    /// Shared goal allocation counter, wrapping at 64.
    pub allocation_serial: u8,
    pub asset_mode: u8,
}
impl Initial {
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        if self.allocation_serial >= 64 {
            return Err(Error::Bound);
        }
        if !matches!(self.asset_mode, 2 | 3) {
            return Err(Error::Unsupported);
        }
        let index = self.catalog.flatten(self.asset)?;
        w.put(u64::from(self.allocation_serial), 6)?;
        w.put(u64::from(self.asset_mode), 2)?;
        w.put(u64::from(self.asset.bundle), 11)?;
        w.put(u64::from(index), usize::from(self.catalog.width()))
    }
}

impl Record {
    /// The spline catalog must be supplied for creation. Implicit and null
    /// assets remain unsupported: neither supplies a usable traffic path.
    pub fn decode_traffic(
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
        if kind != Kind::TrafficGoal {
            return Err(Error::TypeMismatch);
        }
        let initial = if create {
            let allocation_serial = r.take(6)? as u8;
            let asset_mode = r.take(2)? as u8;
            if !matches!(asset_mode, 2 | 3) {
                return Err(Error::Unsupported);
            }
            let bundle = r.take(11)? as u16;
            let catalog = content
                .and_then(|c| c.catalog(bundle))
                .ok_or(Error::Unsupported)?;
            let asset = catalog.resolve(r.take(catalog.width())? as u16)?;
            Some(super::Initial::TrafficGoal(Initial {
                asset,
                catalog: catalog.clone(),
                allocation_serial,
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
                update: Update::TrafficGoal,
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
        let initial = Initial {
            asset: Asset {
                bundle: 23,
                type_id: 5101,
                local_index: 1,
            },
            catalog: catalog.clone(),
            allocation_serial: 37,
            asset_mode: 2,
        };
        (
            Record {
                id: 27,
                initial: Some(super::super::Initial::TrafficGoal(initial)),
                update: Update::TrafficGoal,
            },
            Content::new(vec![catalog]).unwrap(),
        )
    }

    #[test]
    fn exact_creation_boundary_and_truncation() {
        let (record, content) = sample();
        let wire = record.encode().unwrap();
        let mut expected = BitWriter::new();
        for (v, n) in [(27, 13), (1, 1), (26, 5), (37, 6), (2, 2), (23, 11), (4, 3)] {
            expected.put(v, n);
        }
        assert_eq!(wire, expected);
        for n in 0..wire.len() {
            assert!(
                Record::decode_traffic(wire.span().slice(0, n).unwrap(), None, Some(&content))
                    .is_err()
            );
        }
        let mut joined = wire.clone();
        joined.put_span(wire.span());
        let first = Record::decode_traffic(joined.span(), None, Some(&content)).unwrap();
        assert_eq!(first.record, record);
        assert_eq!(first.initial_bits, Some(wire.len()));
        assert_eq!(first.bits, wire.len());
        assert_eq!(
            Record::decode_traffic(
                joined.span().after(first.bits).unwrap(),
                None,
                Some(&content)
            )
            .unwrap()
            .record,
            record
        );
        assert_eq!(Record::decode(wire.span(), None), Err(Error::Unsupported));
    }

    #[test]
    fn invalid_assets_and_counters_do_not_allocate() {
        let (record, content) = sample();
        let Some(super::super::Initial::TrafficGoal(initial)) = record.initial.clone() else {
            panic!()
        };
        for field in 0..5 {
            let mut bad = initial.clone();
            match field {
                0 => bad.allocation_serial = 64,
                1 => bad.asset_mode = 1,
                2 => bad.asset.local_index = 2,
                3 => bad.asset.bundle = 24,
                _ => bad.asset.type_id = 5102,
            }
            let mut world = super::super::state::World::default();
            assert!(
                world
                    .spawn(super::super::Initial::TrafficGoal(bad), Update::TrafficGoal)
                    .is_err()
            );
            assert!(world.is_empty());
            assert_eq!(
                world
                    .spawn(record.initial.clone().unwrap(), record.update.clone())
                    .unwrap()
                    .id,
                1
            );
        }
        let wire = record.encode().unwrap();
        let mut bad = BitWriter::new();
        bad.put_span(wire.span().slice(0, 38).unwrap());
        bad.put(5, 3);
        assert!(Record::decode_traffic(bad.span(), None, Some(&content)).is_err());
    }

    #[test]
    fn section_creation_update_deletion_and_binding_isolation() {
        use super::super::section::{Bindings, Section};
        let (record, content) = sample();
        let mut bindings = Bindings::default().with_splines(content);
        let mut section = Section {
            float_bits: None,
            flag: false,
            deleted: vec![],
            setup: Some(super::super::section::Setup::Packed {
                tag: None,
                width: None,
                axes: [None; 3],
            }),
            records: vec![record],
        };
        let wire = section.encode().unwrap();
        assert_eq!(bindings.decode(wire.span()).unwrap().section, section);
        section.records[0].initial = None;
        let wire = section.encode().unwrap();
        assert_eq!(bindings.decode(wire.span()).unwrap().section, section);
        assert!(Bindings::default().decode(wire.span()).is_err());
        assert_eq!(
            Record::decode(
                section.records[0].encode().unwrap().span(),
                Some(Kind::TrafficGoal)
            )
            .unwrap()
            .bits,
            14
        );
        section.records.clear();
        section.setup = None;
        section.deleted.push(27);
        assert_eq!(
            bindings
                .decode(section.encode().unwrap().span())
                .unwrap()
                .section,
            section
        );
        assert!(bindings.is_empty());
    }
}
