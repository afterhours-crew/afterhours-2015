// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/// One complete channel-zero connection-handshake body. This does not admit a
/// peer or check direction, transport headers, tokens, cursors or session state.
/// Those checks belong to the owning handshake endpoints.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Control {
    Request { token: u32, sender: u32 },
    Reply { token: u32, sender: u32 },
    Confirm { token: u32 },
}

impl Control {
    /// Decode exactly one body; trailing bytes/words and unknown opcodes fail.
    /// No allocation or unbounded scan is performed.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if !matches!(bytes.len(), 8 | 12) {
            return None;
        }
        let (words, _) = bytes.as_chunks::<4>();
        match (u32::from_be_bytes(words[0]), words) {
            (1, [_, token, sender]) => Some(Self::Request {
                token: u32::from_be_bytes(*token),
                sender: u32::from_be_bytes(*sender),
            }),
            (5, [_, token, sender]) => Some(Self::Reply {
                token: u32::from_be_bytes(*token),
                sender: u32::from_be_bytes(*sender),
            }),
            (2, [_, token]) => Some(Self::Confirm {
                token: u32::from_be_bytes(*token),
            }),
            _ => None,
        }
    }

    pub fn encode(self) -> Vec<u8> {
        let (words, count) = match self {
            Self::Request { token, sender } => ([1, token, sender], 3),
            Self::Reply { token, sender } => ([5, token, sender], 3),
            Self::Confirm { token } => ([2, token, 0], 2),
        };
        words[..count]
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect()
    }
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(match self {
            Self::Request { .. } => "Request",
            Self::Reply { .. } => "Reply",
            Self::Confirm { .. } => "Confirm",
        })
        .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_shapes_preserve_words_and_reject_truncation_or_appended_data() {
        for (control, bytes) in [
            (
                Control::Request {
                    token: 0x01020304,
                    sender: 0x10203040,
                },
                vec![0, 0, 0, 1, 1, 2, 3, 4, 16, 32, 48, 64],
            ),
            (
                Control::Reply {
                    token: 0x01020304,
                    sender: 0x10203040,
                },
                vec![0, 0, 0, 5, 1, 2, 3, 4, 16, 32, 48, 64],
            ),
            (
                Control::Confirm { token: 0x01020304 },
                vec![0, 0, 0, 2, 1, 2, 3, 4],
            ),
        ] {
            assert_eq!(Control::decode(&bytes), Some(control));
            assert_eq!(control.encode(), bytes);
            for end in 0..bytes.len() {
                assert_eq!(Control::decode(&bytes[..end]), None);
            }
            for length in 1..=8 {
                let mut extra = bytes.clone();
                extra.extend(vec![0; length]);
                assert_eq!(Control::decode(&extra), None);
            }
            let mut unknown = bytes;
            unknown[3] = 3;
            assert_eq!(Control::decode(&unknown), None);
        }
        assert_eq!(Control::decode(&[0; 1024]), None);
    }
}
