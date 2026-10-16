// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! PurchaseItemsOperation request framing. This codec grants no ownership and
//! does not choose a price, authorize a purchase or infer a successful reply.
use super::{
    Catalog, Collection, Envelope, Error, Guid, MAX_COLLECTION, PURCHASE_ITEMS, Reader, Writer,
};

/// A successful purchase returns its resulting items. Rejection is a five-byte
/// sequence/state header: it has neither a length word nor a collection body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurchaseReply {
    pub sequence: u32,
    pub purchased: Option<Collection>,
}

impl PurchaseReply {
    pub fn encode(&self, catalog: &Catalog) -> Result<Vec<u8>, Error> {
        if let Some(items) = &self.purchased {
            Envelope::from_collection(self.sequence, items, catalog)?.encode()
        } else {
            let mut bytes = self.sequence.to_le_bytes().to_vec();
            bytes.push(3);
            Ok(bytes)
        }
    }

    pub fn decode(bytes: &[u8], catalog: &Catalog) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes)?;
        let sequence = reader.u32()?;
        let purchased = match reader.u8()? {
            2 => Some(Envelope::decode(bytes)?.collection(catalog)?),
            3 => {
                reader.finish()?;
                None
            }
            _ => return Err(Error::UnsupportedState),
        };
        Ok(Self {
            sequence,
            purchased,
        })
    }
}

/// Native selector tags distinguish item IDs (2) from definition GUIDs (4).
/// Tag 2 does not prove that the selected records are owned or purchasable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PurchaseSelection {
    Items(Vec<u64>),
    Definitions(Vec<Guid>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurchaseRequest {
    pub sequence: u32,
    pub owner: u64,
    pub selection: PurchaseSelection,
}

impl PurchaseRequest {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes)?;
        let sequence = reader.u32()?;
        if reader.u8()? != PURCHASE_ITEMS {
            return Err(Error::Shape);
        }
        let owner = reader.u64()?;
        let selector = reader.u8()?;
        let count = reader.u32()? as usize;
        if count > MAX_COLLECTION {
            return Err(Error::Bound);
        }
        let selection = match selector {
            2 => PurchaseSelection::Items(
                (0..count).map(|_| reader.u64()).collect::<Result<_, _>>()?,
            ),
            4 => PurchaseSelection::Definitions(
                (0..count)
                    .map(|_| reader.take(16)?.try_into().map_err(|_| Error::Shape))
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err(Error::UnsupportedState),
        };
        reader.finish()?;
        Ok(Self {
            sequence,
            owner,
            selection,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let (tag, count) = match &self.selection {
            PurchaseSelection::Items(ids) => (2, ids.len()),
            PurchaseSelection::Definitions(guids) => (4, guids.len()),
        };
        if count > MAX_COLLECTION {
            return Err(Error::Bound);
        }
        let mut writer = Writer::default();
        writer.u32(self.sequence)?;
        writer.bytes(&[PURCHASE_ITEMS])?;
        writer.u64(self.owner)?;
        writer.bytes(&[tag])?;
        writer.u32(count as u32)?;
        match &self.selection {
            PurchaseSelection::Items(ids) => {
                for &id in ids {
                    writer.u64(id)?;
                }
            }
            PurchaseSelection::Definitions(guids) => {
                for guid in guids {
                    writer.bytes(guid)?;
                }
            }
        }
        Ok(writer.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_has_no_length_word_and_success_retains_collection_framing() {
        let catalog = Catalog::default();
        for (bytes, purchased) in [
            (vec![0x78, 0x56, 0x34, 0x12, 3], None),
            (
                vec![0x78, 0x56, 0x34, 0x12, 2, 4, 0, 0, 0, 0, 0, 0, 0],
                Some(Collection::default()),
            ),
        ] {
            let reply = PurchaseReply {
                sequence: 0x12345678,
                purchased,
            };
            assert_eq!(reply.encode(&catalog).unwrap(), bytes);
            assert_eq!(PurchaseReply::decode(&bytes, &catalog), Ok(reply));
            for end in 0..bytes.len() {
                assert!(PurchaseReply::decode(&bytes[..end], &catalog).is_err());
            }
            assert_eq!(
                PurchaseReply::decode(&[bytes.as_slice(), &[0]].concat(), &catalog),
                Err(Error::Trailing)
            );
        }
    }

    // Original test values, arranged independently from the native field order:
    // u32 sequence, u8 operation, u64 owner, u8 selector, u32 count, entries.
    const IDS: &[u8] = &[
        0x78, 0x56, 0x34, 0x12, 2, 8, 7, 6, 5, 4, 3, 2, 1, 2, 2, 0, 0, 0, 0x18, 0x17, 0x16, 0x15,
        0x14, 0x13, 0x12, 0x11, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    const GUIDS: &[u8] = &[
        3, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 4, 1, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
        12, 13, 14, 15,
    ];

    #[test]
    fn grounded_field_order_preserves_ids_and_guid_bytes() {
        let ids = PurchaseRequest {
            sequence: 0x12345678,
            owner: 0x0102030405060708,
            selection: PurchaseSelection::Items(vec![0x1112131415161718, 0]),
        };
        let guids = PurchaseRequest {
            sequence: 3,
            owner: 0,
            selection: PurchaseSelection::Definitions(vec![std::array::from_fn(|i| i as u8)]),
        };
        for (wire, request) in [(IDS, ids), (GUIDS, guids)] {
            assert_eq!(PurchaseRequest::decode(wire), Ok(request.clone()));
            assert_eq!(request.encode().unwrap(), wire);
            for length in 0..wire.len() {
                assert!(PurchaseRequest::decode(&wire[..length]).is_err());
            }
            let concatenated = [wire, wire].concat();
            assert_eq!(PurchaseRequest::decode(&concatenated), Err(Error::Trailing));
        }
    }

    #[test]
    fn rejects_wrong_route_selector_and_unbounded_count() {
        let mut wire = GUIDS.to_vec();
        wire[4] = 0x10;
        assert_eq!(PurchaseRequest::decode(&wire), Err(Error::Shape));
        wire[4] = PURCHASE_ITEMS;
        wire[13] = 8;
        assert_eq!(PurchaseRequest::decode(&wire), Err(Error::UnsupportedState));
        wire[13] = 4;
        wire[14..18].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(PurchaseRequest::decode(&wire), Err(Error::Bound));
        let request = PurchaseRequest {
            sequence: 0,
            owner: 0,
            selection: PurchaseSelection::Items(vec![1; MAX_COLLECTION + 1]),
        };
        assert_eq!(request.encode(), Err(Error::Bound));
    }

    #[test]
    fn empty_and_repeated_selectors_are_preserved_for_domain_validation() {
        for selection in [
            PurchaseSelection::Items(vec![]),
            PurchaseSelection::Definitions(vec![]),
            PurchaseSelection::Items(vec![7, 7]),
        ] {
            let request = PurchaseRequest {
                sequence: u32::MAX,
                owner: 9,
                selection,
            };
            assert_eq!(
                PurchaseRequest::decode(&request.encode().unwrap()),
                Ok(request)
            );
        }
    }
}
