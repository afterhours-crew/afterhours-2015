// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The client's messageRead on a message manager: method 3 with the scene and
//! participant references, the asset reference (mode 2, 11-bit bundle,
//! 12-bit flat index) and the read flag, padded to a byte (client writer
//! `0x144456111`; official E1026 923.634 s reads flat 969 with flag 1).
//! Host calls use `game_mode::Arguments::Message`.
use crate::replication::Error;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

/// Asset-reference mode of a registry object (bundle and flat index follow).
const REGISTRY: u32 = 2;
const READ: u32 = 3;
const READ_BITS: usize = 2 + 11 + 12 + 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Read {
    pub manager: u16,
    pub selector: u16,
    pub participant: u16,
    pub bundle: u16,
    pub flat: u16,
    pub flag: bool,
}
impl Read {
    /// `Ok(None)` is "not a messageRead": another object, method or selector.
    pub fn decode(
        body: BitSpan<'_>,
        manager_scene: u16,
        selectors: &[u16],
    ) -> Result<Option<Self>, Error> {
        // Classify with the general client RPC bounds: every other call on
        // the manager's scene passes through here first (E1248 replay: a
        // 103-bit gameplay s194 m2 report was refused under message bounds).
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 8192,
                max_references: 255,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if route.method_index() != READ
            || !selectors.contains(&route.selector())
            || envelope.references().first() != Some(&manager_scene)
        {
            return Ok(None);
        }
        let &[manager, participant] = envelope.references() else {
            return Err(Error::Shape);
        };
        let bits = route.arguments();
        // The arguments end at the payload's byte boundary.
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || !(READ_BITS..READ_BITS + 8).contains(&bits.len())
        {
            return Err(Error::Shape);
        }
        if manager == 0 || participant == 0 {
            return Err(Error::UnknownObject);
        }
        let read = |offset, width| bits.read_u32(offset, width).map_err(|_| Error::Shape);
        if read(0, 2)? != REGISTRY {
            return Err(Error::Shape);
        }
        Ok(Some(Self {
            manager,
            selector: route.selector(),
            participant,
            bundle: read(2, 11)? as u16,
            flat: read(13, 12)? as u16,
            flag: read(25, 1)? == 1,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bits::BitWriter,
        game_mode::{Arguments, Call},
    };
    use nfs_protocol::world::rpc::Serial;
    fn read_body(selector: u16, method: u32, args: &str, refs: &[u16]) -> BitWriter {
        let mut payload = BitWriter::new();
        payload.put(selector.into(), 9).put(method.into(), 32);
        for c in args.chars() {
            payload.put(u64::from(c == '1'), 1);
        }
        payload.align();
        let mut body = BitWriter::new();
        body.put(0, 32).put(0, 32).put(refs.len() as u64, 8);
        for &r in refs {
            body.put(r.into(), 13);
        }
        body.put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body
    }
    #[test]
    fn official_message_read_decodes_the_reminder_and_rejects_other_shapes() {
        // E1026 923.634: client s106 m3, mode 2, bundle 1, flat 969, read.
        let official = "1000000000001001111001001".to_owned() + "1" + "10000";
        let body = read_body(106, 3, &official, &[240, 454]);
        let read = Read::decode(body.span(), 240, &[106, 107])
            .unwrap()
            .unwrap();
        assert_eq!(
            read,
            Read {
                manager: 240,
                selector: 106,
                participant: 454,
                bundle: 1,
                flat: 969,
                flag: true,
            }
        );
        assert!(
            Read::decode(
                read_body(105, 3, &official, &[240, 454]).span(),
                240,
                &[106, 107]
            )
            .unwrap()
            .is_none()
        );
        assert!(
            Read::decode(
                read_body(106, 2, &official, &[240, 454]).span(),
                240,
                &[106]
            )
            .unwrap()
            .is_none()
        );
        // Another scene's selector-106 method 3 (a volume report) is not ours.
        assert!(
            Read::decode(
                read_body(106, 3, &"0".repeat(103), &[18, 454]).span(),
                240,
                &[106]
            )
            .unwrap()
            .is_none()
        );
        // Nor is a larger report on the manager's own scene (E1248 replay,
        // gameplay s194 m2 with 103 argument bits), at any payload size.
        for bits in [103, 900] {
            assert!(
                Read::decode(
                    read_body(194, 2, &"1".repeat(bits), &[240, 454]).span(),
                    240,
                    &[106]
                )
                .unwrap()
                .is_none()
            );
        }
        // A manager read with an oversized payload is malformed.
        assert!(
            Read::decode(
                read_body(106, 3, &"1".repeat(300), &[240, 454]).span(),
                240,
                &[106]
            )
            .is_err()
        );
        for bad in [
            read_body(106, 3, &official, &[240]),
            read_body(106, 3, &official[..20], &[240, 454]),
            read_body(106, 3, &("0".to_owned() + &official[1..]), &[240, 454]),
            read_body(106, 3, &official, &[240, 0]),
        ] {
            assert!(Read::decode(bad.span(), 240, &[106]).is_err());
        }
    }
    #[test]
    fn message_added_encodes_the_official_reference_bits() {
        let call = Call {
            target: 240,
            selector: 106,
            serial: Serial::new(48).unwrap(),
            method: 2,
            reference: None,
            arguments: Arguments::Message {
                bundle: 1,
                flat: 969,
                flag: false,
            },
        };
        let body = call.encode().unwrap();
        let bytes = body.bytes();
        // 64 envelope words, 8-bit count, one 13-bit reference, 9-bit length.
        let span = BitSpan::new(bytes, 0, bytes.len() * 8).unwrap();
        let payload = 64 + 8 + 13 + 9;
        let args = payload + 9 + 10 + 32;
        let official = "1000000000001001111001001000000000";
        for (i, c) in official.chars().enumerate() {
            assert_eq!(
                span.read_u32(args + i, 1).unwrap(),
                u32::from(c == '1'),
                "bit {i}"
            );
        }
        assert_eq!(span.read_u32(payload - 9, 9).unwrap(), 11, "payload bytes");
        for (bad, arguments) in [
            (
                2,
                Arguments::Message {
                    bundle: 0,
                    flat: 1,
                    flag: false,
                },
            ),
            (
                2,
                Arguments::Message {
                    bundle: 1,
                    flat: 4096,
                    flag: false,
                },
            ),
            (
                1,
                Arguments::Message {
                    bundle: 1,
                    flat: 1,
                    flag: false,
                },
            ),
        ] {
            assert!(
                Call {
                    method: bad,
                    arguments,
                    ..call
                }
                .encode()
                .is_err()
            );
        }
    }
}
