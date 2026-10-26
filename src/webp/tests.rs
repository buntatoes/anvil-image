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
    let encoded = encode_webp(&image);
    assert!(chunk(&encoded, b"VP8L").is_some());
    assert_eq!(decode(&encoded).unwrap(), image);
    // Legacy VP8X + zlib RGBA containers still decode.

    let legacy = crate::encode_webp_legacy_rgba(&image);
    assert_eq!(decode(&legacy).unwrap(), image);
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

/// Y, U, and V planes without macroblock padding, as a reference decoder writes them.
fn packed_planes(frame: &vp8::Frame) -> Vec<u8> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Vec::with_capacity(w * h + 2 * cw * ch);
    for y in 0..h {
        out.extend_from_slice(&frame.y[y * frame.y_stride..y * frame.y_stride + w]);
    }
    for plane in [&frame.u, &frame.v] {
        for y in 0..ch {
            out.extend_from_slice(&plane[y * frame.uv_stride..y * frame.uv_stride + cw]);
        }
    }
    out
}

/// A pixel position and the reference decoder's RGBA there.
type Spot = (u32, u32, [u8; 4]);

/// Every channel within one step of the reference decoder's RGBA.
fn check_spots(image: &Image, spots: &[Spot]) {
    for &(x, y, want) in spots {
        let got = image.pixel(x, y).unwrap();
        for c in 0..4 {
            let diff = (i32::from(got[c]) - i32::from(want[c])).abs();
            assert!(diff <= 1, "pixel {x},{y}: got {got:?}, reference {want:?}");
        }
    }
}

#[test]
fn lossy_planes_match_the_reference_decoder_exactly() {
    let cases: [(&[u8], u64, &[Spot]); 2] = [
        (
            LOSSY_ALL_MODES,
            LOSSY_ALL_MODES_YUV_FNV,
            LOSSY_ALL_MODES_SPOTS,
        ),
        (
            LOSSY_SIMPLE_FILTER,
            LOSSY_SIMPLE_FILTER_YUV_FNV,
            LOSSY_SIMPLE_FILTER_SPOTS,
        ),
    ];
    for (i, (file, hash, spots)) in cases.into_iter().enumerate() {
        let frame = vp8::decode(chunk(file, b"VP8 ").unwrap()).unwrap();
        assert_eq!(fnv1a(&packed_planes(&frame)), hash, "case {i}");
        let image = decode_auto(file).unwrap();
        check_spots(&image, spots);
        assert!(image.pixels.chunks(4).all(|p| p[3] == 255));
    }
}

#[test]
fn lossy_file_inside_vp8x_decodes() {
    let payload = chunk(LOSSY_SIMPLE_FILTER, b"VP8 ").unwrap();
    let file = riff(&[(b"VP8X", &vp8x(0, 40, 24)), (b"VP8 ", payload)]);
    assert_eq!(decode(&file).unwrap(), decode(LOSSY_SIMPLE_FILTER).unwrap());
    let wrong = riff(&[(b"VP8X", &vp8x(0, 40, 25)), (b"VP8 ", payload)]);
    assert!(matches!(decode(&wrong), Err(ImageError::InvalidHeader(_))));
}

#[test]
fn truncated_lossy_files_are_errors() {
    let payload = chunk(LOSSY_ALL_MODES, b"VP8 ").unwrap();
    for keep in [0, 5, 9, 10, 40, payload.len() / 2, payload.len() - 20] {
        let file = riff(&[(b"VP8 ", &payload[..keep])]);
        assert!(decode(&file).is_err(), "kept {keep} bytes");
    }
}

#[test]
fn lossy_alpha_matches_the_reference_decoder() {
    let cases: [(&[u8], u64, u64, &[Spot]); 2] = [
        (
            LOSSY_ALPHA_GRADIENT,
            LOSSY_ALPHA_GRADIENT_YUV_FNV,
            LOSSY_ALPHA_GRADIENT_ALPHA_FNV,
            LOSSY_ALPHA_GRADIENT_SPOTS,
        ),
        (
            LOSSY_ALPHA_VERTICAL,
            LOSSY_ALPHA_VERTICAL_YUV_FNV,
            LOSSY_ALPHA_VERTICAL_ALPHA_FNV,
            LOSSY_ALPHA_VERTICAL_SPOTS,
        ),
    ];
    for (i, (file, yuv_hash, alpha_hash, spots)) in cases.into_iter().enumerate() {
        let frame = vp8::decode(chunk(file, b"VP8 ").unwrap()).unwrap();
        assert_eq!(fnv1a(&packed_planes(&frame)), yuv_hash, "case {i}");
        let image = decode_auto(file).unwrap();
        let alpha: Vec<u8> = image.pixels.chunks(4).map(|p| p[3]).collect();
        assert_eq!(fnv1a(&alpha), alpha_hash, "case {i}");
        check_spots(&image, spots);
    }
}

#[test]
fn alpha_filters_undo_their_predictions() {
    // 3x3 alpha plane 10, 20, 30 / 40, 50, 60 / 70, 80, 90.
    let plane: Vec<u8> = (1..=9).map(|v| v * 10).collect();
    // Horizontal: left neighbor, or the pixel above in the first column.
    let horizontal = [10u8, 10, 10, 30, 10, 10, 30, 10, 10];
    // Vertical: pixel above, or left neighbor in the first row.
    let vertical = [10u8, 10, 10, 30, 30, 30, 30, 30, 30];
    // Gradient: clamp(left + above - corner).
    let gradient = [10u8, 10, 10, 30, 0, 0, 30, 0, 0];
    for (filter, deltas) in [(1u8, horizontal), (2, vertical), (3, gradient)] {
        let mut data = deltas.to_vec();
        unfilter_alpha(&mut data, 3, 3, filter);
        assert_eq!(data, plane, "filter {filter}");
    }
    // Wrapping arithmetic, not saturating.
    let mut wrap = vec![200u8, 100];
    unfilter_alpha(&mut wrap, 2, 1, 1);
    assert_eq!(wrap, vec![200, 44]);
}

#[test]
fn raw_alpha_chunks_apply_with_each_filter() {
    let payload = chunk(LOSSY_SIMPLE_FILTER, b"VP8 ").unwrap();
    let (w, h) = (40u32, 24u32);
    let wanted: Vec<u8> = (0..w * h).map(|i| ((i * 7) % 256) as u8).collect();
    let base = decode(LOSSY_SIMPLE_FILTER).unwrap();
    for filter in 0..4u8 {
        // Build the filtered deltas by predicting from the wanted plane.
        let mut alph = vec![filter << 2];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = y * w as usize + x;
                let left = (x > 0).then(|| wanted[i - 1]);
                let above = (y > 0).then(|| wanted[i - w as usize]);
                let prediction = match (filter, left, above) {
                    (0, _, _) | (_, None, None) => 0,
                    (_, Some(l), None) => l,
                    (_, None, Some(a)) => a,
                    (1, Some(l), Some(_)) => l,
                    (2, Some(_), Some(a)) => a,
                    (_, Some(l), Some(a)) => {
                        let c = wanted[i - w as usize - 1];
                        (i32::from(l) + i32::from(a) - i32::from(c)).clamp(0, 255) as u8
                    }
                };
                alph.push(wanted[i].wrapping_sub(prediction));
            }
        }
        let file = riff(&[
            (b"VP8X", &vp8x(0x10, w, h)),
            (b"ALPH", &alph),
            (b"VP8 ", payload),
        ]);
        let image = decode(&file).unwrap();
        for (i, (got, plain)) in image
            .pixels
            .chunks(4)
            .zip(base.pixels.chunks(4))
            .enumerate()
        {
            assert_eq!(got[3], wanted[i], "filter {filter}, pixel {i}");
            assert_eq!(&got[..3], &plain[..3]);
        }
    }
}

#[test]
fn bad_alpha_chunks_are_errors() {
    let payload = chunk(LOSSY_SIMPLE_FILTER, b"VP8 ").unwrap();
    let short_raw = riff(&[
        (b"VP8X", &vp8x(0x10, 40, 24)),
        (b"ALPH", &[0, 1, 2, 3]),
        (b"VP8 ", payload),
    ]);
    assert_eq!(decode(&short_raw), Err(ImageError::UnexpectedEof));
    let empty = riff(&[
        (b"VP8X", &vp8x(0x10, 40, 24)),
        (b"ALPH", &[]),
        (b"VP8 ", payload),
    ]);
    assert_eq!(decode(&empty), Err(ImageError::UnexpectedEof));
    let method = riff(&[
        (b"VP8X", &vp8x(0x10, 40, 24)),
        (b"ALPH", &[2; 961]),
        (b"VP8 ", payload),
    ]);
    assert!(matches!(decode(&method), Err(ImageError::InvalidHeader(_))));
    let lossless = riff(&[
        (b"VP8X", &vp8x(0x10, 40, 24)),
        (b"ALPH", &[1, 0xFF, 0xFF]),
        (b"VP8 ", payload),
    ]);
    assert!(decode(&lossless).is_err());
}
