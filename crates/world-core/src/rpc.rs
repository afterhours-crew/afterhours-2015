// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Lossless envelope encoding with caller-resolved object references.
//! This does not establish ownership, lifetime or method semantics.
use crate::bits::BitWriter;
use nfs_protocol::world::{Error, rpc::Envelope};

/// Replace only the references of a bounded, decoded type-47 body. The count
/// must stay unchanged: callers may not remove an argument or change dispatch
/// shape. Preserve both opaque words, every payload bit (including serials and
/// inline padding), and up to seven trailing alignment bits from a standalone
/// message. A following byte is not part of the envelope.
pub fn with_references(source: &Envelope<'_>, references: &[u16]) -> Result<BitWriter, Error> {
    if references.len() != source.references().len()
        || references.len() > u8::MAX as usize
        || references.iter().any(|id| *id >= 8192)
    {
        return Err(Error::ReferenceLimit);
    }
    if source.payload().len() > 256 * 8 || !source.payload().len().is_multiple_of(8) {
        return Err(Error::PayloadLimit);
    }
    if source.remaining().len() >= 8 {
        return Err(Error::InvalidBitRange);
    }
    let mut body = BitWriter::new();
    for word in source.words() {
        body.put(u64::from(word), 32);
    }
    body.put(references.len() as u64, 8);
    for id in references {
        body.put(u64::from(*id), 13);
    }
    body.put((source.payload().len() / 8) as u64, 9)
        .put_span(source.payload())
        .put_span(source.remaining());
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_protocol::world::{
        BitSpan,
        rpc::{Limits, RouteProfile},
    };

    const LIMITS: Limits = Limits {
        max_input_bits: 8192,
        max_references: 255,
        max_payload_bytes: 256,
    };

    fn source(references: &[u16], payload: &[u8], tail: usize) -> BitWriter {
        let mut bits = BitWriter::new();
        bits.put(0x12345678, 32)
            .put(0x87654321, 32)
            .put(references.len() as u64, 8);
        for id in references {
            bits.put((*id).into(), 13);
        }
        bits.put(payload.len() as u64, 9)
            .put_bytes(payload)
            .put(0x7f, tail);
        bits
    }
    fn bit_values(span: BitSpan<'_>) -> Vec<u32> {
        (0..span.len())
            .map(|i| span.read_u32(i, 1).unwrap())
            .collect()
    }

    #[test]
    fn rewriting_changes_only_references_in_both_directions_and_unaligned_bodies() {
        for profile in [RouteProfile::ClientSend, RouteProfile::ClientReceive] {
            let mut payload = BitWriter::new();
            payload.put(71, 9);
            if profile == RouteProfile::ClientReceive {
                payload.put(721, 10);
            }
            payload.put(0x12345678, 32).put(0x35, 6).align();
            for tail in 0..8 {
                let original = source(&[4, 29, 501], payload.bytes(), tail);
                let mut backing = BitWriter::new();
                backing.put(5, 3).put_span(original.span()).put(0xff, 8);
                let input = backing.span().slice(3, original.len()).unwrap();
                let decoded = Envelope::decode(input, LIMITS).unwrap();
                assert_eq!(
                    with_references(&decoded, decoded.references()).unwrap(),
                    original
                );
                let rewritten = with_references(&decoded, &[8191, 0, 31]).unwrap();
                assert_eq!(rewritten.len(), original.len());
                assert_eq!(rewritten.span().read_u32(72, 13), Ok(8191));
                assert_eq!(rewritten.span().read_u32(85, 13), Ok(0));
                assert_eq!(rewritten.span().read_u32(98, 13), Ok(31));
                assert_eq!(
                    bit_values(rewritten.span().slice(0, 72).unwrap()),
                    bit_values(input.slice(0, 72).unwrap())
                );
                assert_eq!(
                    bit_values(rewritten.span().after(111).unwrap()),
                    bit_values(input.after(111).unwrap())
                );
                let result = Envelope::decode(rewritten.span(), LIMITS).unwrap();
                let before = decoded.route(profile).unwrap();
                let after = result.route(profile).unwrap();
                assert_eq!(
                    (after.selector(), after.serial(), after.method_index()),
                    (before.selector(), before.serial(), before.method_index())
                );
                assert_eq!(
                    bit_values(after.arguments()),
                    bit_values(before.arguments())
                );
            }
        }
    }

    #[test]
    fn rewriting_rejects_reference_shape_changes_and_extra_bytes() {
        let original = source(&[1, 2], &[0; 6], 0);
        let decoded = Envelope::decode(original.span(), LIMITS).unwrap();
        for references in [&[1][..], &[1, 2, 3], &[1, 8192], &[u16::MAX, 2]] {
            assert_eq!(
                with_references(&decoded, references),
                Err(Error::ReferenceLimit)
            );
        }
        let trailing = source(&[1, 2], &[0; 6], 8);
        let decoded = Envelope::decode(trailing.span(), LIMITS).unwrap();
        assert_eq!(
            with_references(&decoded, &[1, 2]),
            Err(Error::InvalidBitRange)
        );
        for length in 0..original.len() {
            assert!(Envelope::decode(original.span().slice(0, length).unwrap(), LIMITS).is_err());
        }
        // The envelope codec preserves null/empty syntax; resolving it is the
        // caller's responsibility. Maximum reference and payload sizes fit.
        for (references, payload) in [(vec![], vec![]), (vec![8191; 255], vec![0xa5; 256])] {
            let original = source(&references, &payload, 0);
            let decoded = Envelope::decode(original.span(), LIMITS).unwrap();
            assert_eq!(with_references(&decoded, &references).unwrap(), original);
        }
    }
}
