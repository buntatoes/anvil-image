// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2026 Buntato
//! RIFF `WEBP` container walk and the glue between its chunks.
//!
//! A file is `RIFF`, a little-endian size, `WEBP`, then chunks. Each chunk is
//! a FourCC, a little-endian payload length, the payload, and one pad byte
//! when the length is odd. These layouts are read:
//!
//! * simple lossy: a single `VP8 ` chunk;
//! * simple lossless: a single `VP8L` chunk;
//! * extended: a `VP8X` header (flags and canvas size) followed by optional
//!   metadata chunks, an optional `ALPH` chunk, and one `VP8 ` or `VP8L`
//!   bitstream chunk;
//! * the layout [`encode_webp`](crate::encode_webp) writes: `VP8X` plus a
//!   zlib `RGBA` chunk.
//!
//! `ALPH` only applies to a lossy bitstream; lossless bitstreams carry their
//! own alpha. Animated files (`VP8X` animation flag, `ANIM`, or `ANMF`) are
//! refused with [`ImageError::UnsupportedFormat`]. Metadata chunks (`ICCP`,
//! `EXIF`, `XMP `) and unknown chunks are skipped.

mod vp8;
mod vp8l;

use crate::{Image, ImageError, validate_dimensions};
use anvil_compress::decompress_zlib_bounded;

/// `VP8X` flag bit for an animated file.
const FLAG_ANIMATION: u8 = 0x02;

/// Chunks found during the walk. Only the first of each kind is kept.
#[derive(Default)]
struct Chunks<'a> {
    canvas: Option<(u32, u32)>,
    vp8l: Option<&'a [u8]>,
    vp8: Option<&'a [u8]>,
    alpha: Option<&'a [u8]>,
    rgba: Option<&'a [u8]>,
}

fn read_u24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

/// Walk the chunk list. Stops at the end of the RIFF payload or the buffer,
/// whichever comes first.
fn walk(bytes: &[u8]) -> Result<Chunks<'_>, ImageError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(ImageError::InvalidHeader("missing RIFF WEBP header"));
    }
    let riff_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let end = riff_size.saturating_add(8).clamp(12, bytes.len());

    let mut chunks = Chunks::default();
    let mut pos = 12usize;
    while pos + 8 <= end {
        let fourcc = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        pos += 8;
        if len > end - pos {
            return Err(ImageError::UnexpectedEof);
        }
        let payload = &bytes[pos..pos + len];
        match fourcc {
            b"VP8X" => {
                if payload.len() < 10 {
                    return Err(ImageError::InvalidHeader("truncated VP8X chunk"));
                }
                if payload[0] & FLAG_ANIMATION != 0 {
                    return Err(ImageError::UnsupportedFormat(
                        "animated WebP is not supported",
                    ));
                }
                let width = read_u24(&payload[4..7]) + 1;
                let height = read_u24(&payload[7..10]) + 1;
                validate_dimensions(width, height)?;
                chunks.canvas.get_or_insert((width, height));
            }
            b"ANIM" | b"ANMF" => {
                return Err(ImageError::UnsupportedFormat(
                    "animated WebP is not supported",
                ));
            }
            b"VP8L" => {
                chunks.vp8l.get_or_insert(payload);
            }
            b"VP8 " => {
                chunks.vp8.get_or_insert(payload);
            }
            b"ALPH" => {
                chunks.alpha.get_or_insert(payload);
            }
            b"RGBA" => {
                chunks.rgba.get_or_insert(payload);
            }
            _ => {}
        }
        pos += len + (len & 1);
    }
    Ok(chunks)
}

/// Check that a bitstream's size matches the `VP8X` canvas, when there is one.
fn check_canvas(canvas: Option<(u32, u32)>, width: u32, height: u32) -> Result<(), ImageError> {
    match canvas {
        Some(size) if size != (width, height) => Err(ImageError::InvalidHeader(
            "WebP bitstream size differs from the VP8X canvas",
        )),
        _ => Ok(()),
    }
}

/// Expand ARGB words (as `VP8L` stores them) to RGBA bytes.
fn argb_to_image(width: u32, height: u32, argb: &[u32]) -> Result<Image, ImageError> {
    let mut pixels = Vec::with_capacity(argb.len() * 4);
    for &p in argb {
        pixels.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
    }
    Image::from_rgba(width, height, pixels)
}

/// Decode an `ALPH` chunk into one alpha byte per pixel.
///
/// The first byte holds the compression method (bits 0-1), the prediction
/// filter (bits 2-3), and an informational preprocessing hint (bits 4-5),
/// which is ignored. Method 0 stores `width * height` raw bytes; method 1 is
/// a header-less lossless image stream whose green channel carries alpha.
fn decode_alpha(payload: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ImageError> {
    let (&info, data) = payload.split_first().ok_or(ImageError::UnexpectedEof)?;
    let count = width as usize * height as usize;
    let mut alpha = match info & 3 {
        0 => data.get(..count).ok_or(ImageError::UnexpectedEof)?.to_vec(),
        1 => vp8l::decode_headerless(data, width, height)?
            .iter()
            .map(|&p| (p >> 8) as u8)
            .collect(),
        _ => return Err(ImageError::InvalidHeader("unknown ALPH compression method")),
    };
    unfilter_alpha(&mut alpha, width as usize, height as usize, (info >> 2) & 3);
    Ok(alpha)
}

/// Undo the `ALPH` prediction filter in place.
///
/// Filter 1 predicts from the left, 2 from above, and 3 from the clamped
/// gradient `left + above - above_left`. The first pixel predicts 0, the rest
/// of the top row predicts from the left, and the first column (filters 1
/// and 3) predicts from above.
pub(crate) fn unfilter_alpha(alpha: &mut [u8], width: usize, height: usize, filter: u8) {
    if filter == 0 || width == 0 {
        return;
    }
    for x in 1..width {
        alpha[x] = alpha[x].wrapping_add(alpha[x - 1]);
    }
    for y in 1..height {
        let row = y * width;
        for x in 0..width {
            let i = row + x;
            let above = alpha[i - width];
            let prediction = if x == 0 {
                above
            } else {
                match filter {
                    1 => alpha[i - 1],
                    2 => above,
                    _ => {
                        let left = i32::from(alpha[i - 1]);
                        let corner = i32::from(alpha[i - width - 1]);
                        (left + i32::from(above) - corner).clamp(0, 255) as u8
                    }
                }
            };
            alpha[i] = alpha[i].wrapping_add(prediction);
        }
    }
}

/// Decode a RIFF `WEBP` file. See the module notes for what is accepted.
pub(crate) fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    let chunks = walk(bytes)?;

    if let Some(payload) = chunks.vp8l {
        let (width, height, argb) = vp8l::decode(payload)?;
        check_canvas(chunks.canvas, width, height)?;
        return argb_to_image(width, height, &argb);
    }
    if let Some(payload) = chunks.vp8 {
        let frame = vp8::decode(payload)?;
        check_canvas(chunks.canvas, frame.width, frame.height)?;
        let mut rgba = frame.to_rgba();
        if let Some(alpha_chunk) = chunks.alpha {
            let alpha = decode_alpha(alpha_chunk, frame.width, frame.height)?;
            for (pixel, &a) in rgba.chunks_mut(4).zip(alpha.iter()) {
                pixel[3] = a;
            }
        }
        return Image::from_rgba(frame.width, frame.height, rgba);
    }
    if let (Some(compressed), Some((width, height))) = (chunks.rgba, chunks.canvas) {
        let expected = (width as usize) * (height as usize) * 4;
        let pixels = decompress_zlib_bounded(compressed, expected)?;
        return Image::from_rgba(width, height, pixels);
    }
    Err(ImageError::UnsupportedFormat(
        "WebP file has no image chunk",
    ))
}

#[cfg(test)]
mod tests;
