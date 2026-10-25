// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2026 Buntato
//! Robustness checks for the still-image decoders.
//!
//! Inputs come from a seeded generator, so a failure names a case number
//! that reproduces on every run. Each test stops after a short time budget.
//! A decoder may return an error for any of these inputs. It must not panic
//! or hang, and an image it returns must respect the size caps.

use anvil_image::{
    Image, MAX_IMAGE_DIMENSION, MAX_IMAGE_PIXELS, decode_auto, decode_bmp, decode_gif, decode_ico,
    decode_jpeg, decode_png, decode_ppm, decode_webp, encode_bmp, encode_gif, encode_ico,
    encode_jpeg, encode_png, encode_ppm, encode_webp,
};

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
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
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
            panic!("{label}: case {case} panicked on input {}", preview(&shared));
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
fn run_cases(label: &str, seed: u64, mut generate: impl FnMut(&mut Rng) -> Vec<u8>, target: fn(&[u8])) {
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

type Encoder = fn(&Image) -> Vec<u8>;
type Decoder = fn(&[u8]) -> Result<Image, anvil_image::ImageError>;

const PNG: (Encoder, Decoder) = (encode_png, decode_png);
const JPEG: (Encoder, Decoder) = (encode_jpeg, decode_jpeg);
const GIF: (Encoder, Decoder) = (encode_gif, decode_gif);
const WEBP: (Encoder, Decoder) = (encode_webp, decode_webp);
const ICO: (Encoder, Decoder) = (encode_ico, decode_ico);
const BMP: (Encoder, Decoder) = (encode_bmp, decode_bmp);
const PPM: (Encoder, Decoder) = (encode_ppm, decode_ppm);

/// A small image with smooth gradients and a few random pixels.
fn sample_image(rng: &mut Rng) -> Image {
    let width = 1 + rng.below(24) as u32;
    let height = 1 + rng.below(24) as u32;
    let mut image = Image::new(width, height);
    let tint = rng.byte();
    for y in 0..height {
        for x in 0..width {
            let rgba = if rng.one_in(8) {
                [rng.byte(), rng.byte(), rng.byte(), rng.byte()]
            } else {
                [(x * 9) as u8, (y * 9) as u8, tint, 255]
            };
            image.set_pixel(x, y, rgba);
        }
    }
    image
}

fn check_image(image: &Image) {
    assert!(image.width <= MAX_IMAGE_DIMENSION && image.height <= MAX_IMAGE_DIMENSION);
    assert!(u64::from(image.width) * u64::from(image.height) <= MAX_IMAGE_PIXELS);
    assert_eq!(image.pixels.len(), image.width as usize * image.height as usize * 4);
}

/// Every decoder sees every input, so a header check that lets a foreign
/// format through still gets exercised.
fn decode_everything(input: &[u8]) {
    for decode in [
        decode_auto, decode_png, decode_jpeg, decode_gif, decode_webp, decode_ico, decode_bmp,
        decode_ppm,
    ] {
        if let Ok(image) = decode(input) {
            check_image(&image);
        }
    }
}

fn mutated_case(rng: &mut Rng, format: (Encoder, Decoder)) -> Vec<u8> {
    let encoded = (format.0)(&sample_image(rng));
    mutate(rng, &encoded)
}

#[test]
fn random_bytes_with_known_signatures_do_not_panic() {
    const SIGNATURES: &[&[u8]] = &[
        b"\x89PNG\r\n\x1a\n", b"\xFF\xD8", b"GIF89a", b"GIF87a", b"RIFF\0\0\0\0WEBP",
        b"\0\0\x01\0", b"\0\0\x02\0", b"BM", b"P6", b"P5", b"",
    ];
    run_cases("image-random", 21, |rng| {
        let mut input = rng.pick(SIGNATURES).to_vec();
        input.extend(rng.bytes(200));
        input
    }, decode_everything);
}

#[test]
fn mutated_png_does_not_panic() {
    run_cases("image-png", 22, |rng| mutated_case(rng, PNG), decode_everything);
}

#[test]
fn mutated_jpeg_does_not_panic() {
    run_cases("image-jpeg", 23, |rng| mutated_case(rng, JPEG), decode_everything);
}

#[test]
fn mutated_gif_does_not_panic() {
    run_cases("image-gif", 24, |rng| mutated_case(rng, GIF), decode_everything);
}

#[test]
fn mutated_webp_ico_bmp_ppm_do_not_panic() {
    run_cases("image-other", 25, |rng| {
        let format = rng.pick(&[WEBP, ICO, BMP, PPM]);
        mutated_case(rng, format)
    }, decode_everything);
}

#[test]
fn truncated_files_do_not_panic() {
    run_cases("image-truncated", 26, |rng| {
        let format = rng.pick(&[PNG, JPEG, GIF, WEBP, ICO, BMP, PPM]);
        let mut encoded = (format.0)(&sample_image(rng));
        let keep = rng.below(encoded.len());
        encoded.truncate(keep);
        encoded
    }, decode_everything);
}

#[test]
fn lossless_formats_round_trip() {
    run_cases("image-round-trip", 27, |rng| {
        // The test target is a plain fn, so the image travels as PNG bytes.
        encode_png(&sample_image(rng))
    }, |png| {
        let image = decode_png(png).expect("encoder output decodes");
        check_image(&image);
        let again = decode_png(&encode_png(&image)).expect("second round decodes");
        assert_eq!(again, image);
        let jpeg = decode_jpeg(&encode_jpeg(&image)).expect("JPEG encoder output decodes");
        assert_eq!((jpeg.width, jpeg.height), (image.width, image.height));
        let gif = decode_gif(&encode_gif(&image)).expect("GIF encoder output decodes");
        assert_eq!((gif.width, gif.height), (image.width, image.height));
    });
}

#[test]
fn oversized_headers_are_refused() {
    // A PNG header that claims 65535 x 65535 must be refused before any
    // buffer of that size is allocated.
    let mut png = encode_png(&Image::new(1, 1));
    png[16..20].copy_from_slice(&65535u32.to_be_bytes());
    png[20..24].copy_from_slice(&65535u32.to_be_bytes());
    let crc = png_crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    assert!(decode_png(&png).is_err());
    assert!(decode_auto(&png).is_err());
}

/// CRC-32 (IEEE, reflected) so the test can re-seal an edited PNG chunk
/// without a dev-dependency.
fn png_crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
