// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2026 Buntato
//! Tests for the WebP container, checked against files from a stock encoder
//! and against small streams built by hand.

use super::*;
use crate::{decode_auto, encode_webp};

mod fixtures {
    include!("../../tests/fixtures/webp.rs");
}
use fixtures::*;

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// Wrap chunks in a RIFF `WEBP` header, padding odd payloads.
fn riff(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    for (fourcc, payload) in chunks {
        body.extend_from_slice(*fourcc);
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// A `VP8X` payload with `flags` and a canvas of `width` by `height`.
fn vp8x(flags: u8, width: u32, height: u32) -> Vec<u8> {
    let mut p = vec![flags, 0, 0, 0];
    p.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    p.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    p
}

/// Find the first chunk named `fourcc` in a RIFF file.
fn chunk<'a>(file: &'a [u8], fourcc: &[u8; 4]) -> Option<&'a [u8]> {
    let mut pos = 12;
    while pos + 8 <= file.len() {
        let len = u32::from_le_bytes([file[pos + 4], file[pos + 5], file[pos + 6], file[pos + 7]])
            as usize;
        if &file[pos..pos + 4] == fourcc {
            return file.get(pos + 8..pos + 8 + len);
        }
        pos += 8 + len + (len & 1);
    }
    None
}

#[test]
fn lossless_files_match_the_reference_decoder() {
    let cases: [(&[u8], u64, u32, u32); 5] = [
        (
            LOSSLESS_PREDICT_CROSS_CACHE,
            LOSSLESS_PREDICT_CROSS_CACHE_RGBA_FNV,
            12,
            10,
        ),
        (LOSSLESS_PALETTE_2, LOSSLESS_PALETTE_2_RGBA_FNV, 13, 9),
        (LOSSLESS_PALETTE_11, LOSSLESS_PALETTE_11_RGBA_FNV, 13, 9),
        (
            LOSSLESS_SUBTRACT_GREEN,
            LOSSLESS_SUBTRACT_GREEN_RGBA_FNV,
            13,
            9,
        ),
        (LOSSLESS_META_CODES, LOSSLESS_META_CODES_RGBA_FNV, 24, 32),
    ];
    for (i, (file, hash, width, height)) in cases.into_iter().enumerate() {
        let image = decode_auto(file).unwrap_or_else(|e| panic!("case {i}: {e}"));
        assert_eq!((image.width, image.height), (width, height), "case {i}");
        assert_eq!(fnv1a(&image.pixels), hash, "case {i}");
    }
}

#[test]
fn encoder_output_still_decodes() {
    let mut image = Image::new(5, 4);
    image.set_pixel(1, 2, [1, 2, 3, 4]);
    image.set_pixel(4, 3, [250, 128, 0, 255]);
    assert_eq!(decode(&encode_webp(&image)).unwrap(), image);
}

#[test]
fn simple_lossless_file_without_vp8x_decodes() {
    // Drop nothing: the stock lossless files are already in the simple layout.
    assert!(chunk(LOSSLESS_PALETTE_2, b"VP8X").is_none());
    let payload = chunk(LOSSLESS_PALETTE_2, b"VP8L").unwrap();
    // The same bitstream behind a matching VP8X header decodes the same.
    let extended = riff(&[(b"VP8X", &vp8x(0, 13, 9)), (b"VP8L", payload)]);
    assert_eq!(
        decode(&extended).unwrap(),
        decode(LOSSLESS_PALETTE_2).unwrap()
    );
    // A canvas that disagrees with the bitstream is an error.
    let mismatched = riff(&[(b"VP8X", &vp8x(0, 14, 9)), (b"VP8L", payload)]);
    assert!(matches!(
        decode(&mismatched),
        Err(ImageError::InvalidHeader(_))
    ));
}

#[test]
fn animation_is_refused() {
    let payload = chunk(LOSSLESS_PALETTE_2, b"VP8L").unwrap();
    let flagged = riff(&[(b"VP8X", &vp8x(0x02, 13, 9)), (b"VP8L", payload)]);
    assert!(matches!(
        decode(&flagged),
        Err(ImageError::UnsupportedFormat(_))
    ));
    let anim = riff(&[
        (b"VP8X", &vp8x(0, 13, 9)),
        (b"ANIM", &[0; 6]),
        (b"VP8L", payload),
    ]);
    assert!(matches!(
        decode(&anim),
        Err(ImageError::UnsupportedFormat(_))
    ));
    let anmf = riff(&[(b"VP8X", &vp8x(0, 13, 9)), (b"ANMF", &[0; 16])]);
    assert!(matches!(
        decode(&anmf),
        Err(ImageError::UnsupportedFormat(_))
    ));
}

#[test]
fn container_errors_are_reported() {
    assert!(matches!(
        decode(b"RIFF\0\0\0\0WEBQ"),
        Err(ImageError::InvalidHeader(_))
    ));
    assert!(matches!(decode(b"RIFF"), Err(ImageError::InvalidHeader(_))));
    // Metadata only: nothing to decode.
    let meta = riff(&[(b"VP8X", &vp8x(0x08, 2, 2)), (b"EXIF", b"abc")]);
    assert!(matches!(
        decode(&meta),
        Err(ImageError::UnsupportedFormat(_))
    ));
    // A chunk that claims more bytes than the file has.
    let mut short = riff(&[(b"VP8L", chunk(LOSSLESS_PALETTE_2, b"VP8L").unwrap())]);
    short.truncate(short.len() - 3);
    let riff_size = (short.len() - 8) as u32;
    short[4..8].copy_from_slice(&riff_size.to_le_bytes());
    assert_eq!(decode(&short), Err(ImageError::UnexpectedEof));
    // A truncated VP8X header.
    let tiny = riff(&[(b"VP8X", &[0u8; 6])]);
    assert!(matches!(decode(&tiny), Err(ImageError::InvalidHeader(_))));
    // A canvas over the size cap is refused before anything is allocated.
    let huge = riff(&[(b"VP8X", &vp8x(0, 1 << 24, 1 << 24))]);
    assert!(matches!(
        decode(&huge),
        Err(ImageError::DimensionsTooLarge { .. })
    ));
}

#[test]
fn bytes_past_the_riff_size_are_ignored() {
    let mut file = LOSSLESS_PALETTE_11.to_vec();
    file.extend_from_slice(b"VP8L\xFF\xFF\xFF\x7Fjunk after the RIFF payload");
    assert_eq!(decode(&file).unwrap(), decode(LOSSLESS_PALETTE_11).unwrap());
}

#[test]
fn unknown_chunks_are_skipped() {
    let payload = chunk(LOSSLESS_PALETTE_11, b"VP8L").unwrap();
    let file = riff(&[
        (b"VP8X", &vp8x(0x20, 13, 9)),
        (b"ICCP", b"profile"),
        (b"ABCD", b"odd"),
        (b"VP8L", payload),
        (b"XMP ", b"<x/>"),
    ]);
    assert_eq!(decode(&file).unwrap(), decode(LOSSLESS_PALETTE_11).unwrap());
}
