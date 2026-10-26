// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{BitWriter, Error, Reader};

pub const EX_PERFORMANCE_PROFILE: u8 = 67;

/// Client hardware report. Preserve native fields without interpreting them as
/// readiness, inventory state or authority to change server resource limits.
#[derive(Clone, Eq, PartialEq)]
pub struct PerformanceProfile {
    pub words: [u32; 3],
    pub quantities: [u64; 2],
    pub label: Vec<u8>,
    pub category: u8,
}

impl std::fmt::Debug for PerformanceProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerformanceProfile")
            .field("label_bytes", &self.label.len())
            .finish_non_exhaustive()
    }
}

impl PerformanceProfile {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let words = [r.take(32)?, r.take(32)?, r.take(32)?];
        let mut quantities = [0; 2];
        for value in &mut quantities {
            let low = r.take(32)?;
            *value = u64::from(low) | (u64::from(r.take(32)?) << 32);
        }
        let label = super::string(r)?;
        let category = r.take(4)? as u8;
        if category > 11 {
            return Err(Error::Shape);
        }
        Ok(Self {
            words,
            quantities,
            label,
            category,
        })
    }

    /// Body without the registry index, suitable for MessageEX chunking.
    pub fn encode(&self) -> Result<BitWriter, Error> {
        let mut w = BitWriter::new();
        self.write(&mut w)?;
        Ok(w)
    }

    pub(super) fn write(&self, w: &mut BitWriter) -> Result<(), Error> {
        if self.label.len() > super::MAX_STRING_BYTES {
            return Err(Error::Bound);
        }
        if self.category > 11 {
            return Err(Error::Shape);
        }
        for word in self.words {
            w.put(u64::from(word), 32);
        }
        for value in self.quantities {
            w.put(value & u64::from(u32::MAX), 32).put(value >> 32, 32);
        }
        w.put(self.label.len() as u64, 10);
        for &byte in &self.label {
            w.put(u64::from(byte), 8);
        }
        w.put(u64::from(self.category), 4);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{self, Message};

    fn sample() -> PerformanceProfile {
        PerformanceProfile {
            words: [0x01234567, 0x89abcdef, 0xfedcba98],
            quantities: [0x1122334455667788, 0x99aabbccddeeff00],
            label: b"ABC".to_vec(),
            category: 11,
        }
    }

    #[test]
    fn native_word_order_string_and_enum_match_independent_bytes() {
        let bytes = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x55, 0x66,
            0x77, 0x88, 0x11, 0x22, 0x33, 0x44, 0xdd, 0xee, 0xff, 0x00, 0x99, 0xaa, 0xbb, 0xcc,
            0x00, 0xd0, 0x50, 0x90, 0xec,
        ];
        let body = sample().encode().unwrap();
        assert_eq!(body.len(), 262);
        assert_eq!(body.bytes(), bytes);
        assert_eq!(
            frame::assembled_message(EX_PERFORMANCE_PROFILE, &bytes),
            Ok(Message::PerformanceProfile(sample()))
        );
        for len in 0..body.len() {
            assert!(
                PerformanceProfile::read(&mut Reader {
                    span: body.span().slice(0, len).unwrap(),
                    pos: 0
                })
                .is_err()
            );
        }
        let mut extra = bytes.to_vec();
        extra.push(0);
        assert!(frame::assembled_message(EX_PERFORMANCE_PROFILE, &extra).is_err());
        let mut invalid = bytes;
        invalid[32] = 0xf0; // category 12, retaining the final string bits.
        assert_eq!(
            frame::assembled_message(EX_PERFORMANCE_PROFILE, &invalid),
            Err(Error::Shape)
        );
        assert!(!format!("{:?}", sample()).contains("ABC"));
    }

    #[test]
    fn limits_and_following_message_alignment_are_checked() {
        let mut value = sample();
        value.label = vec![b'x'; super::super::MAX_STRING_BYTES];
        let body = value.encode().unwrap();
        assert_eq!(
            frame::assembled_message(EX_PERFORMANCE_PROFILE, body.bytes()),
            Ok(Message::PerformanceProfile(value.clone()))
        );
        value.label.push(b'x');
        assert_eq!(value.encode(), Err(Error::Bound));
        value.label.clear();
        value.category = 12;
        assert_eq!(value.encode(), Err(Error::Shape));
        let frame = frame::Builder::new()
            .messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(0),
                    messages: vec![Message::PerformanceProfile(sample()), Message::State(1)],
                }],
            )
            .build()
            .unwrap();
        let parsed = frame::parse(frame.span(), frame::Direction::FromClient).unwrap();
        assert!(parsed.complete);
        assert_eq!(
            parsed.messages.unwrap().groups[0].messages,
            vec![Message::PerformanceProfile(sample()), Message::State(1)]
        );
    }
}
