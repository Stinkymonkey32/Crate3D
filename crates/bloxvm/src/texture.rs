//! Roblox texture decoding.
//!
//! Roblox stores runtime textures as DDS files (DXT-family block compression).
//! Place files reference them via `Content` asset URIs (`rbxassetid://...`,
//! `rbxasset://...`) or embed the raw bytes directly through `SharedString`
//! /`BinaryString` properties (e.g. `SurfaceAppearance.ColorMap`).
//!
//! This module parses the DDS container and decompresses the common block
//! formats (DXT1/3/5, BC4, BC5) to straight RGBA8 so they can be handed to a
//! renderer. Unsupported formats are reported but never cause loading to fail.

use std::collections::HashMap;

use flate2::read::ZlibDecoder;
use glow::HasContext;
use std::io::Read;

/// OpenGL constant for `GL_COMPRESSED_RGBA_S3TC_DXT1_EXT`.
const COMPRESSED_RGBA_S3TC_DXT1_EXT: u32 = 0x83F1;
/// OpenGL constant for `GL_COMPRESSED_RGBA_S3TC_DXT3_EXT`.
const COMPRESSED_RGBA_S3TC_DXT3_EXT: u32 = 0x83F2;
/// OpenGL constant for `GL_COMPRESSED_RGBA_S3TC_DXT5_EXT`.
const COMPRESSED_RGBA_S3TC_DXT5_EXT: u32 = 0x83F3;

/// The compressed pixel format of a DDS surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    /// DXT1 / BC1, 8 bytes per 4x4 block.
    Dxt1,
    /// DXT3 / BC2, 16 bytes per 4x4 block.
    Dxt3,
    /// DXT5 / BC3, 16 bytes per 4x4 block.
    Dxt5,
    /// BC4, single channel, 16 bytes per 4x4 block.
    Bc4,
    /// BC5, two channels, 16 bytes per 4x4 block.
    Bc5,
    /// BC6H, HDR. Header parsed, decompression not implemented.
    Bc6h,
    /// BC7, 16 bytes per 4x4 block. Header parsed, decompression not implemented.
    Bc7,
    /// Uncompressed 32-bit BGRA.
    Bgra8,
    /// Uncompressed 32-bit RGBA.
    Rgba8,
    /// Uncompressed 24-bit RGB.
    Rgb8,
    /// Uncompressed 8-bit grayscale.
    R8,
    /// Something this module does not recognize.
    Unknown,
}

/// A parsed DDS texture. `data` holds every mip level back-to-back.
#[derive(Debug, Clone)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mip_levels: u32,
    pub format: TextureFormat,
    pub data: Vec<u8>,
}

const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_DEPTH: u32 = 0x800000;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSD_MIPMAPCOUNT: u32 = 0x20000;

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x20000;

fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Parses a DDS file. Returns `None` for non-DDS input; malformed or exotic
/// surfaces come back with [`TextureFormat::Unknown`] rather than an error.
pub fn parse_dds(bytes: &[u8]) -> Option<Texture> {
    if bytes.len() < 128 || &bytes[0..4] != b"DDS " {
        return None;
    }
    let h = &bytes[4..4 + 124];

    let flags = le_u32(h, 4);
    let height = if flags & DDSD_HEIGHT != 0 { le_u32(h, 8) } else { 1 };
    let width = if flags & DDSD_WIDTH != 0 { le_u32(h, 12) } else { 1 };
    let depth = if flags & DDSD_DEPTH != 0 { le_u32(h, 20) } else { 1 };
    let mip_levels = if flags & DDSD_MIPMAPCOUNT != 0 { le_u32(h, 24).max(1) } else { 1 };

    // DDS_PIXELFORMAT sits at header offset 72.
    let pf_flags = le_u32(h, 76);
    let fourcc = le_u32(h, 80);
    let bit_count = le_u32(h, 84);
    let a_mask = le_u32(h, 96);

    let mut is_dx10 = false;
    let format = if flags & DDSD_PIXELFORMAT == 0 {
        TextureFormat::Unknown
    } else if pf_flags & DDPF_FOURCC != 0 {
        match fourcc {
            0x31545844 => TextureFormat::Dxt1,       // DXT1
            0x33545844 => TextureFormat::Dxt3,       // DXT3
            0x35545844 => TextureFormat::Dxt5,       // DXT5
            0x31495441 | 0x31424334 => TextureFormat::Bc4, // ATI1 / BC4U
            0x32495441 | 0x31424335 => TextureFormat::Bc5, // ATI2 / BC5U
            0x31424336 => TextureFormat::Bc6h,       // BC6H
            0x31424337 => TextureFormat::Bc7,        // BC7U
            0x30315844 => {
                // "DX10": extended header follows, DXGI format decides.
                if bytes.len() < 4 + 124 + 20 {
                    return None;
                }
                is_dx10 = true;
                let dxgi = le_u32(&bytes[4 + 124..], 0);
                dxgi_format(dxgi)
            }
            _ => TextureFormat::Unknown,
        }
    } else if pf_flags & (DDPF_RGB | DDPF_LUMINANCE) != 0 {
        match bit_count {
            32 => {
                if pf_flags & DDPF_ALPHAPIXELS != 0 || a_mask != 0 {
                    TextureFormat::Rgba8
                } else {
                    TextureFormat::Rgb8
                }
            }
            24 => TextureFormat::Rgb8,
            8 => TextureFormat::R8,
            _ => TextureFormat::Unknown,
        }
    } else {
        TextureFormat::Unknown
    };

    let data_start = 4 + 124 + if is_dx10 { 20 } else { 0 };
    let data = bytes[data_start..].to_vec();

    Some(Texture {
        width,
        height,
        depth,
        mip_levels,
        format,
        data,
    })
}

/// Parses a PNG file and decompresses it to an RGBA8 [`Texture`].
/// Returns `None` for non-PNG input or unsupported formats.
pub fn parse_png(bytes: &[u8]) -> Option<Texture> {
    if bytes.len() < 8 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }

    let mut offset = 8;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut idat_data = Vec::new();

    while offset + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().ok()?) as usize;
        let chunk_type = &bytes[offset + 4..offset + 8];

        if offset + 12 + len > bytes.len() {
            break;
        }

        let chunk_data = &bytes[offset + 8..offset + 8 + len];

        match chunk_type {
            b"IHDR" => {
                if chunk_data.len() < 13 {
                    return None;
                }
                width = u32::from_be_bytes(chunk_data[0..4].try_into().ok()?);
                height = u32::from_be_bytes(chunk_data[4..8].try_into().ok()?);
                bit_depth = chunk_data[8];
                color_type = chunk_data[9];
            }
            b"IDAT" => {
                idat_data.extend_from_slice(chunk_data);
            }
            b"IEND" => break,
            _ => {}
        }

        offset += 12 + len;
    }

    if width == 0 || height == 0 || idat_data.is_empty() {
        return None;
    }

    // Decompress the concatenated IDAT data (zlib stream).
    let mut decoder = ZlibDecoder::new(&idat_data[..]);
    let mut raw = Vec::new();
    decoder.read_to_end(&mut raw).ok()?;

    let bpp: usize = match color_type {
        0 => 1, // Grayscale
        2 => 3, // RGB
        3 => 1, // Indexed (palette)
        4 => 2, // Grayscale + Alpha
        6 => 4, // RGBA
        _ => return None,
    };

    if bit_depth != 8 {
        return None; // Only 8-bit per channel supported.
    }

    let stride = width as usize * bpp;
    let mut rgba = vec![0u8; width as usize * height as usize * 4];

    for y in 0..height as usize {
        let row_start = y * (stride + 1);
        if row_start >= raw.len() {
            return None;
        }
        let filter = raw[row_start];
        let src_row_start = row_start + 1;
        let src_row_end = src_row_start + stride;
        if src_row_end > raw.len() {
            return None;
        }
        let src_row = &raw[src_row_start..src_row_end];
        let row_off = y * width as usize * 4;
        let prev_off = if y > 0 { (y - 1) * width as usize * 4 } else { 0 };

        for x in 0..width as usize {
            let si = x * bpp;
            let di = x * 4;

            let (r, g, b, a) = match color_type {
                6 => (src_row[si], src_row[si + 1], src_row[si + 2], src_row[si + 3]),
                2 => (src_row[si], src_row[si + 1], src_row[si + 2], 255),
                0 => {
                    let v = src_row[si];
                    (v, v, v, 255)
                }
                4 => {
                    let v = src_row[si];
                    (v, v, v, src_row[si + 1])
                }
                _ => (0, 0, 0, 255),
            };

            match filter {
                0 => {
                    rgba[row_off + di] = r;
                    rgba[row_off + di + 1] = g;
                    rgba[row_off + di + 2] = b;
                    rgba[row_off + di + 3] = a;
                }
                1 => { // Sub
                    let pr = if x > 0 { rgba[row_off + di - 4] } else { 0 };
                    let pg = if x > 0 { rgba[row_off + di - 3] } else { 0 };
                    let pb = if x > 0 { rgba[row_off + di - 2] } else { 0 };
                    let pa = if x > 0 { rgba[row_off + di - 1] } else { 0 };
                    rgba[row_off + di] = r.wrapping_add(pr);
                    rgba[row_off + di + 1] = g.wrapping_add(pg);
                    rgba[row_off + di + 2] = b.wrapping_add(pb);
                    rgba[row_off + di + 3] = a.wrapping_add(pa);
                }
                2 => { // Up
                    let pr = if y > 0 { rgba[prev_off + di] } else { 0 };
                    let pg = if y > 0 { rgba[prev_off + di + 1] } else { 0 };
                    let pb = if y > 0 { rgba[prev_off + di + 2] } else { 0 };
                    let pa = if y > 0 { rgba[prev_off + di + 3] } else { 0 };
                    rgba[row_off + di] = r.wrapping_add(pr);
                    rgba[row_off + di + 1] = g.wrapping_add(pg);
                    rgba[row_off + di + 2] = b.wrapping_add(pb);
                    rgba[row_off + di + 3] = a.wrapping_add(pa);
                }
                3 => { // Average
                    let pr = if x > 0 { rgba[row_off + di - 4] } else { 0 };
                    let pg = if x > 0 { rgba[row_off + di - 3] } else { 0 };
                    let pb = if x > 0 { rgba[row_off + di - 2] } else { 0 };
                    let pa = if x > 0 { rgba[row_off + di - 1] } else { 0 };
                    let ar = if y > 0 { rgba[prev_off + di] } else { 0 };
                    let ag = if y > 0 { rgba[prev_off + di + 1] } else { 0 };
                    let ab = if y > 0 { rgba[prev_off + di + 2] } else { 0 };
                    let aa = if y > 0 { rgba[prev_off + di + 3] } else { 0 };
                    rgba[row_off + di] = r.wrapping_add(((pr as u16 + ar as u16) / 2) as u8);
                    rgba[row_off + di + 1] = g.wrapping_add(((pg as u16 + ag as u16) / 2) as u8);
                    rgba[row_off + di + 2] = b.wrapping_add(((pb as u16 + ab as u16) / 2) as u8);
                    rgba[row_off + di + 3] = a.wrapping_add(((pa as u16 + aa as u16) / 2) as u8);
                }
                4 => { // Paeth
                    let paeth = |pr: u8, ar: u8, br: u8| -> u8 {
                        let p = pr as i16 + ar as i16 - br as i16;
                        let (pr_i, ar_i, br_i) = (pr as i16, ar as i16, br as i16);
                        let (pa, pc, pbb) = ((p - pr_i).abs(), (p - ar_i).abs(), (p - br_i).abs());
                        if pa <= pc && pa <= pbb { pr }
                        else if pc <= pbb { ar }
                        else { br }
                    };
                    let pr_r = if x > 0 { rgba[row_off + di - 4] } else { 0 };
                    let pr_g = if x > 0 { rgba[row_off + di - 3] } else { 0 };
                    let pr_b = if x > 0 { rgba[row_off + di - 2] } else { 0 };
                    let pr_a = if x > 0 { rgba[row_off + di - 1] } else { 0 };
                    let ar = if y > 0 { rgba[prev_off + di] } else { 0 };
                    let ag = if y > 0 { rgba[prev_off + di + 1] } else { 0 };
                    let ab = if y > 0 { rgba[prev_off + di + 2] } else { 0 };
                    let aa = if y > 0 { rgba[prev_off + di + 3] } else { 0 };
                    let br = if x > 0 && y > 0 { rgba[prev_off + di - 4] } else { 0 };
                    let bg = if x > 0 && y > 0 { rgba[prev_off + di - 3] } else { 0 };
                    let bb = if x > 0 && y > 0 { rgba[prev_off + di - 2] } else { 0 };
                    let ba = if x > 0 && y > 0 { rgba[prev_off + di - 1] } else { 0 };
                    rgba[row_off + di] = r.wrapping_add(paeth(pr_r, ar, br));
                    rgba[row_off + di + 1] = g.wrapping_add(paeth(pr_g, ag, bg));
                    rgba[row_off + di + 2] = b.wrapping_add(paeth(pr_b, ab, bb));
                    rgba[row_off + di + 3] = a.wrapping_add(paeth(pr_a, aa, ba));
                }
                _ => {
                    rgba[row_off + di] = r;
                    rgba[row_off + di + 1] = g;
                    rgba[row_off + di + 2] = b;
                    rgba[row_off + di + 3] = a;
                }
            }
        }
    }

    Some(Texture {
        width,
        height,
        depth: 1,
        mip_levels: 1,
        format: TextureFormat::Rgba8,
        data: rgba,
    })
}

/// Maps a DXGI format id (from the DX10 header) to a [`TextureFormat`].
fn dxgi_format(dxgi: u32) -> TextureFormat {
    match dxgi {
        71 | 72 => TextureFormat::Dxt1, // BC1_UNORM / BC1_UNORM_SRGB
        74 | 75 => TextureFormat::Dxt3, // BC2_UNORM / BC2_UNORM_SRGB
        77 | 78 => TextureFormat::Dxt5, // BC3_UNORM / BC3_UNORM_SRGB
        80 | 81 => TextureFormat::Bc4,  // BC4_UNORM / BC4_SNORM
        83 | 84 => TextureFormat::Bc5,  // BC5_UNORM / BC5_SNORM
        95 | 96 => TextureFormat::Bc6h, // BC6H_UF16 / BC6H_SF16
        98 | 99 => TextureFormat::Bc7,  // BC7_UNORM / BC7_UNORM_SRGB
        28 | 29 | 87 | 88 => TextureFormat::Rgba8, // R8G8B8A8 / B8G8R8A8
        61 | 62 => TextureFormat::R8,   // R8_UNORM / R8_UNORM_SRGB
        _ => TextureFormat::Unknown,
    }
}

impl TextureFormat {
    /// The OpenGL internal format constant for uploading this format to the
    /// GPU, or `None` if this format cannot be uploaded directly.
    pub fn gl_internal_format(&self) -> Option<u32> {
        match self {
            TextureFormat::Dxt1 => Some(COMPRESSED_RGBA_S3TC_DXT1_EXT),
            TextureFormat::Dxt3 => Some(COMPRESSED_RGBA_S3TC_DXT3_EXT),
            TextureFormat::Dxt5 => Some(COMPRESSED_RGBA_S3TC_DXT5_EXT),
            // BC4/BC5 need ARB_texture_compression / EXT_texture_compression_rgtc
            // which is rarer than S3TC. Fall back to RGBA8 for those.
            TextureFormat::Rgba8 => Some(glow::RGBA8 as u32),
            TextureFormat::Bgra8 => Some(glow::RGBA8 as u32),
            TextureFormat::Rgb8 => Some(glow::RGB8 as u32),
            TextureFormat::R8 => Some(glow::R8 as u32),
            _ => None,
        }
    }
}

impl Texture {
    /// `true` if the surface uses a 4x4 block-compressed format.
    pub fn is_compressed(&self) -> bool {
        matches!(
            self.format,
            TextureFormat::Dxt1 | TextureFormat::Dxt3 | TextureFormat::Dxt5 | TextureFormat::Bc4 | TextureFormat::Bc5 | TextureFormat::Bc6h | TextureFormat::Bc7
        )
    }

    /// Bytes per 4x4 block for block-compressed formats, or `None` for
    /// uncompressed formats.
    pub fn compressed_block_size(&self) -> Option<usize> {
        match self.format {
            TextureFormat::Dxt1 => Some(8),
            TextureFormat::Dxt3 | TextureFormat::Dxt5
            | TextureFormat::Bc4 | TextureFormat::Bc5
            | TextureFormat::Bc6h | TextureFormat::Bc7 => Some(16),
            _ => None,
        }
    }

    /// Returns the raw byte slice for a single mip level, or `None` if the
    /// data is too short.
    pub fn mip_data(&self, level: u32) -> Option<&[u8]> {
        let mut offset = 0usize;
        for l in 0..self.mip_levels {
            let (w, h) = self.mip_dimensions(l);
            let blocks = if self.is_compressed() {
                ((w as usize + 3) / 4) * ((h as usize + 3) / 4) * self.block_size()
            } else {
                w as usize * h as usize * self.pixel_bytes()
            };
            if l == level {
                return self.data.get(offset..offset + blocks);
            }
            offset += blocks;
        }
        None
    }

    /// Bytes per pixel for uncompressed formats.
    fn pixel_bytes(&self) -> usize {
        match self.format {
            TextureFormat::Rgba8 | TextureFormat::Bgra8 => 4,
            TextureFormat::Rgb8 => 3,
            TextureFormat::R8 => 1,
            _ => 0,
        }
    }

    /// Bytes per 4x4 block for block-compressed formats (internal use).
    fn block_size(&self) -> usize {
        match self.format {
            TextureFormat::Dxt1 => 8,
            _ => 16,
        }
    }

    /// The dimensions of a given mip level (`0` = base).
    pub fn mip_dimensions(&self, level: u32) -> (u32, u32) {
        (
            (self.width >> level).max(1),
            (self.height >> level).max(1),
        )
    }

    /// Decompresses the whole mip chain to RGBA8. Returns `None` for formats
    /// this module cannot decompress (BC6H, BC7, unknown).
    pub fn to_rgba8(&self) -> Option<Vec<u8>> {
        match self.format {
            TextureFormat::Dxt1 | TextureFormat::Dxt3 | TextureFormat::Dxt5 | TextureFormat::Bc4 | TextureFormat::Bc5 => {
                let mut out = Vec::new();
                let mut offset = 0usize;
                for level in 0..self.mip_levels {
                    let (w, h) = self.mip_dimensions(level);
                    let blocks = ((w as usize + 3) / 4) * ((h as usize + 3) / 4) * self.block_size();
                    let level_data = self.data.get(offset..offset + blocks)?;
                    offset += blocks;
                    out.extend(decompress_level(self.format, level_data, w, h));
                }
                Some(out)
            }
            TextureFormat::Rgba8 => Some(self.data.clone()),
            TextureFormat::Bgra8 => Some(
                self.data
                    .chunks_exact(4)
                    .flat_map(|p| [p[2], p[1], p[0], p[3]])
                    .collect(),
            ),
            TextureFormat::Rgb8 => Some(
                self.data
                    .chunks_exact(3)
                    .flat_map(|p| [p[0], p[1], p[2], 255])
                    .collect(),
            ),
            TextureFormat::R8 => Some(self.data.iter().flat_map(|&v| [v, v, v, 255]).collect()),
            _ => None,
        }
    }
}

/// Decompresses a single mip level into `w * h * 4` RGBA8 bytes.
fn decompress_level(format: TextureFormat, data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let bw = (w + 3) / 4;
    let bh = (h + 3) / 4;
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let mut block = [0u8; 16];
    let mut rgba = [0u8; 64];

    for by in 0..bh {
        for bx in 0..bw {
            let bi = (by * bw + bx) as usize;
            let block_size = match format {
                TextureFormat::Dxt1 => 8,
                _ => 16,
            };
            let start = bi * block_size;
            if start + block_size > data.len() {
                break;
            }
            block[..block_size].copy_from_slice(&data[start..start + block_size]);
            rgba.fill(0);
            match format {
                TextureFormat::Dxt1 => decode_dxt1_color(&block[..8], &mut rgba),
                TextureFormat::Dxt3 => {
                    decode_dxt3_alpha(&block[..8], &mut rgba);
                    decode_dxt1_color(&block[8..16], &mut rgba);
                }
                TextureFormat::Dxt5 => {
                    decode_dxt5_alpha(&block[..8], &mut rgba);
                    decode_dxt1_color(&block[8..16], &mut rgba);
                }
                TextureFormat::Bc4 => decode_dxt5_alpha(&block[..8], &mut rgba),
                TextureFormat::Bc5 => {
                    decode_dxt5_alpha(&block[..8], &mut rgba);
                    let mut green = [0u8; 16];
                    decode_dxt5_alpha(&block[8..16], &mut green);
                    for p in 0..16 {
                        rgba[p * 4 + 1] = green[p * 4];
                    }
                }
                _ => {}
            }
            write_block(&mut out, &rgba, w as usize, h as usize, bx as usize, by as usize);
        }
    }
    out
}

/// Copies a decoded 4x4 block into the output, clipping at the right/bottom
/// edge for surfaces whose size is not a multiple of 4.
fn write_block(out: &mut [u8], block: &[u8], w: usize, h: usize, bx: usize, by: usize) {
    let x0 = bx * 4;
    let y0 = by * 4;
    for y in 0..4 {
        if y0 + y >= h {
            break;
        }
        for x in 0..4 {
            if x0 + x >= w {
                break;
            }
            let src = (y * 4 + x) * 4;
            let dst = ((y0 + y) * w + (x0 + x)) * 4;
            out[dst..dst + 4].copy_from_slice(&block[src..src + 4]);
        }
    }
}

/// Expands an RGB565 color to RGBA8.
fn unpack_rgb565(c: u16, out: &mut [u8]) {
    let r = ((c >> 11) & 0x1f) as u8;
    let g = ((c >> 5) & 0x3f) as u8;
    let b = (c & 0x1f) as u8;
    out[0] = (r << 3) | (r >> 2);
    out[1] = (g << 2) | (g >> 4);
    out[2] = (b << 3) | (b >> 2);
    out[3] = 255;
}

fn mix(a: &[u8], b: &[u8], wa: u32, wb: u32, out: &mut [u8]) {
    for i in 0..3 {
        out[i] = ((a[i] as u32 * wa + b[i] as u32 * wb + 1) / (wa + wb)) as u8;
    }
    out[3] = 255;
}

/// Decodes the 8-byte color block shared by DXT1/3/5 into 4x4 RGBA8.
/// DXT1 uses the transparent index 3 in 3-color mode; the others always use
/// 4-color mode (callers keep that case unreachable by checking `c0 > c1`).
fn decode_dxt1_color(block: &[u8], out: &mut [u8]) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);

    let mut colors = [0u8; 16];
    unpack_rgb565(c0, &mut colors[0..4]);
    unpack_rgb565(c1, &mut colors[4..8]);
    let a = [colors[0], colors[1], colors[2]];
    let b = [colors[4], colors[5], colors[6]];
    if c0 > c1 {
        mix(&a, &b, 2, 1, &mut colors[8..12]);
        mix(&a, &b, 1, 2, &mut colors[12..16]);
    } else {
        mix(&a, &b, 1, 1, &mut colors[8..12]);
        colors[12..16].fill(0); // transparent black
    }

    for y in 0..4 {
        for x in 0..4 {
            let idx = ((indices >> (2 * (y * 4 + x))) & 3) as usize;
            let src = idx * 4;
            let dst = (y * 4 + x) * 4;
            out[dst..dst + 4].copy_from_slice(&colors[src..src + 4]);
        }
    }
}

/// Decodes the 4-bit-per-pixel alpha block of DXT3 into the alpha channel.
fn decode_dxt3_alpha(block: &[u8], out: &mut [u8]) {
    for p in 0..16 {
        let byte = block[p / 2];
        let nibble = if p % 2 == 0 { byte & 0x0f } else { byte >> 4 };
        out[p * 4 + 3] = nibble * 17;
    }
}

/// Decodes the interpolated 8-bit alpha block of DXT5/BC4 into the alpha
/// channel (BC4 reuses this for a single grayscale channel).
fn decode_dxt5_alpha(block: &[u8], out: &mut [u8]) {
    let a0 = block[0];
    let a1 = block[1];
    let bits = u64::from_le_bytes([block[2], block[3], block[4], block[5], block[6], block[7], 0, 0]);

    let alphas: [u8; 8] = if a0 > a1 {
        [
            a0,
            a1,
            ((6 * a0 as u32 + 1 * a1 as u32) / 7) as u8,
            ((5 * a0 as u32 + 2 * a1 as u32) / 7) as u8,
            ((4 * a0 as u32 + 3 * a1 as u32) / 7) as u8,
            ((3 * a0 as u32 + 4 * a1 as u32) / 7) as u8,
            ((2 * a0 as u32 + 5 * a1 as u32) / 7) as u8,
            ((1 * a0 as u32 + 6 * a1 as u32) / 7) as u8,
        ]
    } else {
        [
            a0,
            a1,
            ((4 * a0 as u32 + 1 * a1 as u32) / 5) as u8,
            ((3 * a0 as u32 + 2 * a1 as u32) / 5) as u8,
            ((2 * a0 as u32 + 3 * a1 as u32) / 5) as u8,
            ((1 * a0 as u32 + 4 * a1 as u32) / 5) as u8,
            0,
            255,
        ]
    };

    for p in 0..16 {
        let idx = ((bits >> (p * 3)) & 7) as usize;
        out[p * 4 + 3] = alphas[idx];
    }
}

/// Parses a 32-character hex string (MD5 key) into `[u8; 16]`.
pub fn parse_md5_hex(s: &str) -> Option<[u8; 16]> {
    let s = s.trim();
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Manages GPU textures: upload, cache by MD5 key, and a magenta fallback.
pub struct TextureManager {
    cache: HashMap<[u8; 16], glow::Texture>,
    fallback: glow::Texture,
    has_s3tc: bool,
}

impl TextureManager {
    /// Creates a new texture manager. Currently always uses the RGBA8 software
    /// decompression path. Compressed GPU upload (S3TC) can be enabled later
    /// once `glGetStringi` is available via glow.
    pub fn new(gl: &glow::Context) -> Self {
        let has_s3tc = false;

        let fallback = Self::create_magenta_fallback(gl);

        TextureManager {
            cache: HashMap::new(),
            fallback,
            has_s3tc,
        }
    }

    /// Uploads a parsed [`Texture`] to the GPU, keyed by its MD5.
    /// Compressed formats are uploaded directly if S3TC is available;
    /// otherwise the texture is decompressed to RGBA8 first.
    pub fn upload(&mut self, gl: &glow::Context, key: &[u8; 16], tex: &Texture) {
        if self.cache.contains_key(key) {
            return;
        }

        let gl_tex = unsafe { gl.create_texture() }.expect("create texture");
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(gl_tex));
        }

        let uploaded = if self.has_s3tc && tex.is_compressed() && tex.format.gl_internal_format().is_some() {
            Self::upload_compressed(gl, tex)
        } else {
            Self::upload_rgba8(gl, tex)
        };

        if uploaded {
            self.cache.insert(*key, gl_tex);
        } else {
            unsafe { gl.delete_texture(gl_tex) };
        }
    }

    /// Returns the GL texture handle for the given key, or the magenta
    /// fallback if the key is not in the cache.
    pub fn handle(&self, key: &[u8; 16]) -> glow::Texture {
        self.cache.get(key).copied().unwrap_or(self.fallback)
    }

    /// The magenta checkerboard fallback texture.
    pub fn fallback(&self) -> glow::Texture {
        self.fallback
    }

    /// Frees all GPU textures owned by this manager.
    pub fn delete(&mut self, gl: &glow::Context) {
        for tex in self.cache.values() {
            unsafe { gl.delete_texture(*tex) };
        }
        self.cache.clear();
        unsafe { gl.delete_texture(self.fallback) };
    }

    fn upload_compressed(gl: &glow::Context, tex: &Texture) -> bool {
        let gl_fmt = match tex.format.gl_internal_format() {
            Some(f) => f,
            None => return false,
        };
        if tex.compressed_block_size().is_none() {
            return false;
        }

        for level in 0..tex.mip_levels {
            let (w, h) = tex.mip_dimensions(level);
            if let Some(data) = tex.mip_data(level) {
                unsafe {
                    gl.compressed_tex_image_2d(
                        glow::TEXTURE_2D,
                        level as i32,
                        gl_fmt as i32,
                        w as i32,
                        h as i32,
                        0,
                        data.len() as i32,
                        data,
                    );
                }
            }
        }

        unsafe {
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
        }
        true
    }

    fn upload_rgba8(gl: &glow::Context, tex: &Texture) -> bool {
        let rgba = match tex.to_rgba8() {
            Some(data) => data,
            None => return false,
        };

        let (w, h) = tex.mip_dimensions(0);
        unsafe {
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(&rgba),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
            gl.generate_mipmap(glow::TEXTURE_2D);
        }
        true
    }

    /// Creates a 4x4 magenta/green checkerboard texture for missing assets.
    fn create_magenta_fallback(gl: &glow::Context) -> glow::Texture {
        let tex = unsafe { gl.create_texture() }.expect("create fallback texture");
        // Classic magenta checkered: bright magenta and dark green.
        let pixels: [u8; 64] = [
            // Row 0
            255, 0, 255, 255,   0, 0, 0, 255,   255, 0, 255, 255,   0, 0, 0, 255,
            // Row 1
            0, 0, 0, 255,   255, 0, 255, 255,   0, 0, 0, 255,   255, 0, 255, 255,
            // Row 2
            255, 0, 255, 255,   0, 0, 0, 255,   255, 0, 255, 255,   0, 0, 0, 255,
            // Row 3
            0, 0, 0, 255,   255, 0, 255, 255,   0, 0, 0, 255,   255, 0, 255, 255,
        ];
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                4,
                4,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(&pixels),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
        }
        tex
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal DXT1 DDS header for `w x h` with `levels` mips.
    fn dds_header(w: u32, h: u32, fourcc: &[u8; 4], levels: u32, linear_size: u32) -> Vec<u8> {
        const DDSD_CAPS: u32 = 0x1;
        const DDSD_LINEARSIZE: u32 = 0x80000;
        let mut b = vec![0u8; 4 + 124];
        b[0..4].copy_from_slice(b"DDS ");
        let flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT | DDSD_MIPMAPCOUNT | DDSD_LINEARSIZE;
        b[4 + 0..4 + 4].copy_from_slice(&124u32.to_le_bytes()); // dwSize
        b[4 + 4..4 + 8].copy_from_slice(&flags.to_le_bytes());
        b[4 + 8..4 + 12].copy_from_slice(&h.to_le_bytes());
        b[4 + 12..4 + 16].copy_from_slice(&w.to_le_bytes());
        b[4 + 16..4 + 20].copy_from_slice(&linear_size.to_le_bytes());
        b[4 + 24..4 + 28].copy_from_slice(&levels.to_le_bytes());
        // DDS_PIXELFORMAT at header offset 72 (byte 76 in the file).
        let pf = 4 + 72;
        b[pf..pf + 4].copy_from_slice(&32u32.to_le_bytes());
        b[pf + 4..pf + 8].copy_from_slice(&DDPF_FOURCC.to_le_bytes());
        b[pf + 8..pf + 12].copy_from_slice(fourcc);
        // dwCaps = DDSCAPS_TEXTURE at header offset 104.
        b[4 + 104..4 + 108].copy_from_slice(&0x1000u32.to_le_bytes());
        b
    }

    #[test]
    fn parses_dxt1_header() {
        let hdr = dds_header(8, 8, b"DXT1", 1, 64);
        let tex = parse_dds(&hdr).expect("parse");
        assert_eq!(tex.width, 8);
        assert_eq!(tex.height, 8);
        assert_eq!(tex.mip_levels, 1);
        assert_eq!(tex.format, TextureFormat::Dxt1);
        assert!(tex.is_compressed());
    }

    #[test]
    fn rejects_non_dds() {
        assert!(parse_dds(b"not a dds file").is_none());
    }

    #[test]
    fn dxt1_decompresses_to_rgba() {
        let mut data = dds_header(8, 8, b"DXT1", 1, 64);
        // One 8x8 block pair, all-white via c0 > c1 with a packed index 0.
        for _ in 0..8 {
            let c0: u16 = 0xFFFF;
            let c1: u16 = 0x0000;
            data.extend_from_slice(&c0.to_le_bytes());
            data.extend_from_slice(&c1.to_le_bytes());
            data.extend_from_slice(&0u32.to_le_bytes());
        }
        let tex = parse_dds(&data).unwrap();
        let rgba = tex.to_rgba8().expect("decompress");
        assert_eq!(rgba.len(), 8 * 8 * 4);
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[(8 * 8 - 1) * 4..], &[255, 255, 255, 255]);
    }

    #[test]
    fn dxt5_decompresses_with_alpha() {
        let mut data = dds_header(4, 4, b"DXT5", 1, 16);
        // alpha refs a0=255, a1=0, indices all 0 (opaque).
        data.extend_from_slice(&[255, 0, 0, 0, 0, 0, 0, 0]);
        // color block: c0=white, c1=black, index 0.
        data.extend_from_slice(&0xFFFFu16.to_le_bytes());
        data.extend_from_slice(&0x0000u16.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        let tex = parse_dds(&data).unwrap();
        let rgba = tex.to_rgba8().unwrap();
        assert_eq!(rgba.len(), 4 * 4 * 4);
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[3 * 4..4 * 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn parses_md5_hex() {
        let key = "d41d8cd98f00b204e9800998ecf8427e";
        let result = parse_md5_hex(key).expect("parse md5");
        assert_eq!(result[0], 0xd4);
        assert_eq!(result[1], 0x1d);
        assert_eq!(result[15], 0x7e);
    }

    #[test]
    fn rejects_invalid_md5_hex() {
        assert!(parse_md5_hex("not-a-valid-md5").is_none());
        assert!(parse_md5_hex("d41d8cd98f00b204e9800998ecf8427").is_none()); // too short
        assert!(parse_md5_hex("").is_none());
    }

    #[test]
    fn mip_data_returns_correct_slice() {
        let mut data = dds_header(8, 8, b"DXT1", 2, 64);
        // Level 0: 8x8 = 2x2 blocks = 4 blocks * 8 bytes = 32 bytes
        // Level 1: 4x4 = 1 block * 8 bytes = 8 bytes
        for _ in 0..40 {
            data.push(0xFF);
        }
        let tex = parse_dds(&data).unwrap();
        assert_eq!(tex.mip_data(0).unwrap().len(), 32);
        assert_eq!(tex.mip_data(1).unwrap().len(), 8);
        assert!(tex.mip_data(2).is_none());
    }

    #[test]
    fn odd_dimensions_clip() {
        let mut data = dds_header(5, 3, b"DXT1", 1, 16);
        for _ in 0..2 {
            data.extend_from_slice(&0xFFFFu16.to_le_bytes());
            data.extend_from_slice(&0x0000u16.to_le_bytes());
            data.extend_from_slice(&0u32.to_le_bytes());
        }
        let tex = parse_dds(&data).unwrap();
        let rgba = tex.to_rgba8().unwrap();
        assert_eq!(rgba.len(), 5 * 3 * 4);
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        // Bottom-right clipped pixel is still opaque white.
        assert_eq!(&rgba[(5 * 3 - 1) * 4..], &[255, 255, 255, 255]);
    }
}
