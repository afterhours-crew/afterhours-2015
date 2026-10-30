// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Garage livery-cache notifications. The editor reports a persona and cache
//! key; the host invalidates that key in client caches. This does not upload,
//! persist or apply a wrap. The originating client ignores its own invalidation.

use crate::{bits::BitWriter, participants::Endpoint, replication::Error};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

pub const COMPONENT: usize = 8;
/// Local queue/storage policy, below the native ten-bit string wire bound.
pub const MAX_KEY_BYTES: usize = 128;

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Key {
    bytes: [u8; MAX_KEY_BYTES],
    len: u8,
}
impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveryCacheKey")
            .field("bytes", &self.len)
            .finish()
    }
}
impl Key {
    pub fn new(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_KEY_BYTES {
            return Err(Error::Bound);
        }
        let mut value = Self {
            bytes: [0; MAX_KEY_BYTES],
            len: bytes.len() as u8,
        };
        value.bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(value)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Invalidation {
    pub endpoint: Endpoint,
    pub persona: u64,
    pub key: Key,
}
impl Invalidation {
    pub fn encode(self) -> Result<BitWriter, Error> {
        if self.endpoint.scene == 0
            || self.endpoint.scene > 8191
            || self.endpoint.selector > 511
            || self.persona == 0
        {
            return Err(Error::Bound);
        }
        let mut payload = BitWriter::new();
        payload
            .put(self.endpoint.selector.into(), 9)
            .put(self.endpoint.serial.value().into(), 10)
            .put(1, 32)
            .put(self.persona & 0xffff_ffff, 32)
            .put(self.persona >> 32, 32)
            .put(u64::from(self.key.len), 10);
        for byte in self.key.bytes() {
            payload.put(u64::from(*byte), 8);
        }
        payload.align();
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(1, 8)
            .put(self.endpoint.scene.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        Ok(body)
    }
}

/// `endpoint` is resolved from the current owned garage's published component.
/// The caller qualifies the persona against this connection's participants.
pub fn receive(
    endpoint: Endpoint,
    body: BitSpan<'_>,
    owns: impl FnOnce(u64) -> bool,
) -> Result<Option<Invalidation>, Error> {
    let envelope = Envelope::decode(
        body,
        Limits {
            max_input_bits: 8192,
            max_references: 32,
            max_payload_bytes: 511,
        },
    )
    .map_err(|_| Error::Shape)?;
    let route = envelope
        .route(RouteProfile::ClientSend)
        .map_err(|_| Error::Shape)?;
    if envelope.references().first() != Some(&endpoint.scene)
        || route.selector() != endpoint.selector
        || route.method_index() != 0
    {
        return Ok(None);
    }
    if envelope.references().len() != 1
        || envelope.words() != [0, 0]
        || !envelope.remaining().is_empty()
    {
        return Err(Error::Shape);
    }
    let arguments = route.arguments();
    let read = |offset, width| arguments.read_u32(offset, width).map_err(|_| Error::Shape);
    let persona = u64::from(read(0, 32)?) | (u64::from(read(32, 32)?) << 32);
    let len = read(64, 10)? as usize;
    if len > MAX_KEY_BYTES {
        return Err(Error::Bound);
    }
    // Five final bits are physical byte padding, including nonzero values.
    if arguments.len() != 79 + len * 8 {
        return Err(Error::Shape);
    }
    if persona == 0 || !owns(persona) {
        return Err(Error::UnknownObject);
    }
    let mut key = Key {
        bytes: [0; MAX_KEY_BYTES],
        len: len as u8,
    };
    for i in 0..len {
        key.bytes[i] = read(74 + i * 8, 8)? as u8;
    }
    Ok(Some(Invalidation {
        endpoint,
        persona,
        key,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_protocol::world::rpc::Serial;
    const PERSONA: u64 = 0x1234_5678_2345_6789;
    fn endpoint() -> Endpoint {
        Endpoint {
            scene: 27,
            selector: 5,
            serial: Serial::new(7).unwrap(),
        }
    }
    fn request(persona: u64, key: &[u8], pad: u8) -> BitWriter {
        let mut payload = BitWriter::new();
        payload
            .put(5, 9)
            .put(0, 32)
            .put(persona & 0xffff_ffff, 32)
            .put(persona >> 32, 32)
            .put(key.len() as u64, 10);
        for byte in key {
            payload.put(u64::from(*byte), 8);
        }
        payload.put(pad.into(), 5);
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put(1, 8)
            .put(27, 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body
    }
    #[test]
    fn owned_updates_and_repeats_create_typed_invalidations() {
        for key in [
            &b"BakedLivery_0X0123456789ABCDEF"[..],
            &b""[..],
            &[0xff; MAX_KEY_BYTES][..],
        ] {
            for pad in [0, 1, 5, 31] {
                let input = request(PERSONA, key, pad);
                let result = receive(endpoint(), input.span(), |p| p == PERSONA)
                    .unwrap()
                    .unwrap();
                assert_eq!(result.persona, PERSONA);
                assert_eq!(result.key.bytes(), key);
                let output = result.encode().unwrap();
                let e = Envelope::decode(
                    output.span(),
                    Limits {
                        max_input_bits: 8192,
                        max_references: 1,
                        max_payload_bytes: 511,
                    },
                )
                .unwrap();
                let r = e.route(RouteProfile::ClientReceive).unwrap();
                assert_eq!(e.references(), &[27]);
                assert_eq!(
                    (r.selector(), r.serial(), r.method_index()),
                    (5, Some(endpoint().serial), 1)
                );
                let a = r.arguments();
                assert_eq!(a.len(), 77 + key.len() * 8);
                assert_eq!(a.read_u32(0, 32).unwrap(), PERSONA as u32);
                assert_eq!(a.read_u32(32, 32).unwrap(), (PERSONA >> 32) as u32);
                assert_eq!(a.read_u32(64, 10).unwrap() as usize, key.len());
                for (i, byte) in key.iter().enumerate() {
                    assert_eq!(a.read_u32(74 + i * 8, 8).unwrap(), u32::from(*byte));
                }
            }
        }
    }
    #[test]
    fn ownership_lengths_and_current_endpoint_are_required() {
        let input = request(PERSONA, b"key", 0);
        assert_eq!(
            receive(endpoint(), input.span(), |_| false),
            Err(Error::UnknownObject)
        );
        assert_eq!(
            receive(
                Endpoint {
                    scene: 28,
                    ..endpoint()
                },
                input.span(),
                |_| true
            ),
            Ok(None)
        );
        assert_eq!(
            receive(
                Endpoint {
                    selector: 6,
                    ..endpoint()
                },
                input.span(),
                |_| true
            ),
            Ok(None)
        );
        let zero = request(0, b"key", 0);
        assert_eq!(
            receive(endpoint(), zero.span(), |_| true),
            Err(Error::UnknownObject)
        );
        let big = request(PERSONA, &[1; MAX_KEY_BYTES + 1], 0);
        assert_eq!(receive(endpoint(), big.span(), |_| true), Err(Error::Bound));
        for bits in 0..input.len() {
            assert!(
                receive(
                    endpoint(),
                    BitSpan::new(input.bytes(), 0, bits).unwrap(),
                    |_| true
                )
                .is_err()
            );
        }
        let mut extra = input.clone();
        extra.put(0, 8);
        assert_eq!(
            receive(endpoint(), extra.span(), |_| true),
            Err(Error::Shape)
        );
    }
}
