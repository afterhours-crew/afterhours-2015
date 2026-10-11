// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Allocation, Current, Error};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub const SEED_BYTES: usize = 40;

/// A fresh local world and dedicated host. The edge must supply unique entropy
/// and keep the advertised endpoint bound for the allocation's lifetime.
#[derive(Clone)]
pub struct Generated {
    pub game_id: u64,
    pub reporting_id: u64,
    pub host_persona: i64,
    pub host_uid: u64,
    pub host_connection: u64,
    pub shared_seed: u32,
    pub clock: i64,
    pub endpoint: SocketAddr,
    pub uuid: Vec<u8>,
    name: Vec<u8>,
    external_name: Vec<u8>,
}
impl Generated {
    /// Raw injected entropy: five ID words, a shared seed and a UUID. Domain
    /// prefixes are local allocation policy, not remote account identifiers.
    pub fn from_seed(seed: &[u8], clock: i64, endpoint: SocketAddr) -> Result<Self, Error> {
        if seed.len() != SEED_BYTES
            || seed.iter().all(|b| *b == 0)
            || clock <= 0
            || endpoint.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST)
            || endpoint.port() == 0
        {
            return Err(Error::Context);
        }
        let word = |at| u32::from_le_bytes(seed[at..at + 4].try_into().expect("checked entropy"));
        let id = |at, domain| u64::from((word(at) & 0x0fff_ffff) | domain);
        let shared_seed = word(20);
        if shared_seed == 0 {
            return Err(Error::Context);
        }
        let mut bytes: [u8; 16] = seed[24..40].try_into().expect("checked entropy");
        bytes[6] = (bytes[6] & 15) | 0x40;
        bytes[8] = (bytes[8] & 63) | 0x80;
        let mut uuid = Vec::with_capacity(36);
        for (i, b) in bytes.into_iter().enumerate() {
            if [4, 6, 8, 10].contains(&i) {
                uuid.push(b'-');
            }
            uuid.push(b"0123456789abcdef"[usize::from(b >> 4)]);
            uuid.push(b"0123456789abcdef"[usize::from(b & 15)]);
        }
        let game_id = id(0, 0x9000_0000);
        Ok(Self {
            game_id,
            reporting_id: id(4, 0xa000_0000),
            host_persona: id(8, 0xb000_0000) as i64,
            host_uid: id(12, 0xc000_0000),
            host_connection: id(16, 0xd000_0000),
            shared_seed,
            clock,
            endpoint,
            uuid,
            name: format!("local-alldrive-{game_id:08x}").into_bytes(),
            external_name: format!("local-world-{game_id:08x}").into_bytes(),
        })
    }
    pub fn allocation(&self) -> Allocation<'_> {
        Allocation {
            game: self.game_id,
            reporting: self.reporting_id,
            host_persona: self.host_persona,
            host_session: self.host_uid,
            host_connection: self.host_connection,
            shared_seed: self.shared_seed,
            clock: self.clock,
            endpoint: self.endpoint,
            uuid: &self.uuid,
            name: &self.name,
            external_name: &self.external_name,
        }
    }
    pub fn validate(&self, current: &Current<'_>) -> Result<(), Error> {
        self.allocation().validate(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_words_have_separate_domains_and_uuid_bits() {
        let seed: [u8; SEED_BYTES] = std::array::from_fn(|i| i as u8 + 1);
        let g = Generated::from_seed(&seed, 123, "127.0.0.1:9000".parse().unwrap()).unwrap();
        assert_eq!(g.game_id, 0x94030201);
        assert_eq!(g.reporting_id, 0xa8070605);
        assert_eq!(g.host_persona, 0xbc0b0a09);
        assert_eq!(g.host_uid, 0xc00f0e0d);
        assert_eq!(g.host_connection, 0xd4131211);
        assert_eq!(g.shared_seed, 0x18171615);
        assert_eq!(&g.uuid, b"191a1b1c-1d1e-4f20-a122-232425262728");
        assert_eq!(g.allocation().name, b"local-alldrive-94030201");
        let mut other = seed;
        other[0] ^= 1;
        other[39] ^= 1;
        let h = Generated::from_seed(&other, 124, g.endpoint).unwrap();
        assert_ne!(g.game_id, h.game_id);
        assert_ne!(g.uuid, h.uuid);
    }
    #[test]
    fn entropy_clock_endpoint_and_shared_seed_are_bounded() {
        let endpoint = "127.0.0.1:9000".parse().unwrap();
        for n in 0..SEED_BYTES {
            assert!(Generated::from_seed(&vec![1; n], 1, endpoint).is_err());
        }
        assert!(Generated::from_seed(&[1; SEED_BYTES + 1], 1, endpoint).is_err());
        assert!(Generated::from_seed(&[0; SEED_BYTES], 1, endpoint).is_err());
        for time in [0, -1] {
            assert!(Generated::from_seed(&[1; SEED_BYTES], time, endpoint).is_err());
        }
        for endpoint in ["127.0.0.1:0", "192.0.2.1:9000", "[::1]:9000"] {
            assert!(Generated::from_seed(&[1; SEED_BYTES], 1, endpoint.parse().unwrap()).is_err());
        }
        let mut seed = [1; SEED_BYTES];
        seed[20..24].fill(0);
        assert!(Generated::from_seed(&seed, 1, endpoint).is_err());
    }
}
