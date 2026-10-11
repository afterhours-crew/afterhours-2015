// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error, Generated};
pub const SEED_BYTES: usize = 48;
impl Generated {
    /// Microseconds since Unix epoch are an explicit local clock policy.
    /// The caller selects and documents its clock domain.
    pub fn from_seed(seed: &[u8], unix_micros: i64) -> Result<Self, Error> {
        if seed.len() != SEED_BYTES || seed.iter().all(|b| *b == 0) || unix_micros <= 0 {
            return Err(Error::Config);
        }
        let id = |at, domain| {
            u64::from(
                (u32::from_le_bytes(seed[at..at + 4].try_into().expect("checked seed"))
                    & 0x0fff_ffff)
                    | domain,
            )
        };
        let uuid = |at| {
            let mut bytes: [u8; 16] = seed[at..at + 16].try_into().expect("checked seed");
            bytes[6] = (bytes[6] & 15) | 0x40;
            bytes[8] = (bytes[8] & 63) | 0x80;
            let mut output = Vec::with_capacity(36);
            for (i, byte) in bytes.into_iter().enumerate() {
                if [4, 6, 8, 10].contains(&i) {
                    output.push(b'-');
                }
                output.push(b"0123456789abcdef"[usize::from(byte >> 4)]);
                output.push(b"0123456789abcdef"[usize::from(byte & 15)]);
            }
            output
        };
        let game_id = id(0, 0x4000_0000);
        let result = Self {
            game_id,
            reporting_id: id(4, 0x5000_0000),
            player_session_id: id(8, 0x6000_0000),
            seed: u32::from_le_bytes(seed[12..16].try_into().expect("checked seed")),
            create_time: unix_micros,
            join_time: unix_micros,
            game_uuid: uuid(16),
            player_uuid: uuid(32),
            external_session_name: format!("local-{game_id:08x}").into_bytes(),
        };
        result.validate()?;
        Ok(result)
    }
}
