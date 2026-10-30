// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unsequenced connection close: reason byte and a u16 byte-counted message.
//! The envelope has no kind-20 checksum. Reason values and message bytes stay
//! opaque; decoding does not identify or authenticate a connection.

use super::envelope::{self, ZeroTail};
use std::fmt;

pub const KIND: u8 = 14;
/// Local allocation policy, not a native wire limit.
pub const MAX_MESSAGE_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Envelope(envelope::Error),
    WrongKind,
    Truncated,
    MessageLimit,
    PayloadLength,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "application disconnect {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Eq, PartialEq)]
pub struct Disconnect {
    pub selector: u16,
    pub reason: u8,
    pub message: Vec<u8>,
    /// Four unused physical bits, retained without requiring zero.
    pub padding: u8,
}

impl fmt::Debug for Disconnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Disconnect")
            .field("selector", &self.selector)
            .field("reason", &self.reason)
            .field("message_bytes", &self.message.len())
            .finish_non_exhaustive()
    }
}

impl Disconnect {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let envelope = ZeroTail::new(MAX_MESSAGE_BYTES + 7)
            .expect("fixed nonzero limit")
            .decode(bytes)
            .map_err(Error::Envelope)?;
        if envelope.kind() != KIND {
            return Err(Error::WrongKind);
        }
        let payload = envelope.payload();
        let field = |offset, width| {
            payload
                .read_u32(offset, width)
                .map_err(|_| Error::Truncated)
        };
        let reason = field(0, 8)? as u8;
        let len = field(8, 16)? as usize;
        if len > MAX_MESSAGE_BYTES {
            return Err(Error::MessageLimit);
        }
        if payload.len() != 24 + len * 8 {
            return Err(Error::PayloadLength);
        }
        let message = (0..len)
            .map(|i| field(24 + i * 8, 8).map(|byte| byte as u8))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            selector: envelope.connection_selector(),
            reason,
            message,
            padding: bytes[bytes.len() - 1] & 15,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Constructed bit fields, independent of the decoder's offsets.
    fn wire(selector: u16, reason: u8, message: &[u8], padding: u8) -> Vec<u8> {
        let mut bits = format!(
            "100000{selector:014b}00001110{reason:08b}{:016b}",
            message.len()
        );
        for byte in message {
            bits.push_str(&format!("{byte:08b}"));
        }
        bits.push_str(&format!("{padding:04b}"));
        bits.as_bytes()
            .chunks(8)
            .map(|chunk| {
                chunk
                    .iter()
                    .fold(0, |byte, bit| (byte << 1) | u8::from(*bit == b'1'))
            })
            .collect()
    }

    #[test]
    fn reason_message_and_unused_bits_are_preserved() {
        for selector in [0, 4, 0x3fff] {
            for reason in [0, 7, 255] {
                for message in [&b""[..], &b"done\0\xff"[..], &[5; MAX_MESSAGE_BYTES][..]] {
                    for padding in [0, 15] {
                        let decoded =
                            Disconnect::decode(&wire(selector, reason, message, padding)).unwrap();
                        assert_eq!(
                            decoded,
                            Disconnect {
                                selector,
                                reason,
                                message: message.to_vec(),
                                padding
                            }
                        );
                        assert!(!format!("{decoded:?}").contains("done"));
                    }
                }
            }
        }
    }

    #[test]
    fn lengths_kind_transform_and_allocation_are_checked() {
        let valid = wire(4, 0, b"message", 0);
        for end in 0..valid.len() {
            assert!(Disconnect::decode(&valid[..end]).is_err());
        }
        let mut extended = valid.clone();
        extended.push(0);
        assert_eq!(Disconnect::decode(&extended), Err(Error::PayloadLength));
        let mut wrong = valid.clone();
        wrong[3] ^= 0x10;
        assert_eq!(Disconnect::decode(&wrong), Err(Error::WrongKind));
        let mut transformed = valid.clone();
        transformed[0] |= 4;
        assert!(matches!(
            Disconnect::decode(&transformed),
            Err(Error::Envelope(envelope::Error::UnsupportedTail))
        ));
        let mut too_long = wire(4, 0, &[], 0);
        too_long[4] |= 15;
        too_long[5] = 255;
        too_long[6] = 240;
        assert_eq!(Disconnect::decode(&too_long), Err(Error::MessageLimit));
        assert!(Disconnect::decode(&wire(4, 0, &[0; MAX_MESSAGE_BYTES + 1], 0)).is_err());
    }
}
