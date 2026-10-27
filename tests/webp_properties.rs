// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato
//! Exact-pixel and property checks for the WebP decoders.
//!
//! Lossless files and `ALPH` chunks come from the writer in
//! `support/webp_writer.rs`, so each case has a known result and the decoded
//! pixels must match it byte for byte. Lossy frames come from the fixtures.
//! Each case runs on its own thread with a time limit, like the robustness
//! tests.

use anvil_image::{ImageError, decode_auto, decode_webp};

mod webp_fixtures {
    include!("fixtures/webp.rs");
}

#[allow(dead_code)]
mod webp_writer {
    include!("support/webp_writer.rs");
}

use webp_writer::{Chooser, alph, chunk, riff, vp8l, vp8x};

// ---- shared harness (same text in each repo's robustness test) ----

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Wall-clock budget for one robustness test. Cases run until it is spent.
/// Set `ANVIL_ROBUSTNESS_MS` for a longer local soak run.
fn test_budget() -> Duration {
    let millis = std::env::var("ANVIL_ROBUSTNESS_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1500);
    Duration::from_millis(millis)
}

/// One input that runs longer than this counts as a hang.
const CASE_LIMIT: Duration = Duration::from_secs(10);

/// Upper bound on cases per test, so a fast machine does not loop forever.
const MAX_CASES: usize = 200_000;

/// xorshift64 generator. A fixed seed gives the same inputs on every run.
struct Rng(u64);

#[allow(dead_code)] // Not every test file uses every helper.
impl Rng {
    fn new(seed: u64) -> Self {
        Rng((seed ^ 0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Value in `0..n`. Returns 0 when `n` is 0.
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    fn byte(&mut self) -> u8 {
        self.next_u64() as u8
    }

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }

    fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        let len = self.below(max_len + 1);
        (0..len).map(|_| self.byte()).collect()
    }
}

/// Apply 1 to 8 random edits: bit flips, byte overwrites, boundary values,
/// inserts, deletes, duplicated runs, and truncation.
fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut out = seed.to_vec();
    let edits = 1 + rng.below(8);
    for _ in 0..edits {
        let len = out.len();
        match rng.below(8) {
            0 if len > 0 => {
                let i = rng.below(len);
                out[i] ^= 1 << rng.below(8);
            }
            1 if len > 0 => {
                let i = rng.below(len);
                out[i] = rng.byte();
            }
            2 if len > 0 => {
                let i = rng.below(len);
                out[i] = rng.pick(&[0x00, 0x01, 0x7F, 0x80, 0xFE, 0xFF]);
            }
            3 if len >= 4 => {
                let i = rng.below(len - 3);
                let v = rng.pick(&[0u32, 1, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0xFFFF]);
                out[i..i + 4].copy_from_slice(&v.to_be_bytes());
            }
            4 => {
                let i = rng.below(len + 1);
                let extra = rng.bytes(16);
                out.splice(i..i, extra);
            }
            5 if len > 0 => {
                let i = rng.below(len);
                let n = 1 + rng.below((len - i).min(32));
                out.drain(i..i + n);
            }
            6 if len > 0 => {
                let i = rng.below(len);
                let n = 1 + rng.below((len - i).min(64));
                let run = out[i..i + n].to_vec();
                let at = rng.below(len + 1);
                out.splice(at..at, run);
            }
            _ => {
                let keep = rng.below(len + 1);
                out.truncate(keep);
            }
        }
    }
    out
}

fn preview(input: &[u8]) -> String {
    let shown: Vec<String> = input.iter().take(96).map(|b| format!("{b:02x}")).collect();
    let more = if input.len() > 96 { " ..." } else { "" };
    format!("{} bytes: {}{}", input.len(), shown.join(" "), more)
}

/// Run `target` on `input` in its own thread. Fails the test on a panic or
/// when the case runs past [`CASE_LIMIT`]. A hung thread is left behind and
/// ends with the test process.
fn check_case(label: &str, case: usize, input: Vec<u8>, target: fn(&[u8])) {
    let (done_tx, done_rx) = mpsc::channel();
    let shared = std::sync::Arc::new(input);
    let worker_input = shared.clone();
    let handle = thread::Builder::new()
        .name(format!("{label}-{case}"))
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            target(&worker_input);
            let _ = done_tx.send(());
        })
        .expect("spawn case thread");
    match done_rx.recv_timeout(CASE_LIMIT) {
        Ok(()) => {
            let _ = handle.join();
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = handle.join();
            panic!(
                "{label}: case {case} panicked on input {}",
                preview(&shared)
            );
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!(
                "{label}: case {case} did not finish within {:?} on input {}",
                CASE_LIMIT,
                preview(&shared)
            );
        }
    }
}

/// Generate cases from `seed` until the budget is spent and check each one.
fn run_cases(
    label: &str,
    seed: u64,
    mut generate: impl FnMut(&mut Rng) -> Vec<u8>,
    target: fn(&[u8]),
) {
    let mut rng = Rng::new(seed);
    let budget = test_budget();
    let started = Instant::now();
    let mut case = 0;
    while case < MAX_CASES && started.elapsed() < budget {
        let input = generate(&mut rng);
        check_case(label, case, input, target);
        case += 1;
    }
    eprintln!("{label}: {case} cases in {:?}", started.elapsed());
}

impl Chooser for Rng {
    fn choose(&mut self, n: usize) -> usize {
        self.below(n)
    }
}

/// Run `target` on seeds `0, 1, 2, ...` until the budget is spent. The
/// target builds its case from the seed, so a failure names the seed.
fn run_seeds(label: &str, salt: u64, target: fn(&[u8])) {
    let budget = test_budget();
    let started = Instant::now();
    let mut case = 0;
    while case < MAX_CASES && started.elapsed() < budget {
        let seed = (salt << 32) | case as u64;
        check_case(label, case, seed.to_le_bytes().to_vec(), target);
        case += 1;
    }
    eprintln!("{label}: {case} cases in {:?}", started.elapsed());
}

fn seed_of(input: &[u8]) -> u64 {
    u64::from_le_bytes(input.try_into().expect("8-byte seed"))
}

/// Random RGBA pixels: a few colors in runs and repeated rows (so copies
/// and the color cache get used), or plain noise.
fn random_pixels(rng: &mut Rng, width: u32, height: u32) -> Vec<u8> {
    let count = width as usize * height as usize;
    let mut out = Vec::with_capacity(count * 4);
    if rng.one_in(4) {
        for _ in 0..count * 4 {
            out.push(rng.byte());
        }
        return out;
    }
    let colors: Vec<[u8; 4]> = (0..1 + rng.below(6))
        .map(|_| {
            let any = rng.byte();
            let alpha = rng.pick(&[255, 255, 0, any]);
            [rng.byte(), rng.byte(), rng.byte(), alpha]
        })
        .collect();
    let row = width as usize * 4;
    while out.len() < count * 4 {
        if out.len() >= row && rng.one_in(3) {
            // Repeat the row above.
            let start = out.len() - row;
            let take = row.min(count * 4 - out.len());
            let copy = out[start..start + take].to_vec();
            out.extend_from_slice(&copy);
            continue;
        }
        let color = rng.pick(&colors);
        for _ in 0..1 + rng.below(12) {
            if out.len() < count * 4 {
                out.extend_from_slice(&color);
            }
        }
    }
    out
}

/// Side lengths: mostly small, some odd and wide, a few at the edges of
/// the 14-bit field and the decoder's caps.
fn random_size(rng: &mut Rng) -> (u32, u32) {
    match rng.below(10) {
        0 => (1, 1 + rng.below(300) as u32),
        1 => (1 + rng.below(300) as u32, 1),
        2 => (1 + rng.below(700) as u32, 1 + rng.below(3) as u32),
        _ => (1 + rng.below(48) as u32, 1 + rng.below(48) as u32),
    }
}

/// A lossless file with known pixels, in a simple or an extended container
/// with metadata and unknown chunks (odd lengths, so pad bytes too).
fn lossless_case(rng: &mut Rng) -> (Vec<u8>, u32, u32, Vec<u8>) {
    let (width, height) = random_size(rng);
    let rgba = random_pixels(rng, width, height);
    let subtract_green = rng.one_in(2);
    let cache_bits = if rng.one_in(2) {
        None
    } else {
        Some(1 + rng.below(11) as u32)
    };
    let stream = vp8l(width, height, &rgba, subtract_green, cache_bits, rng);
    let file = if rng.one_in(2) {
        riff(&[chunk(b"VP8L", &stream)])
    } else {
        let alpha = rgba.chunks(4).any(|p| p[3] != 255);
        let mut chunks = vec![vp8x(width, height, alpha, 0)];
        for _ in 0..rng.below(3) {
            let tag = rng.pick(&[b"ICCP", b"EXIF", b"XMP ", b"Zzzz"]);
            let payload = rng.bytes(9);
            chunks.push(chunk(tag, &payload));
        }
        let at = rng.below(chunks.len()) + 1;
        chunks.insert(at, chunk(b"VP8L", &stream));
        if rng.one_in(2) {
            chunks.push(chunk(b"EXIF", &rng.bytes(5)));
        }
        riff(&chunks)
    };
    (file, width, height, rgba)
}

fn generated_lossless_matches(input: &[u8]) {
    let mut rng = Rng::new(seed_of(input));
    let (file, width, height, rgba) = lossless_case(&mut rng);
    let image = decode_webp(&file).unwrap_or_else(|e| panic!("{width}x{height}: {e}"));
    assert_eq!((image.width, image.height), (width, height));
    if image.pixels != rgba {
        let first = image
            .pixels
            .iter()
            .zip(&rgba)
            .position(|(a, b)| a != b)
            .unwrap_or(0)
            / 4;
        panic!(
            "{width}x{height}: pixel {} is {:?}, want {:?}",
            first,
            &image.pixels[first * 4..first * 4 + 4],
            &rgba[first * 4..first * 4 + 4]
        );
    }
    // decode_auto picks the same decoder.
    assert_eq!(decode_auto(&file).expect("decode_auto"), image);
}

#[test]
fn generated_lossless_files_decode_exactly() {
    run_seeds("webp-lossless-exact", 1, generated_lossless_matches);
}

/// Mutate generated lossless files. The decoder must return an error or an
/// image of the declared size, never panic or hang.
#[test]
fn mutated_generated_lossless_files_never_panic() {
    run_cases(
        "webp-lossless-mutated",
        2,
        |rng| {
            let (file, ..) = lossless_case(rng);
            mutate(rng, &file)
        },
        |input| {
            if let Ok(image) = decode_webp(input) {
                assert_eq!(
                    image.pixels.len(),
                    image.width as usize * image.height as usize * 4
                );
            }
        },
    );
}

/// The lossy fixtures, as (file, VP8 payload, width, height).
fn lossy_frames() -> Vec<(&'static [u8], &'static [u8], u32, u32)> {
    [
        webp_fixtures::LOSSY_ALL_MODES,
        webp_fixtures::LOSSY_SIMPLE_FILTER,
        webp_fixtures::LOSSY_ALPHA_GRADIENT,
        webp_fixtures::LOSSY_ALPHA_VERTICAL,
    ]
    .iter()
    .map(|file| {
        let file: &'static [u8] = file;
        let at = file
            .windows(4)
            .position(|w| w == b"VP8 ")
            .expect("VP8 chunk");
        let len = u32::from_le_bytes(file[at + 4..at + 8].try_into().unwrap()) as usize;
        let frame = &file[at + 8..at + 8 + len];
        let width = u32::from(u16::from_le_bytes([frame[6], frame[7]]) & 0x3fff);
        let height = u32::from(u16::from_le_bytes([frame[8], frame[9]]) & 0x3fff);
        (file, frame, width, height)
    })
    .collect()
}

fn alpha_case_matches(input: &[u8]) {
    let mut rng = Rng::new(seed_of(input));
    let frames = lossy_frames();
    let (_, frame, width, height) = frames[rng.below(frames.len())];
    let opaque = decode_webp(&riff(&[chunk(b"VP8 ", frame)])).expect("plain lossy frame");
    let count = (width * height) as usize;
    let plane: Vec<u8> = match rng.below(4) {
        0 => (0..count).map(|_| rng.byte()).collect(),
        1 => (0..count)
            .map(|i| ((i % width as usize) * 5 + (i / width as usize) * 3) as u8)
            .collect(),
        2 => vec![rng.byte(); count],
        _ => random_pixels(&mut rng, width, height)
            .chunks(4)
            .map(|p| p[1])
            .collect(),
    };
    let filter = rng.below(4) as u8;
    let lossless = rng.one_in(2);
    let payload = alph(&plane, width, height, filter, lossless, &mut rng);
    let file = riff(&[
        vp8x(width, height, true, 0),
        chunk(b"ALPH", &payload),
        chunk(b"VP8 ", frame),
    ]);
    let image = decode_webp(&file)
        .unwrap_or_else(|e| panic!("{width}x{height} filter {filter} lossless {lossless}: {e}"));
    assert_eq!((image.width, image.height), (width, height));
    for (i, (got, want)) in image
        .pixels
        .chunks(4)
        .zip(opaque.pixels.chunks(4))
        .enumerate()
    {
        assert_eq!(got[..3], want[..3], "color at pixel {i} changed with ALPH");
        assert_eq!(
            got[3], plane[i],
            "alpha at pixel {i} (filter {filter}, lossless {lossless})"
        );
    }
}

/// `ALPH` with each filter, raw and lossless, on every lossy fixture: the
/// alpha matches exactly and the color is the same as without the chunk.
#[test]
fn alph_chunks_decode_exactly_with_every_filter() {
    run_seeds("webp-alph-exact", 3, alpha_case_matches);
}

/// A lossy frame decodes to the same pixels in a simple container and in an
/// extended one with metadata and unknown chunks around it.
#[test]
fn lossy_frames_decode_the_same_in_any_container() {
    for (file, frame, width, height) in lossy_frames() {
        let simple = decode_webp(&riff(&[chunk(b"VP8 ", frame)])).expect("simple");
        let extended = riff(&[
            vp8x(width, height, false, 0x08),
            chunk(b"ICCP", &[1, 2, 3]),
            chunk(b"Zzzz", &[]),
            chunk(b"VP8 ", frame),
            chunk(b"EXIF", &[7; 11]),
        ]);
        let image = decode_webp(&extended).expect("extended");
        assert_eq!(image.pixels.len(), simple.pixels.len());
        for (a, b) in image.pixels.chunks(4).zip(simple.pixels.chunks(4)) {
            assert_eq!(a[..3], b[..3]);
        }
        // The fixture itself (which may carry ALPH) has the same color.
        let original = decode_webp(file).expect("fixture");
        for (a, b) in original.pixels.chunks(4).zip(simple.pixels.chunks(4)) {
            assert_eq!(a[..3], b[..3]);
        }
    }
}

/// A `VP8X` canvas that does not match the bitstream is refused.
#[test]
fn canvas_size_must_match_the_bitstream() {
    let mut rng = Rng::new(4);
    let rgba = random_pixels(&mut rng, 5, 4);
    let stream = vp8l(5, 4, &rgba, false, None, &mut rng);
    for (w, h) in [(6, 4), (5, 3), (1, 1), (4096, 4096)] {
        let file = riff(&[vp8x(w, h, true, 0), chunk(b"VP8L", &stream)]);
        assert!(
            decode_webp(&file).is_err(),
            "canvas {w}x{h} for a 5x4 image"
        );
    }
    for (_, frame, width, height) in lossy_frames() {
        let file = riff(&[vp8x(width + 1, height, false, 0), chunk(b"VP8 ", frame)]);
        assert!(decode_webp(&file).is_err());
    }
}

/// Sizes at the caps: 8192 wide or tall decodes exactly, 8193 is refused,
/// and a 4096 by 4096 image (exactly the pixel cap) made of long copies
/// decodes. 4097 by 4096 is over the pixel cap.
#[test]
fn lossless_sizes_at_the_caps() {
    let mut rng = Rng::new(5);
    for (w, h) in [(8192, 1), (1, 8192), (8192, 2)] {
        let rgba = random_pixels(&mut rng, w, h);
        let stream = vp8l(w, h, &rgba, rng.one_in(2), Some(10), &mut rng);
        let image = decode_webp(&riff(&[chunk(b"VP8L", &stream)])).expect("at the side cap");
        assert!(image.pixels == rgba, "{w}x{h}");
    }
    for (w, h) in [(8193, 1), (1, 8193), (4097, 4096)] {
        let rgba = vec![9u8; (w * h * 4) as usize];
        let stream = vp8l(w, h, &rgba, false, None, &mut rng);
        assert!(
            matches!(
                decode_webp(&riff(&[chunk(b"VP8L", &stream)])),
                Err(ImageError::DimensionsTooLarge { .. })
            ),
            "{w}x{h}"
        );
    }
    let rgba: Vec<u8> = [12u8, 34, 56, 78].repeat(4096 * 4096);
    let stream = vp8l(4096, 4096, &rgba, false, None, &mut rng);
    let image = decode_webp(&riff(&[chunk(b"VP8L", &stream)])).expect("at the pixel cap");
    assert!(image.pixels == rgba);
}

/// Copies are checked against the pixels decoded so far and the end of the
/// image. In-range copies (overlapping ones too) give the exact pixels.
#[test]
fn copies_must_stay_inside_the_image() {
    use webp_writer::vp8l_one_copy;
    let decode =
        |w, h, len, dist| decode_webp(&riff(&[chunk(b"VP8L", &vp8l_one_copy(w, h, len, dist))]));
    let solid = |w: u32, h: u32| [8u8, 7, 9, 255].repeat((w * h) as usize);
    // Overlapping copy of the one pixel decoded so far.
    assert_eq!(
        decode(4, 1, 3, 1).expect("copy of 3 from 1 back").pixels,
        solid(4, 1)
    );
    assert_eq!(
        decode(4096, 1, 4095, 1).expect("longest copy").pixels,
        solid(4096, 1)
    );
    assert_eq!(
        decode(3, 3, 4, 1).expect("copy across rows").pixels,
        solid(3, 3)
    );
    // Before the first pixel.
    assert!(decode(4, 1, 1, 2).is_err(), "distance 2 after one pixel");
    assert!(
        decode(8, 8, 1, 1000).is_err(),
        "distance 1000 after one pixel"
    );
    // Past the end of the image.
    assert!(
        decode(4, 1, 4, 1).is_err(),
        "copy of 4 into 3 remaining pixels"
    );
    assert!(
        decode(2, 2, 4096, 1).is_err(),
        "copy of 4096 into a 2x2 image"
    );
}
