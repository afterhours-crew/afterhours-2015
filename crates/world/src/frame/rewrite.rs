// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

/// Replace only client movement object IDs in a bounded frame. The shared
/// movement reader/writer establishes the section boundaries; time sync,
/// messages, replication and padding retain their exact bits.
///
/// File handlers are unsupported. A nonempty replication tail is preserved,
/// not validated here: callers must decode it with their connection's state
/// before using the result. This helper establishes neither object ownership
/// nor causal permission to send a report.
pub fn with_client_movement_ids(input: BitSpan<'_>, ids: &[u16]) -> Result<BitWriter, Error> {
    if input.len() > 65_536 || ids.len() > 63 {
        return Err(Error::Bound);
    }
    let mask = input.read_u32(0, 6)? as u8;
    if mask & (1 << MOVEMENT) == 0 || mask & 0b110000 != 0 {
        return Err(Error::Shape);
    }
    let parsed = parse(input, Direction::FromClient)?;
    // `rest` is the end of the time/message prefix. Ask the same movement
    // parser for its exact end instead of duplicating its width arithmetic.
    let mut reader = Reader {
        span: input,
        pos: parsed.rest,
    };
    let mut reports = movement(&mut reader, Direction::FromClient)?;
    if reports.records.len() != ids.len() {
        return Err(Error::Shape);
    }
    for (report, &id) in reports.records.iter_mut().zip(ids) {
        report.id = id;
    }
    let mut out = BitWriter::new();
    out.put_span(input.slice(0, parsed.rest)?);
    Builder::new().client_movement(reports).append(&mut out)?;
    out.put_span(input.after(reader.pos)?);
    if out.len() != input.len() {
        return Err(Error::Shape);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::{Initial, Record, Update, client_info, section::Section};

    fn mixed() -> BitWriter {
        let mut header = BitWriter::new();
        header.put(0x12345678, 32).put(0xabcdef01, 32).put(19, 32);
        let mut body = BitWriter::new();
        body.put(0x95, 8).put(3, 2);
        let builder = Builder::new()
            .messages(
                None,
                vec![Group {
                    channel: 0,
                    sequence: Some(120),
                    messages: vec![
                        Message::RecommendationSync(RecommendationSync {
                            persona: 42,
                            entries: vec![],
                        }),
                        Message::Sentinel(-1),
                    ],
                }],
            )
            .client_movement(Movement {
                header: Some(header.span()),
                records: vec![
                    MovementRecord {
                        id: 9,
                        component: 0,
                        data: body.span(),
                    },
                    MovementRecord {
                        id: 14,
                        component: 3,
                        data: body.span(),
                    },
                ],
            });
        let section = Section {
            float_bits: None,
            flag: false,
            deleted: vec![],
            setup: Some(crate::replication::section::Setup::ContextOnly),
            records: vec![Record {
                id: 1,
                initial: Some(Initial::ClientToServerInfo),
                update: Update::ClientToServerInfo(client_info::Update::Enabled(vec![Some(
                    client_info::Vector::Packed {
                        tag: 4,
                        components: [1, 2, 3],
                    },
                )])),
            }],
        }
        .encode()
        .unwrap();
        let mut frame = BitWriter::new();
        parse(builder.build().unwrap().span(), Direction::FromClient).expect("complete prefix");
        frame.put(u64::from(builder.mask() | 1 << GHOST | 1 << TIME_SYNC), 6);
        frame.put(0x123456789, 38); // Client TimeSync sample, not host's three times.
        builder.append(&mut frame).unwrap();
        frame.put_span(section.span());
        // Deliberately nonzero final padding must not be normalized away.
        while !frame.len().is_multiple_of(8) {
            frame.put(1, 1);
        }
        frame
    }

    #[test]
    fn movement_rewrite_preserves_mixed_handlers_unaligned_input_and_padding() {
        let source = mixed();
        let parsed = parse(source.span(), Direction::FromClient).unwrap();
        let records = &parsed.movement.as_ref().unwrap().records;
        let ranges: Vec<_> = records
            .iter()
            .map(|r| {
                // Independently assert the fixed-width object ID before the
                // component selector and declared body length in each record.
                let start = r.data.start() - 36;
                start..start + 16
            })
            .collect();
        let mut unaligned = BitWriter::new();
        unaligned.put(5, 3).put_span(source.span()).put(3, 2);
        let rewritten = with_client_movement_ids(
            unaligned.span().slice(3, source.len()).unwrap(),
            &[65535, 2],
        )
        .unwrap();
        assert_eq!(rewritten.len(), source.len());
        for i in 0..source.len() {
            if !ranges.iter().any(|range| range.contains(&i)) {
                assert_eq!(
                    source.span().read_u32(i, 1),
                    rewritten.span().read_u32(i, 1),
                    "bit {i}"
                );
            }
        }
        let after = parse(rewritten.span(), Direction::FromClient).unwrap();
        assert_eq!(
            after
                .movement
                .as_ref()
                .unwrap()
                .records
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            [65535, 2]
        );
        let mut state = client_info::State::new(1).unwrap();
        let ghost = after.ghost.unwrap();
        state
            .decode(rewritten.span().after(ghost.start).unwrap())
            .unwrap();
        assert!(state.get(1).unwrap().enabled);
        let same = with_client_movement_ids(source.span(), &[9, 14]).unwrap();
        assert_eq!(same, source);
    }

    #[test]
    fn movement_rewrite_rejects_missing_records_wrong_counts_and_input_bounds() {
        let source = mixed();
        assert!(with_client_movement_ids(source.span(), &[1]).is_err());
        assert!(with_client_movement_ids(source.span(), &[1; 64]).is_err());
        assert!(with_client_movement_ids(Builder::new().build().unwrap().span(), &[]).is_err());
        let end = parse(source.span(), Direction::FromClient)
            .unwrap()
            .ghost
            .unwrap()
            .start;
        for length in 0..end {
            assert!(
                with_client_movement_ids(source.span().slice(0, length).unwrap(), &[1, 2]).is_err()
            );
        }
        let mut too_large = source.clone();
        too_large.put_bytes(&[0; 8192]);
        assert_eq!(
            with_client_movement_ids(too_large.span(), &[1, 2]),
            Err(Error::Bound)
        );
        let mut file = BitWriter::new();
        file.put(0b010100, 6)
            .put_span(source.span().after(6).unwrap());
        assert_eq!(
            with_client_movement_ids(file.span(), &[1, 2]),
            Err(Error::Shape)
        );
    }
}
