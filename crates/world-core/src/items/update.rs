// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

/// Server-initiated UpdateItems completion. The context is the native runtime
/// player index, not a Ghost id, account id or request sequence. This changes
/// existing records; inventory additions/removals use a separate operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateNotification {
    pub player_index: u32,
    pub items: Collection,
}
impl UpdateNotification {
    pub fn encode(&self, catalog: &Catalog) -> Result<Vec<u8>, Error> {
        let body = self.items.encode(catalog)?;
        if body.len() > MAX_BYTES - 13 {
            return Err(Error::Bound);
        }
        let mut writer = Writer::default();
        writer.u32(0)?;
        writer.bytes(&[2])?;
        writer.u32(self.player_index)?;
        writer.u32(body.len() as u32)?;
        writer.bytes(&body)?;
        Ok(writer.0)
    }
    pub fn decode(bytes: &[u8], catalog: &Catalog) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes)?;
        if reader.u32()? != 0 || reader.u8()? != 2 {
            return Err(Error::UnsupportedState);
        }
        let player_index = reader.u32()?;
        let length = reader.u32()? as usize;
        let items = Collection::decode(reader.take(length)?, catalog)?;
        reader.finish()?;
        Ok(Self {
            player_index,
            items,
        })
    }
}

/// Server-initiated inventory change: sequence 1, a state byte, the runtime
/// player index, then the added and the removed collections, each
/// length-delimited. The client builds the first collection's records as new
/// items and inserts them into the player's inventory; it resolves the
/// second's against that inventory and erases them (receiver 0x14406f9e0,
/// insert 0x14407d0c0, erase 0x14408ae70). An official host sent one added
/// record per notification for parts unlocked by a level-up (E926).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeNotification {
    pub player_index: u32,
    pub added: Collection,
    pub removed: Collection,
}
impl ChangeNotification {
    pub fn encode(&self, catalog: &Catalog) -> Result<Vec<u8>, Error> {
        let added = self.added.encode(catalog)?;
        let removed = self.removed.encode(catalog)?;
        if added.len().saturating_add(removed.len()) > MAX_BYTES - 17 {
            return Err(Error::Bound);
        }
        let mut writer = Writer::default();
        writer.u32(1)?;
        writer.bytes(&[2])?;
        writer.u32(self.player_index)?;
        for body in [&added, &removed] {
            writer.u32(body.len() as u32)?;
            writer.bytes(body)?;
        }
        Ok(writer.0)
    }
    pub fn decode(bytes: &[u8], catalog: &Catalog) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes)?;
        if reader.u32()? != 1 || reader.u8()? != 2 {
            return Err(Error::UnsupportedState);
        }
        let player_index = reader.u32()?;
        let length = reader.u32()? as usize;
        let added = Collection::decode(reader.take(length)?, catalog)?;
        let length = reader.u32()? as usize;
        let removed = Collection::decode(reader.take(length)?, catalog)?;
        reader.finish()?;
        Ok(Self {
            player_index,
            added,
            removed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_header_names_both_collections_and_checks_every_extent() {
        let catalog = Catalog::new([([0x11; 16], DefinitionClass::CurrencyItemData)]).unwrap();
        let record = Item {
            id: 9,
            definition: [0x11; 16],
            owner: 0,
            defaults: vec![],
            children: vec![],
            state: PURCHASABLE,
            buy_price: 215,
            sell_price: 139,
            derived: Derived::Empty,
        };
        let change = ChangeNotification {
            player_index: 2,
            added: Collection {
                roots: vec![9],
                items: [(9, record)].into(),
            },
            removed: Collection::default(),
        };
        let bytes = change.encode(&catalog).unwrap();
        let added = change.added.encode(&catalog).unwrap();
        let mut expected = vec![1, 0, 0, 0, 2, 2, 0, 0, 0];
        expected.extend((added.len() as u32).to_le_bytes());
        expected.extend(&added);
        expected.extend([4, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(bytes, expected);
        assert_eq!(ChangeNotification::decode(&bytes, &catalog), Ok(change));
        // Never mistaken for an update or the reverse.
        assert_eq!(
            UpdateNotification::decode(&bytes, &catalog),
            Err(Error::UnsupportedState)
        );
        let update = UpdateNotification {
            player_index: 2,
            items: Collection::default(),
        }
        .encode(&catalog)
        .unwrap();
        assert_eq!(
            ChangeNotification::decode(&update, &catalog),
            Err(Error::UnsupportedState)
        );
        for end in 0..bytes.len() {
            assert!(ChangeNotification::decode(&bytes[..end], &catalog).is_err());
        }
        let mut bad = bytes.clone();
        bad[4] = 3;
        assert_eq!(
            ChangeNotification::decode(&bad, &catalog),
            Err(Error::UnsupportedState)
        );
        bad = bytes.clone();
        bad[9..13].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(ChangeNotification::decode(&bad, &catalog).is_err());
        bad = bytes;
        bad.push(0);
        assert_eq!(
            ChangeNotification::decode(&bad, &catalog),
            Err(Error::Trailing)
        );
    }
    #[test]
    fn update_header_is_distinct_from_a_request_reply_and_checks_every_extent() {
        let catalog = Catalog::default();
        let update = UpdateNotification {
            player_index: 37,
            items: Collection::default(),
        };
        let bytes = update.encode(&catalog).unwrap();
        assert_eq!(bytes, [0, 0, 0, 0, 2, 37, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(UpdateNotification::decode(&bytes, &catalog), Ok(update));
        for end in 0..bytes.len() {
            assert!(UpdateNotification::decode(&bytes[..end], &catalog).is_err());
        }
        let mut bad = bytes.clone();
        bad[0] = 1;
        assert_eq!(
            UpdateNotification::decode(&bad, &catalog),
            Err(Error::UnsupportedState)
        );
        bad = bytes.clone();
        bad[4] = 3;
        assert_eq!(
            UpdateNotification::decode(&bad, &catalog),
            Err(Error::UnsupportedState)
        );
        bad = bytes.clone();
        bad[9..13].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(UpdateNotification::decode(&bad, &catalog).is_err());
        bad = bytes;
        bad.push(0);
        assert_eq!(
            UpdateNotification::decode(&bad, &catalog),
            Err(Error::Trailing)
        );
    }
}
