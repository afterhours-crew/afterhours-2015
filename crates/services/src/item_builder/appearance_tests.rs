// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world_core::bits::BitWriter;

fn wire(method: u32, arguments: &BitWriter, references: &[u16]) -> BitWriter {
    let mut payload = BitWriter::new();
    payload
        .put(6, 9)
        .put(method.into(), 32)
        .put_span(arguments.span());
    let padding = (8 - payload.len() % 8) % 8;
    payload.put((1 << padding) - 1, padding);
    let mut body = BitWriter::new();
    body.put(0, 64).put(references.len() as u64, 8);
    for id in references {
        body.put((*id).into(), 13);
    }
    body.put(payload.bytes().len() as u64, 9)
        .put_span(payload.span());
    body
}

#[test]
fn plate_reads_ten_bit_byte_count_and_tint_reads_exact_float_bits() {
    check(7, &BitWriter::new(), Operation::StockPaint);
    for text in [
        b"".as_slice(),
        b"LOCAL",
        b"ABCDEFG",
        b"ABCDEFGH",
        b"AB\0C",
        &[255; 249],
    ] {
        let mut args = BitWriter::new();
        args.put(text.len() as u64, 10);
        for byte in text {
            args.put((*byte).into(), 8);
        }
        check(
            2,
            &args,
            Operation::LicensePlate {
                plate: crate::equipment::LicensePlate::new(text),
            },
        );
    }
    let binding = Binding::new(10, 6).unwrap();
    for value in [0f32, -0., 0.125, 0.9, 1.] {
        let mut args = BitWriter::new();
        args.put(value.to_bits().into(), 32);
        check(
            11,
            &args,
            Operation::WindowTint {
                tint: crate::equipment::WindowTint::new(value.to_bits()).unwrap(),
            },
        );
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
        let mut args = BitWriter::new();
        args.put(value.to_bits().into(), 32);
        assert_eq!(
            binding.read(wire(11, &args, &[10, 12]).span(), |_| true),
            Err(Error::Shape)
        );
    }
    for (declared, actual) in [(0, 1), (1, 0), (1023, 1), (250, 250)] {
        let mut args = BitWriter::new();
        args.put(declared, 10);
        for _ in 0..actual {
            args.put(65, 8);
        }
        assert!(
            binding
                .read(wire(2, &args, &[10, 12]).span(), |_| true)
                .is_err()
        );
    }
}
fn check(method: u32, arguments: &BitWriter, operation: Operation) {
    let binding = Binding::new(10, 6).unwrap();
    let body = wire(method, arguments, &[10, 12]);
    assert_eq!(
        binding.read(body.span(), |p| p == 12),
        Ok(Some(Call {
            player: 12,
            operation
        }))
    );
    for end in 0..body.len() {
        assert!(
            binding
                .read(body.span().slice(0, end).unwrap(), |_| true)
                .is_err()
        );
    }
    assert_eq!(binding.read(body.span(), |_| false), Err(Error::Ownership));
    assert_eq!(
        binding.read(wire(method, arguments, &[10, 12, 15]).span(), |_| true),
        Err(Error::Shape)
    );
    assert_eq!(
        binding.read(wire(method, arguments, &[10, 0]).span(), |_| true),
        Err(Error::Ownership)
    );
    assert_eq!(
        binding.read(wire(method, arguments, &[11, 12]).span(), |_| true),
        Ok(None)
    );
    let mut extra = arguments.clone();
    extra.put(0, 8);
    assert_eq!(
        binding.read(wire(method, &extra, &[10, 12]).span(), |_| true),
        Err(Error::Shape)
    );
    let mut concat = body.clone();
    concat.put_span(body.span());
    assert_eq!(binding.read(concat.span(), |_| true), Err(Error::Shape));
}
#[test]
fn paint_targets_have_two_float_triples_and_only_the_owned_player_reference() {
    let color = [0.125f32, 0.5, 0.75].map(f32::to_bits);
    let material = [1.0f32, 0.25, 0.0].map(f32::to_bits);
    for target in 0..=4 {
        let mut args = BitWriter::new();
        for word in color.into_iter().chain(material) {
            args.put(word.into(), 32);
        }
        args.put(target.into(), 8);
        check(
            1,
            &args,
            Operation::Paint {
                paint: crate::equipment::Paint::new(target, color, material).unwrap(),
            },
        );
    }
    let binding = Binding::new(10, 6).unwrap();
    for (target, word) in [
        (5, 0f32.to_bits()),
        (255, 0),
        (0, f32::NAN.to_bits()),
        (1, f32::INFINITY.to_bits()),
        (2, (-0.01f32).to_bits()),
        (4, 1.01f32.to_bits()),
    ] {
        for index in 0..6 {
            let mut args = BitWriter::new();
            for i in 0..6 {
                args.put(if i == index { word.into() } else { 0 }, 32);
            }
            args.put(target, 8);
            assert_eq!(
                binding.read(wire(1, &args, &[10, 12]).span(), |_| true),
                Err(Error::Shape)
            );
        }
    }
}
#[test]
fn rim_size_preserves_high_item_word_and_has_an_eight_bit_selection() {
    for size in [0, 4, 255] {
        let item = 0x12345678_abcdef01u64;
        let mut args = BitWriter::new();
        args.put(item as u32 as u64, 32)
            .put(item >> 32, 32)
            .put(size.into(), 8);
        check(10, &args, Operation::RimSize { item, size });
    }
    let mut args = BitWriter::new();
    args.put(0, 64).put(0, 8);
    assert_eq!(
        Binding::new(10, 6)
            .unwrap()
            .read(wire(10, &args, &[10, 12]).span(), |_| true),
        Err(Error::Shape)
    );
}

#[test]
fn livery_preserves_item_words_and_both_unsigned_fields() {
    let item = 0xfedcba98_12345678u64;
    for (data_id, preset) in [(0, 0), (3, u32::MAX), (u32::MAX, 19)] {
        let mut args = BitWriter::new();
        args.put(item as u32 as u64, 32)
            .put(item >> 32, 32)
            .put(data_id.into(), 32)
            .put(preset.into(), 32);
        check(
            9,
            &args,
            Operation::Livery {
                item,
                data_id,
                preset,
            },
        );
    }
    let mut args = BitWriter::new();
    args.put(0, 64).put(0, 64);
    assert_eq!(
        Binding::new(10, 6)
            .unwrap()
            .read(wire(9, &args, &[10, 12]).span(), |_| true),
        Err(Error::Shape)
    );
}
