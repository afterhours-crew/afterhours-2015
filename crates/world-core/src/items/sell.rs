// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! SellItemsOperation framing. Ownership, resale value and dependent-item policy
//! belong to the account service; an ID in this request authorizes nothing.
use super::{
    Catalog, Collection, Error, MAX_COLLECTION, PurchaseReply, Reader, SELL_ITEMS, Writer,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SellRequest {
    pub sequence: u32,
    pub items: Vec<u64>,
}

impl SellRequest {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes)?;
        let sequence = reader.u32()?;
        if reader.u8()? != SELL_ITEMS {
            return Err(Error::Shape);
        }
        let count = reader.u32()? as usize;
        if count > MAX_COLLECTION {
            return Err(Error::Bound);
        }
        let items = (0..count).map(|_| reader.u64()).collect::<Result<_, _>>()?;
        reader.finish()?;
        Ok(Self { sequence, items })
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.items.len() > MAX_COLLECTION {
            return Err(Error::Bound);
        }
        let mut writer = Writer::default();
        writer.u32(self.sequence)?;
        writer.bytes(&[SELL_ITEMS])?;
        writer.u32(self.items.len() as u32)?;
        for &id in &self.items {
            writer.u64(id)?;
        }
        Ok(writer.0)
    }
}

/// Success carries a length-delimited item collection, using the same native
/// collection serializer as purchase. Failure is only sequence and state3.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SellReply {
    pub sequence: u32,
    pub sold: Option<Collection>,
}

impl SellReply {
    pub fn encode(&self, catalog: &Catalog) -> Result<Vec<u8>, Error> {
        PurchaseReply {
            sequence: self.sequence,
            purchased: self.sold.clone(),
        }
        .encode(catalog)
    }

    pub fn decode(bytes: &[u8], catalog: &Catalog) -> Result<Self, Error> {
        let reply = PurchaseReply::decode(bytes, catalog)?;
        Ok(Self {
            sequence: reply.sequence,
            sold: reply.purchased,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_order_preserves_full_ids_without_an_owner_or_selector_tag() {
        // Independently arranged native fields: sequence32, opcode8, count32,
        // followed directly by count item IDs64. These are constructed values.
        let bytes = [
            0x78, 0x56, 0x34, 0x12, 4, 2, 0, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let request = SellRequest {
            sequence: 0x12345678,
            items: vec![0x0102030405060708, 0],
        };
        assert_eq!(SellRequest::decode(&bytes), Ok(request.clone()));
        assert_eq!(request.encode().unwrap(), bytes);
        for end in 0..bytes.len() {
            assert!(SellRequest::decode(&bytes[..end]).is_err());
        }
        assert_eq!(
            SellRequest::decode(&[bytes.as_slice(), &bytes].concat()),
            Err(Error::Trailing)
        );
        let mut wrong = bytes;
        wrong[4] = 2;
        assert_eq!(SellRequest::decode(&wrong), Err(Error::Shape));
        wrong[4] = SELL_ITEMS;
        wrong[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(SellRequest::decode(&wrong), Err(Error::Bound));
        assert_eq!(
            SellRequest {
                sequence: 2,
                items: vec![1; MAX_COLLECTION + 1]
            }
            .encode(),
            Err(Error::Bound)
        );
    }

    #[test]
    fn replies_require_exact_success_or_failure_framing() {
        let catalog = Catalog::default();
        for (bytes, sold) in [
            (vec![2, 0, 0, 0, 3], None),
            (
                vec![2, 0, 0, 0, 2, 4, 0, 0, 0, 0, 0, 0, 0],
                Some(Collection::default()),
            ),
        ] {
            let reply = SellReply { sequence: 2, sold };
            assert_eq!(reply.encode(&catalog).unwrap(), bytes);
            assert_eq!(SellReply::decode(&bytes, &catalog), Ok(reply));
            for end in 0..bytes.len() {
                assert!(SellReply::decode(&bytes[..end], &catalog).is_err());
            }
            assert_eq!(
                SellReply::decode(&[bytes.as_slice(), &[0]].concat(), &catalog),
                Err(Error::Trailing)
            );
        }
    }
}
