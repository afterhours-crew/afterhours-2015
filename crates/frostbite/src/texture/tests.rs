// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::synthetic::{bc1_solid, texture_header};

const RED: u16 = 0xF800;
const BLUE: u16 = 0x001F;

/// A width x height BC1 texture whose left block column is red, the rest blue.
fn bc1(width: u16, height: u16) -> (TextureHeader, Vec<u8>) {
    let (bw, bh) = (
        usize::from(width).div_ceil(4),
        usize::from(height).div_ceil(4),
    );
    let mut data = Vec::new();
    for _ in 0..bh {
        for x in 0..bw {
            data.extend_from_slice(&bc1_solid(if x == 0 { RED } else { BLUE }));
        }
    }
    let header = texture_header(54, width, height, [9; 16], data.len() as u32);
    (TextureHeader::parse(&header).unwrap(), data)
}

#[test]
fn headers_parse_every_field() {
    let bytes = texture_header(60, 512, 256, [7; 16], 131_072);
    let header = TextureHeader::parse(&bytes).unwrap();
    assert_eq!(
        (
            header.kind,
            header.format,
            header.width,
            header.height,
            header.depth,
            header.slices
        ),
        (0, 60, 512, 256, 1, 1)
    );
    assert_eq!((header.mip_count, header.first_mip), (1, 0));
    assert_eq!(header.chunk, [7; 16]);
    assert_eq!((header.mip_sizes[0], header.chunk_size), (131_072, 131_072));
    assert_eq!(header.group, "UI");
    assert!(TextureHeader::parse(&bytes[..127]).is_err());
    assert!(TextureHeader::parse(&[bytes.clone(), vec![0]].concat()).is_err());
}

#[test]
fn bc1_decodes_to_opaque_rgba_with_partial_blocks_cropped() {
    let (header, data) = bc1(8, 4);
    let image = decode(&header, &data, &Limits::default()).unwrap();
    assert_eq!(
        (image.width, image.height, image.rgba.len()),
        (8, 4, 8 * 4 * 4)
    );
    assert_eq!(&image.rgba[..4], [255, 0, 0, 255]);
    assert_eq!(&image.rgba[4 * 4..4 * 4 + 4], [0, 0, 255, 255]);
    // 6x5 texels use 2x2 blocks; the padding is cropped away.
    let (header, data) = bc1(6, 5);
    let image = decode(&header, &data, &Limits::default()).unwrap();
    assert_eq!(
        (image.width, image.height, image.rgba.len()),
        (6, 5, 6 * 5 * 4)
    );
    assert_eq!(&image.rgba[(4 * 6 + 5) * 4..], [0, 0, 255, 255]);
    // A transparent BC1 texel is opaque black without the alpha format.
    let mut block = [0u8; 8];
    block[..2].copy_from_slice(&BLUE.to_le_bytes());
    block[2..4].copy_from_slice(&RED.to_le_bytes());
    block[4..].copy_from_slice(&[0xFF; 4]);
    let header = TextureHeader::parse(&texture_header(54, 4, 4, [1; 16], 8)).unwrap();
    assert_eq!(
        &decode(&header, &block, &Limits::default()).unwrap().rgba[..4],
        [0, 0, 0, 255]
    );
    let header = TextureHeader::parse(&texture_header(56, 4, 4, [1; 16], 8)).unwrap();
    assert_eq!(
        &decode(&header, &block, &Limits::default()).unwrap().rgba[..4],
        [0, 0, 0, 0]
    );
}

#[test]
fn bc3_and_uncompressed_formats_decode() {
    // BC3: alpha endpoints 255/0 with every alpha index 1 (value 0), colour red.
    let mut block = vec![255, 0, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24];
    block.extend_from_slice(&bc1_solid(RED));
    let header = TextureHeader::parse(&texture_header(60, 4, 4, [1; 16], 16)).unwrap();
    let image = decode(&header, &block, &Limits::default()).unwrap();
    assert_eq!(&image.rgba[..4], [255, 0, 0, 0]);
    let pixels = [1, 2, 3, 4, 5, 6, 7, 8];
    let header = TextureHeader::parse(&texture_header(18, 2, 1, [1; 16], 8)).unwrap();
    assert_eq!(
        decode(&header, &pixels, &Limits::default()).unwrap().rgba,
        pixels
    );
    let header = TextureHeader::parse(&texture_header(23, 2, 1, [1; 16], 8)).unwrap();
    assert_eq!(
        decode(&header, &pixels, &Limits::default()).unwrap().rgba,
        [3, 2, 1, 4, 7, 6, 5, 8]
    );
}

#[test]
fn unsupported_shapes_formats_and_sizes_are_refused() {
    let (header, data) = bc1(8, 8);
    let limits = Limits::default();
    let refused = |change: &dyn Fn(&mut TextureHeader)| {
        let mut changed = header.clone();
        change(&mut changed);
        decode(&changed, &data, &limits).is_err()
    };
    assert!(refused(&|h| h.format = 62), "BC4 is not decoded");
    assert!(refused(&|h| h.format = 0));
    assert!(refused(&|h| h.kind = 1), "cube map");
    assert!(refused(&|h| h.depth = 2));
    assert!(refused(&|h| h.slices = 6));
    assert!(refused(&|h| h.first_mip = 1), "first mip streamed out");
    assert!(refused(&|h| h.mip_count = 0));
    assert!(refused(&|h| h.mip_offsets[0] = 16));
    assert!(refused(&|h| h.width = 0));
    assert!(refused(&|h| h.width = MAX_DIMENSION + 1));
    assert!(refused(&|h| h.mip_sizes[0] += 8), "stated size differs");
    assert!(decode(&header, &data[..data.len() - 1], &limits).is_err());
    let small = Limits {
        max_asset_bytes: 8 * 8 * 4 - 1,
        ..Limits::default()
    };
    assert!(decode(&header, &data, &small).is_err());
    assert!(decode(&header, &data, &limits).is_ok());
}
