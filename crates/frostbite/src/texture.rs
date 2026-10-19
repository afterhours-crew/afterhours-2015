// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Texture resources: the header a `TextureAsset`'s `Resource` names and the
//! first mip of the pixel chunk it points to. The layout was recovered from
//! the supported installation's UI textures.
//!
//! The 128-byte header is little-endian: two mip offsets, the texture type
//! (0 for 2D), the `RenderFormat`, flags, width, height, depth and slice
//! count (16-bit), mip count and first mip (8-bit), the pixel chunk's GUID,
//! fifteen mip sizes, the chunk size, a name hash and a 16-byte group name.
//! Block formats are decoded with `bcdec_rs`; the RGBA result is straight,
//! not premultiplied, and keeps the stored colour values.
use crate::{Error, Limits};

pub const HEADER_BYTES: usize = 128;
/// Largest width or height decoded.
pub const MAX_DIMENSION: u16 = 4096;
/// Texture type of a plain 2D texture.
pub const TYPE_2D: u32 = 0;

/// Pixel formats this reader decodes, from the engine's `RenderFormat`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    /// `BC1_UNORM`/`BC1_SRGB`: no alpha; transparent texels decode opaque.
    Bc1,
    /// `BC1A_UNORM`/`BC1A_SRGB`: one-bit alpha.
    Bc1Alpha,
    Bc2,
    Bc3,
    Bc7,
    Rgba8,
    Bgra8,
}

impl Format {
    pub fn from_render_format(value: u32) -> Option<Self> {
        Some(match value {
            54 | 55 => Self::Bc1,
            56 | 57 => Self::Bc1Alpha,
            58 | 59 => Self::Bc2,
            60 | 61 => Self::Bc3,
            66 | 67 => Self::Bc7,
            18 | 20 => Self::Rgba8,
            23 | 24 => Self::Bgra8,
            _ => return None,
        })
    }
    /// The engine's name for a decodable `RenderFormat` value.
    pub fn render_format_name(value: u32) -> Option<&'static str> {
        Some(match value {
            54 => "BC1_UNORM",
            55 => "BC1_SRGB",
            56 => "BC1A_UNORM",
            57 => "BC1A_SRGB",
            58 => "BC2_UNORM",
            59 => "BC2_SRGB",
            60 => "BC3_UNORM",
            61 => "BC3_SRGB",
            66 => "BC7_UNORM",
            67 => "BC7_SRGB",
            18 => "R8G8B8A8_UNORM",
            20 => "R8G8B8A8_SRGB",
            23 => "B8G8R8A8_UNORM",
            24 => "B8G8R8A8_SRGB",
            _ => return None,
        })
    }
    /// Bytes per 4x4 block, or per pixel for uncompressed formats.
    fn unit_bytes(self) -> usize {
        match self {
            Self::Bc1 | Self::Bc1Alpha => 8,
            Self::Bc2 | Self::Bc3 | Self::Bc7 => 16,
            Self::Rgba8 | Self::Bgra8 => 4,
        }
    }
    fn is_block(self) -> bool {
        !matches!(self, Self::Rgba8 | Self::Bgra8)
    }
}

/// A parsed texture header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextureHeader {
    pub mip_offsets: [u32; 2],
    pub kind: u32,
    /// The engine's `RenderFormat` value, such as 54 for `BC1_UNORM`.
    pub format: u32,
    pub flags: u16,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub slices: u16,
    pub mip_count: u8,
    pub first_mip: u8,
    pub chunk: [u8; 16],
    pub mip_sizes: [u32; 15],
    pub chunk_size: u32,
    pub name_hash: u32,
    pub group: String,
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}
fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().expect("four bytes"))
}

impl TextureHeader {
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        if data.len() != HEADER_BYTES {
            return Err(Error::Malformed("texture header size"));
        }
        let group = &data[112..128];
        let group = &group[..group.iter().position(|b| *b == 0).unwrap_or(group.len())];
        Ok(Self {
            mip_offsets: [u32_at(data, 0), u32_at(data, 4)],
            kind: u32_at(data, 8),
            format: u32_at(data, 12),
            flags: u16_at(data, 16),
            width: u16_at(data, 18),
            height: u16_at(data, 20),
            depth: u16_at(data, 22),
            slices: u16_at(data, 24),
            mip_count: data[26],
            first_mip: data[27],
            chunk: data[28..44].try_into().expect("sixteen bytes"),
            mip_sizes: std::array::from_fn(|i| u32_at(data, 44 + 4 * i)),
            chunk_size: u32_at(data, 104),
            name_hash: u32_at(data, 108),
            group: String::from_utf8_lossy(group).into_owned(),
        })
    }

    /// Bytes mip 0 occupies at the start of the chunk.
    fn mip0_bytes(&self, format: Format) -> usize {
        let (w, h) = (usize::from(self.width), usize::from(self.height));
        if format.is_block() {
            w.div_ceil(4) * h.div_ceil(4) * format.unit_bytes()
        } else {
            w * h * format.unit_bytes()
        }
    }
}

/// Straight RGBA8 pixels, row by row from the top.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decode mip 0 of a plain 2D texture from the start of its pixel chunk.
/// Cube maps, arrays, volumes, streamed-out first mips and formats outside
/// [`Format`] are refused rather than approximated.
pub fn decode(header: &TextureHeader, chunk: &[u8], limits: &Limits) -> Result<Image, Error> {
    let format = Format::from_render_format(header.format)
        .ok_or(Error::Unsupported("texture pixel format"))?;
    if header.kind != TYPE_2D || header.depth != 1 || header.slices != 1 {
        return Err(Error::Unsupported("texture shape"));
    }
    if header.mip_count == 0 || header.first_mip != 0 || header.mip_offsets[0] != 0 {
        return Err(Error::Unsupported("texture mip layout"));
    }
    if header.width == 0
        || header.height == 0
        || header.width > MAX_DIMENSION
        || header.height > MAX_DIMENSION
    {
        return Err(Error::Bound("texture dimensions"));
    }
    let (width, height) = (usize::from(header.width), usize::from(header.height));
    if width * height * 4 > limits.max_asset_bytes {
        return Err(Error::Bound("decoded texture size"));
    }
    let size = header.mip0_bytes(format);
    if header.mip_sizes[0] as usize != size {
        return Err(Error::Malformed("texture mip size"));
    }
    let data = chunk
        .get(..size)
        .ok_or(Error::Malformed("texture chunk shorter than its first mip"))?;
    let rgba = if format.is_block() {
        let (wide, high) = (width.div_ceil(4) * 4, height.div_ceil(4) * 4);
        let pitch = wide * 4;
        let mut full = vec![0u8; pitch * high];
        for (index, block) in data.chunks_exact(format.unit_bytes()).enumerate() {
            let (bx, by) = (index % (wide / 4), index / (wide / 4));
            let out = &mut full[by * 4 * pitch + bx * 16..];
            match format {
                Format::Bc1 | Format::Bc1Alpha => bcdec_rs::bc1(block, out, pitch),
                Format::Bc2 => bcdec_rs::bc2(block, out, pitch),
                Format::Bc3 => bcdec_rs::bc3(block, out, pitch),
                Format::Bc7 => bcdec_rs::bc7(block, out, pitch),
                Format::Rgba8 | Format::Bgra8 => unreachable!("not a block format"),
            }
        }
        let mut rgba = Vec::with_capacity(width * height * 4);
        for row in full.chunks_exact(pitch).take(height) {
            rgba.extend_from_slice(&row[..width * 4]);
        }
        if format == Format::Bc1 {
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
        }
        rgba
    } else {
        let mut rgba = data.to_vec();
        if format == Format::Bgra8 {
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        rgba
    };
    Ok(Image {
        width: u32::from(header.width),
        height: u32::from(header.height),
        rgba,
    })
}

#[cfg(test)]
mod tests;
