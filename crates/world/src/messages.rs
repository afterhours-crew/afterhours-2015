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

/// One `CarPartAttachmentAsset` payload. The identifier is the installed
/// `ItemData.Id`, not an inventory instance ID, definition GUID or message hash.
/// Only the version-zero, single-attachment representation is supported here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CarPartAttachment {
    pub item_data_id: u32,
}
impl CarPartAttachment {
    pub fn encode(self) -> [u8; 5] {
        let id = self.item_data_id.to_be_bytes();
        [0, id[0], id[1], id[2], id[3]]
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let [0, a, b, c, d] = bytes else {
            return Err(Error::Shape);
        };
        Ok(Self {
            item_data_id: u32::from_be_bytes([*a, *b, *c, *d]),
        })
    }
}

/// Message reference and read flag sent by a client. Trailing bits are payload
/// alignment, not additional flags; their values are deliberately unconstrained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadArguments {
    pub bundle: u16,
    pub flat: u16,
    pub flag: bool,
}
impl ReadArguments {
    pub fn decode(bits: BitSpan<'_>) -> Result<Self, Error> {
        if !(READ_BITS..READ_BITS + 8).contains(&bits.len()) {
            return Err(Error::Shape);
        }
        let (bundle, flat) = reference(bits)?;
        Ok(Self {
            bundle,
            flat,
            flag: bits.read_u32(25, 1).map_err(|_| Error::Shape)? == 1,
        })
    }
}

/// Arguments of host messageRestored/messageAdded. Attachments are bounded by
/// the wire's byte length and preserved opaquely; this does not interpret them
/// or establish that a client executed the delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveryArguments {
    pub bundle: u16,
    pub flat: u16,
    pub attachment: Vec<u8>,
    pub flag: bool,
}
impl DeliveryArguments {
    pub fn decode(bits: BitSpan<'_>) -> Result<Self, Error> {
        // Host selector/serial/method occupy 51 bits. The reference, length,
        // attachment and flag occupy 34 + 8*n bits, followed by 3 padding bits.
        if bits.len() < 37 || bits.len() > 37 + 8 * 255 {
            return Err(Error::Shape);
        }
        let (bundle, flat) = reference(bits)?;
        let length = bits.read_u32(25, 8).map_err(|_| Error::Shape)? as usize;
        if bits.len() != 37 + length * 8 {
            return Err(Error::Shape);
        }
        let attachment = (0..length)
            .map(|i| bits.read_u32(33 + i * 8, 8).map(|v| v as u8))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| Error::Shape)?;
        Ok(Self {
            bundle,
            flat,
            attachment,
            flag: bits
                .read_u32(33 + length * 8, 1)
                .map_err(|_| Error::Shape)?
                == 1,
        })
    }
}

fn reference(bits: BitSpan<'_>) -> Result<(u16, u16), Error> {
    let read = |offset, width| bits.read_u32(offset, width).map_err(|_| Error::Shape);
    if read(0, 2)? != REGISTRY {
        return Err(Error::Shape);
    }
    Ok((read(2, 11)? as u16, read(13, 12)? as u16))
}

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
        let arguments = ReadArguments::decode(bits)?;
        Ok(Some(Self {
            manager,
            selector: route.selector(),
            participant,
            bundle: arguments.bundle,
            flat: arguments.flat,
            flag: arguments.flag,
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

    #[test]
    fn car_part_attachment_uses_versioned_network_order_item_id() {
        let value = CarPartAttachment {
            item_data_id: 0x12ab_34cd,
        };
        // Constructed vector with four different bytes catches a symmetric
        // encoder/decoder endian mistake; this is not a captured part ID.
        assert_eq!(value.encode(), [0, 0x12, 0xab, 0x34, 0xcd]);
        assert_eq!(CarPartAttachment::decode(&value.encode()).unwrap(), value);
        assert_ne!(
            CarPartAttachment::decode(&[0, 0xcd, 0x34, 0xab, 0x12]).unwrap(),
            value
        );
        for id in [0, u32::MAX] {
            let edge = CarPartAttachment { item_data_id: id };
            assert_eq!(CarPartAttachment::decode(&edge.encode()).unwrap(), edge);
        }
    }

    #[test]
    fn car_part_attachment_rejects_unknown_versions_and_incomplete_or_extra_data() {
        let bytes = [0, 0x12, 0xab, 0x34, 0xcd];
        for end in 0..bytes.len() {
            assert!(CarPartAttachment::decode(&bytes[..end]).is_err());
        }
        assert!(CarPartAttachment::decode(&[0, 0x12, 0xab, 0x34, 0xcd, 0]).is_err());
        for version in 1..=u8::MAX {
            let mut invalid = bytes;
            invalid[0] = version;
            assert!(CarPartAttachment::decode(&invalid).is_err());
        }
    }
    use nfs_protocol::world::rpc::Serial;

    #[test]
    fn part_added_and_restored_calls_preserve_identity_read_state_and_attachment() {
        let part = CarPartAttachment {
            item_data_id: 0x12ab_34cd,
        };
        for method in [0, 2] {
            for flag in [false, true] {
                let call = Call {
                    target: 42,
                    selector: 9,
                    serial: Serial::new(3).unwrap(),
                    method,
                    reference: None,
                    arguments: Arguments::CarPartMessage {
                        bundle: 17,
                        flat: 2049,
                        flag,
                        part,
                    },
                };
                let encoded = call.encode().unwrap();
                let envelope = Envelope::decode(
                    encoded.span(),
                    Limits {
                        max_input_bits: 512,
                        max_references: 1,
                        max_payload_bytes: 32,
                    },
                )
                .unwrap();
                assert_eq!(envelope.references(), &[42]);
                let route = envelope.route(RouteProfile::ClientReceive).unwrap();
                assert_eq!(route.method_index(), method);
                let delivered = DeliveryArguments::decode(route.arguments()).unwrap();
                assert_eq!(
                    (delivered.bundle, delivered.flat, delivered.flag),
                    (17, 2049, flag)
                );
                assert_eq!(delivered.attachment, [0, 0x12, 0xab, 0x34, 0xcd]);
                assert_eq!(
                    CarPartAttachment::decode(&delivered.attachment).unwrap(),
                    part
                );
                for invalid in [
                    Call { method: 1, ..call },
                    Call {
                        reference: Some(43),
                        ..call
                    },
                    Call {
                        arguments: Arguments::CarPartMessage {
                            bundle: 0,
                            flat: 2049,
                            flag,
                            part,
                        },
                        ..call
                    },
                    Call {
                        arguments: Arguments::CarPartMessage {
                            bundle: 2048,
                            flat: 2049,
                            flag,
                            part,
                        },
                        ..call
                    },
                    Call {
                        arguments: Arguments::CarPartMessage {
                            bundle: 17,
                            flat: 4096,
                            flag,
                            part,
                        },
                        ..call
                    },
                ] {
                    assert!(invalid.encode().is_err());
                }
            }
        }
    }

    fn delivery_bits(length: u8, attachment: &[u8], flag: bool, padding: u8) -> BitWriter {
        let mut bits = BitWriter::new();
        bits.put(2, 2)
            .put(17, 11)
            .put(2049, 12)
            .put(length.into(), 8);
        for byte in attachment {
            bits.put((*byte).into(), 8);
        }
        bits.put(u64::from(flag), 1).put(padding.into(), 3);
        bits
    }

    #[test]
    fn deliveries_preserve_opaque_attachments_and_ignore_only_alignment() {
        for attachment in [vec![], vec![0, 255, 7, 128, 1], vec![219; 255]] {
            for flag in [false, true] {
                for padding in [0, 7] {
                    let bits = delivery_bits(attachment.len() as u8, &attachment, flag, padding);
                    let decoded = DeliveryArguments::decode(bits.span()).unwrap();
                    assert_eq!(
                        (decoded.bundle, decoded.flat, decoded.flag),
                        (17, 2049, flag)
                    );
                    assert_eq!(decoded.attachment, attachment);
                }
            }
        }
    }

    #[test]
    fn delivery_lengths_and_registry_mode_fail_closed() {
        let valid = delivery_bits(5, &[1, 2, 3, 4, 5], true, 7);
        for length in 0..valid.span().len() {
            let short = BitSpan::new(valid.bytes(), 0, length).unwrap();
            assert!(DeliveryArguments::decode(short).is_err(), "length {length}");
        }
        for (declared, actual) in [(0, 1), (1, 0), (254, 255), (255, 254)] {
            assert!(
                DeliveryArguments::decode(
                    delivery_bits(declared, &vec![3; actual], false, 0).span()
                )
                .is_err()
            );
        }
        let mut wrong_mode = valid.bytes().to_vec();
        wrong_mode[0] &= 0x3f;
        assert!(
            DeliveryArguments::decode(BitSpan::new(&wrong_mode, 0, valid.span().len()).unwrap())
                .is_err()
        );
        let mut trailing = valid.clone();
        trailing.put(0, 1);
        assert!(DeliveryArguments::decode(trailing.span()).is_err());
    }

    #[test]
    fn read_payload_keeps_flag_separate_from_nonzero_padding() {
        for flag in [false, true] {
            let mut bits = BitWriter::new();
            bits.put(2, 2)
                .put(17, 11)
                .put(2049, 12)
                .put(u64::from(flag), 1)
                .put(31, 5);
            assert_eq!(
                ReadArguments::decode(bits.span()).unwrap(),
                ReadArguments {
                    bundle: 17,
                    flat: 2049,
                    flag
                }
            );
        }
    }
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
