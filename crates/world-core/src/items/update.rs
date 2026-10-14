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

#[cfg(test)]
mod tests {
    use super::*;
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
