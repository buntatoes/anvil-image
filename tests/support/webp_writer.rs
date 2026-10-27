// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato
// Small WebP writer for the tests. It writes lossless (VP8L) bitstreams with
// literals, backward references, a color cache, and the subtract-green
// transform, raw and lossless ALPH chunks with each prediction filter, and
// RIFF containers with extra chunks. Every file it writes has a known
// pixel-exact result, so the decoders can be checked byte for byte.

/// Bits packed least significant first, as VP8L reads them.
pub struct Bits {
    out: Vec<u8>,
    acc: u64,
    count: u32,
}

impl Bits {
    pub fn new() -> Self {
        Bits {
            out: Vec::new(),
            acc: 0,
            count: 0,
        }
    }

    /// Append the low `count` bits of `value`, least significant first.
    pub fn put(&mut self, value: u32, count: u32) {
        debug_assert!(count <= 24);
        let mask = if count == 0 { 0 } else { (1u64 << count) - 1 };
        self.acc |= (u64::from(value) & mask) << self.count;
        self.count += count;
        while self.count >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.count -= 8;
        }
    }

    /// Append a prefix code. Codes are read most significant bit first.
    fn put_code(&mut self, code: u32, len: u8) {
        for i in (0..len).rev() {
            self.put((code >> i) & 1, 1);
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

/// Order the code length code lengths are stored in.
const CODE_LENGTH_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

/// A complete prefix code over every symbol of an alphabet, using two
/// neighbouring lengths so every symbol can be written.
struct Code {
    lengths: Vec<u8>,
    codes: Vec<u32>,
}

impl Code {
    fn full(size: usize) -> Code {
        let mut short = 0u32;
        while (1usize << (short + 1)) <= size {
            short += 1;
        }
        // `a` symbols get `short` bits and the rest `short + 1`, which fills
        // the code exactly: 2a + (size - a) = 2^(short + 1).
        let a = (1usize << (short + 1)) - size;
        let lengths: Vec<u8> = (0..size)
            .map(|s| if s < a { short as u8 } else { short as u8 + 1 })
            .collect();
        // Canonical codes: by length, then by symbol.
        let mut codes = vec![0u32; size];
        let mut next = 0u32;
        let mut prev_len = 0u8;
        let mut order: Vec<usize> = (0..size).collect();
        order.sort_by_key(|&s| (lengths[s], s));
        for s in order {
            next <<= lengths[s] - prev_len;
            prev_len = lengths[s];
            codes[s] = next;
            next += 1;
        }
        Code { lengths, codes }
    }

    /// Write the code as a normal (not simple) prefix code.
    fn write_header(&self, bits: &mut Bits) {
        let short = self.lengths[0];
        let long_used = self.lengths.iter().any(|&l| l != short);
        // Two code length symbols of one bit each. The smaller symbol gets 0.
        let (zero, one) = if long_used {
            (short, short + 1)
        } else {
            (0, short)
        };
        bits.put(0, 1); // normal code
        bits.put(15, 4); // all 19 code length code lengths follow
        for &symbol in &CODE_LENGTH_ORDER {
            let used = symbol == usize::from(zero) || symbol == usize::from(one);
            bits.put(u32::from(used), 3);
        }
        bits.put(0, 1); // lengths for the whole alphabet
        for &len in &self.lengths {
            bits.put(u32::from(len != zero), 1);
        }
    }

    fn put(&self, bits: &mut Bits, symbol: usize) {
        bits.put_code(self.codes[symbol], self.lengths[symbol]);
    }
}

/// Prefix and extra bits for a length or distance value of at least 1.
fn prefix(value: usize) -> (usize, u32, u32) {
    let d = (value - 1) as u32;
    if d < 4 {
        return (d as usize, 0, 0);
    }
    let high = 31 - d.leading_zeros();
    let second = (d >> (high - 1)) & 1;
    let extra_bits = high - 1;
    (
        (2 * high + second) as usize,
        extra_bits,
        d & ((1 << extra_bits) - 1),
    )
}

fn cache_index(argb: u32, cache_bits: u32) -> usize {
    (0x1e35_a7bd_u32.wrapping_mul(argb) >> (32 - cache_bits)) as usize
}

/// Choices the entropy coder makes. `choose(n)` returns a value in `0..n`.
pub trait Chooser {
    fn choose(&mut self, n: usize) -> usize;
}

/// Write the entropy-coded image: color cache bits, the meta code bit, five
/// prefix codes, then the pixels as literals, cache hits, and copies.
///
/// `argb` is the image after any transforms. Copies use linear distances
/// only (plane codes above 120).
pub fn write_argb_stream(
    bits: &mut Bits,
    argb: &[u32],
    cache_bits: Option<u32>,
    chooser: &mut dyn Chooser,
) {
    match cache_bits {
        Some(b) => {
            bits.put(1, 1);
            bits.put(b, 4);
        }
        None => bits.put(0, 1),
    }
    bits.put(0, 1); // one set of prefix codes for the whole image
    let cache_size = cache_bits.map_or(0, |b| 1usize << b);
    let green = Code::full(256 + 24 + cache_size);
    let red = Code::full(256);
    let blue = Code::full(256);
    let alpha = Code::full(256);
    let dist = Code::full(40);
    for code in [&green, &red, &blue, &alpha, &dist] {
        code.write_header(bits);
    }
    let mut cache = vec![0u32; cache_size];
    let insert = |cache: &mut Vec<u32>, px: u32| {
        if let Some(b) = cache_bits {
            cache[cache_index(px, b)] = px;
        }
    };
    let n = argb.len();
    let mut i = 0;
    while i < n {
        let px = argb[i];
        // Try a copy from a random earlier place that matches.
        if i > 0 && chooser.choose(3) != 0 {
            // Distances past about 2^20 have no prefix code.
            let reach = 64 + chooser.choose(i);
            let d = 1 + chooser.choose(i.min(reach).min(1_000_000));
            if argb[i - d] == px {
                let mut run = 1;
                while i + run < n && run < 4096 && argb[i + run] == argb[i + run - d] {
                    run += 1;
                }
                let len = 1 + chooser.choose(run);
                let (sym, nb, extra) = prefix(len);
                green.put(bits, 256 + sym);
                bits.put(extra, nb);
                let (sym, nb, extra) = prefix(d + 120);
                dist.put(bits, sym);
                bits.put(extra, nb);
                for k in 0..len {
                    insert(&mut cache, argb[i + k]);
                }
                i += len;
                continue;
            }
        }
        if let Some(b) = cache_bits {
            let idx = cache_index(px, b);
            if cache[idx] == px && chooser.choose(2) == 0 {
                green.put(bits, 280 + idx);
                i += 1;
                continue;
            }
        }
        green.put(bits, ((px >> 8) & 0xff) as usize);
        red.put(bits, ((px >> 16) & 0xff) as usize);
        blue.put(bits, (px & 0xff) as usize);
        alpha.put(bits, (px >> 24) as usize);
        insert(&mut cache, px);
        i += 1;
    }
}

/// A VP8L bitstream (header included) for `rgba`.
pub fn vp8l(
    width: u32,
    height: u32,
    rgba: &[u8],
    subtract_green: bool,
    cache_bits: Option<u32>,
    chooser: &mut dyn Chooser,
) -> Vec<u8> {
    let mut bits = Bits::new();
    bits.put(0x2f, 8);
    bits.put(width - 1, 14);
    bits.put(height - 1, 14);
    let has_alpha = rgba.chunks(4).any(|p| p[3] != 255);
    bits.put(u32::from(has_alpha), 1);
    bits.put(0, 3);
    if subtract_green {
        bits.put(1, 1);
        bits.put(2, 2);
    }
    bits.put(0, 1); // no more transforms
    let argb: Vec<u32> = rgba
        .chunks(4)
        .map(|p| {
            let (mut r, g, mut b) = (p[0], p[1], p[2]);
            if subtract_green {
                r = r.wrapping_sub(g);
                b = b.wrapping_sub(g);
            }
            (u32::from(p[3]) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
        })
        .collect();
    write_argb_stream(&mut bits, &argb, cache_bits, chooser);
    bits.finish()
}

/// One RIFF chunk with its pad byte.
pub fn chunk(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = fourcc.to_vec();
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// `RIFF`, the size, `WEBP`, then `chunks` as given.
pub fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + body.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

/// A `VP8X` chunk for a still image of the given canvas size.
pub fn vp8x(width: u32, height: u32, alpha: bool, extra_flags: u8) -> Vec<u8> {
    let mut payload = vec![extra_flags | if alpha { 0x10 } else { 0 }, 0, 0, 0];
    payload.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    payload.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    chunk(b"VP8X", &payload)
}

/// Predictor for an `ALPH` filter at (x, y), from the unfiltered plane.
fn alpha_predictor(plane: &[u8], width: usize, x: usize, y: usize, filter: u8) -> u8 {
    let at = |x: usize, y: usize| plane[y * width + x];
    if x == 0 && y == 0 {
        return 0;
    }
    if y == 0 {
        return at(x - 1, 0);
    }
    if x == 0 {
        return at(0, y - 1);
    }
    match filter {
        1 => at(x - 1, y),
        2 => at(x, y - 1),
        _ => {
            let g = i32::from(at(x - 1, y)) + i32::from(at(x, y - 1)) - i32::from(at(x - 1, y - 1));
            g.clamp(0, 255) as u8
        }
    }
}

/// An `ALPH` chunk payload: `filter` 0 to 3, raw or lossless.
pub fn alph(
    plane: &[u8],
    width: u32,
    height: u32,
    filter: u8,
    lossless: bool,
    chooser: &mut dyn Chooser,
) -> Vec<u8> {
    let w = width as usize;
    let filtered: Vec<u8> = if filter == 0 {
        plane.to_vec()
    } else {
        (0..height as usize)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| plane[y * w + x].wrapping_sub(alpha_predictor(plane, w, x, y, filter)))
            .collect()
    };
    let mut out = vec![(filter << 2) | u8::from(lossless)];
    if lossless {
        // The alpha values ride in the green channel, with no VP8L header.
        let mut bits = Bits::new();
        bits.put(0, 1); // no transforms
        let argb: Vec<u32> = filtered
            .iter()
            .map(|&a| 0xff00_0000 | (u32::from(a) << 8))
            .collect();
        let cache = if chooser.choose(2) == 0 {
            None
        } else {
            Some(1 + chooser.choose(11) as u32)
        };
        write_argb_stream(&mut bits, &argb, cache, chooser);
        out.extend_from_slice(&bits.finish());
    } else {
        out.extend_from_slice(&filtered);
    }
    out
}

/// A VP8L bitstream of one opaque literal pixel followed by one copy of
/// `len` pixels from `dist` back, with no check that the copy fits. Pixels
/// after the copy are literals of the same color. Each pixel is RGBA
/// `[8, 7, 9, 255]`.
pub fn vp8l_one_copy(width: u32, height: u32, len: usize, dist: usize) -> Vec<u8> {
    let mut bits = Bits::new();
    bits.put(0x2f, 8);
    bits.put(width - 1, 14);
    bits.put(height - 1, 14);
    bits.put(0, 1);
    bits.put(0, 3);
    bits.put(0, 1); // no transforms
    bits.put(0, 1); // no color cache
    bits.put(0, 1); // one set of prefix codes
    let green = Code::full(280);
    let others = Code::full(256);
    let dist_code = Code::full(40);
    green.write_header(&mut bits);
    for _ in 0..3 {
        others.write_header(&mut bits);
    }
    dist_code.write_header(&mut bits);
    let literal = |bits: &mut Bits| {
        green.put(bits, 7);
        others.put(bits, 8);
        others.put(bits, 9);
        others.put(bits, 255);
    };
    literal(&mut bits);
    let (sym, nb, extra) = prefix(len);
    green.put(&mut bits, 256 + sym);
    bits.put(extra, nb);
    let (sym, nb, extra) = prefix(dist + 120);
    dist_code.put(&mut bits, sym);
    bits.put(extra, nb);
    let total = width as usize * height as usize;
    for _ in (1 + len).min(total)..total {
        literal(&mut bits);
    }
    bits.finish()
}
