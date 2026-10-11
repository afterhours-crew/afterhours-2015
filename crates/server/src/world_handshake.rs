// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;

const TEMPLATE_HASH: [u8; 32] = [
    0xe2, 0xaf, 0xaa, 0x1c, 0x47, 0x73, 0xce, 0xbd, 0x81, 0xb0, 0x66, 0x56, 0x35, 0xd1, 0xc7, 0x9b,
    0xce, 0x02, 0x17, 0x92, 0xee, 0xc7, 0xdb, 0xea, 0x41, 0xf0, 0xd5, 0xb5, 0x34, 0x4d, 0x2d, 0x2b,
];
pub fn validate_template(bytes: &[u8]) -> Result<[u8; 64], Failure> {
    if bytes.len() != 64 || openssl::sha::sha256(bytes) != TEMPLATE_HASH {
        return Err(Failure::ProfileConfig);
    }
    bytes.try_into().map_err(|_| Failure::ProfileConfig)
}
pub struct Binding {
    key: [u8; 110],
    client: u32,
    host: u32,
    readiness: Option<crate::world_readiness::Binding>,
}
impl Binding {
    pub fn from_current(
        world_uuid: &[u8],
        local_uuid: &[u8],
        client: u64,
        host: u64,
    ) -> Result<Self, Failure> {
        fn uuid(bytes: &[u8]) -> bool {
            bytes.len() == 36
                && bytes.iter().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        *b == b'-'
                    } else {
                        b.is_ascii_digit() || (b'a'..=b'f').contains(b)
                    }
                })
        }
        let client = client as u32;
        let host = host as u32;
        if !uuid(world_uuid)
            || !uuid(local_uuid)
            || world_uuid == local_uuid
            || client == 0
            || host == 0
            || client == host
        {
            return Err(Failure::ProfileConfig);
        }
        let (first, second) = if world_uuid < local_uuid {
            (world_uuid, local_uuid)
        } else {
            (local_uuid, world_uuid)
        };
        let mut key = [0; 110];
        key[..36].copy_from_slice(world_uuid);
        key[36] = b'-';
        key[37..73].copy_from_slice(first);
        key[73] = b'-';
        key[74..].copy_from_slice(second);
        Ok(Self {
            key,
            client,
            host,
            readiness: None,
        })
    }
    /// Attach a readiness gate only when it belongs to these connection IDs.
    pub fn with_readiness(
        mut self,
        binding: crate::world_readiness::Binding,
    ) -> Result<Self, Failure> {
        if binding.local_connection as u32 != self.client
            || binding.host_connection as u32 != self.host
        {
            return Err(Failure::ProfileConfig);
        }
        self.readiness = Some(binding);
        Ok(self)
    }
    pub fn readiness(&self) -> Option<crate::world_readiness::Binding> {
        self.readiness
    }
    pub fn key(&self) -> [u8; 110] {
        self.key
    }
    pub fn client(&self) -> u32 {
        self.client
    }
    pub fn host(&self) -> u32 {
        self.host
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_order_is_lexical_and_low_word_collisions_fail_closed() {
        let a = b"11111111-1111-1111-1111-111111111111";
        let b = b"eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
        for (world, local) in [(a, b), (b, a)] {
            let binding = Binding::from_current(world, local, 7, 9).unwrap();
            assert_eq!(&binding.key[..36], world);
            assert_eq!(&binding.key[37..73], a);
            assert_eq!(&binding.key[74..], b);
        }
        assert!(Binding::from_current(a, b, 7, (1u64 << 32) + 7).is_err());
        assert!(Binding::from_current(a, b, 1u64 << 32, 9).is_err());
        assert!(Binding::from_current(a, a, 7, 9).is_err());
        assert!(Binding::from_current(&[b'x'; 36], b, 7, 9).is_err());
        assert!(validate_template(&[0; 64]).is_err());
        assert!(validate_template(&[0; 65]).is_err());
    }
}
