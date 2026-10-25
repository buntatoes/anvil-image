// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2026 Buntato
//! Lossless `VP8L` bitstream decoder.
//!
//! Follows the lossless part of the WebP spec: an LSB-first bit reader,
//! canonical prefix codes (with the code-length code and its repeat symbols),
//! meta prefix codes selected by an entropy image, a color cache, LZ77
//! backward references with the 120-entry distance map, and the four
//! transforms (predictor, cross color, subtract green, color indexing),
//! undone in reverse order at the end.
//!
//! Every length, index, and distance read from the stream is checked before
//! it is used. Pixel loops always advance by at least one pixel per symbol, so
//! a decode ends after at most `width * height` symbols. Prefix code tables
//! are capped in total size so a hostile stream cannot ask for gigabytes of
//! lookup tables.

use crate::ImageError;

/// Signature byte that starts a `VP8L` chunk.
pub(crate) const VP8L_SIGNATURE: u8 = 0x2F;

/// Number of prefix codes in one group: green+length+cache, red, blue, alpha, distance.
const CODES_PER_GROUP: usize = 5;
/// Literal alphabet size for red, blue, and alpha.
const LITERAL_ALPHABET: usize = 256;
/// Number of LZ77 length prefix symbols that follow the 256 green literals.
const LENGTH_PREFIXES: usize = 24;
/// Distance prefix alphabet size.
const DISTANCE_ALPHABET: usize = 40;
/// Longest code length the format allows.
const MAX_CODE_LENGTH: usize = 15;
/// Bits resolved by the first lookup level of a prefix code table.
const ROOT_BITS: u32 = 8;
/// Upper bound on prefix table entries across all groups of one image.
///
/// Real files use a few hundred groups at most. 8M entries is 32 MiB.
const MAX_TABLE_ENTRIES: usize = 8 << 20;

/// Order in which the code-length code lengths are stored.
const CODE_LENGTH_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

/// Neighbor offsets `(dx, dy)` for distance codes 1 through 120.
///
/// Code `n` maps to `DISTANCE_MAP[n - 1]`; the scan-line distance is
/// `dx + dy * width`, and anything below 1 becomes 1.
const DISTANCE_MAP: [(i8, i8); 120] = [
    (0, 1),
    (1, 0),
    (1, 1),
    (-1, 1),
    (0, 2),
    (2, 0),
    (1, 2),
    (-1, 2),
    (2, 1),
    (-2, 1),
    (2, 2),
    (-2, 2),
    (0, 3),
    (3, 0),
    (1, 3),
    (-1, 3),
    (3, 1),
    (-3, 1),
    (2, 3),
    (-2, 3),
    (3, 2),
    (-3, 2),
    (0, 4),
    (4, 0),
    (1, 4),
    (-1, 4),
    (4, 1),
    (-4, 1),
    (3, 3),
    (-3, 3),
    (2, 4),
    (-2, 4),
    (4, 2),
    (-4, 2),
    (0, 5),
    (3, 4),
    (-3, 4),
    (4, 3),
    (-4, 3),
    (5, 0),
    (1, 5),
    (-1, 5),
    (5, 1),
    (-5, 1),
    (2, 5),
    (-2, 5),
    (5, 2),
    (-5, 2),
    (4, 4),
    (-4, 4),
    (3, 5),
    (-3, 5),
    (5, 3),
    (-5, 3),
    (0, 6),
    (6, 0),
    (1, 6),
    (-1, 6),
    (6, 1),
    (-6, 1),
    (2, 6),
    (-2, 6),
    (6, 2),
    (-6, 2),
    (4, 5),
    (-4, 5),
    (5, 4),
    (-5, 4),
    (3, 6),
    (-3, 6),
    (6, 3),
    (-6, 3),
    (0, 7),
    (7, 0),
    (1, 7),
    (-1, 7),
    (5, 5),
    (-5, 5),
    (7, 1),
    (-7, 1),
    (4, 6),
    (-4, 6),
    (6, 4),
    (-6, 4),
    (2, 7),
    (-2, 7),
    (7, 2),
    (-7, 2),
    (3, 7),
    (-3, 7),
    (7, 3),
    (-7, 3),
    (5, 6),
    (-5, 6),
    (6, 5),
    (-6, 5),
    (8, 0),
    (4, 7),
    (-4, 7),
    (7, 4),
    (-7, 4),
    (8, 1),
    (8, 2),
    (6, 6),
    (-6, 6),
    (8, 3),
    (5, 7),
    (-5, 7),
    (7, 5),
    (-7, 5),
    (8, 4),
    (6, 7),
    (-6, 7),
    (7, 6),
    (-7, 6),
    (8, 5),
    (7, 7),
    (-7, 7),
    (8, 6),
    (8, 7),
];

/// LSB-first bit reader over one `VP8L` payload.
///
/// Bits past the end of the data read as zero for peeking, but consuming them
/// is an [`ImageError::UnexpectedEof`].
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    nbits: u32,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        let mut reader = BitReader {
            data,
            pos: 0,
            acc: 0,
            nbits: 0,
        };
        reader.refill();
        reader
    }

    fn refill(&mut self) {
        while self.nbits <= 56 && self.pos < self.data.len() {
            self.acc |= u64::from(self.data[self.pos]) << self.nbits;
            self.pos += 1;
            self.nbits += 8;
        }
    }

    /// Look at the next `n` bits (at most 32) without consuming them.
    fn peek(&mut self, n: u32) -> u32 {
        if self.nbits < n {
            self.refill();
        }
        (self.acc & ((1u64 << n) - 1)) as u32
    }

    /// Drop `n` bits that were already peeked.
    fn consume(&mut self, n: u32) -> Result<(), ImageError> {
        if n > self.nbits {
            self.refill();
            if n > self.nbits {
                return Err(ImageError::UnexpectedEof);
            }
        }
        self.acc >>= n;
        self.nbits -= n;
        Ok(())
    }

    /// Read `n` bits (at most 32) as an unsigned value, first bit lowest.
    pub(crate) fn read(&mut self, n: u32) -> Result<u32, ImageError> {
        if n == 0 {
            return Ok(0);
        }
        let value = self.peek(n);
        self.consume(n)?;
        Ok(value)
    }

    fn read_flag(&mut self) -> Result<bool, ImageError> {
        Ok(self.read(1)? == 1)
    }
}

/// Canonical prefix code as a two-level lookup table.
///
/// Each entry packs `symbol | (length << 16)`. A root entry with
/// [`LINK`] set points at a second-level table: its low 16 bits are the
/// offset, and bits 16..24 are the number of extra index bits.
#[derive(Clone, Debug)]
struct PrefixCode {
    table: Vec<u32>,
    root_bits: u32,
}

const LINK: u32 = 1 << 31;

impl PrefixCode {
    /// Build a code from per-symbol lengths (0 means unused).
    ///
    /// A code with one used symbol reads zero bits. Otherwise the lengths
    /// must describe a complete prefix code; an over- or under-full set is
    /// rejected, as is a set with no used symbol.
    fn from_lengths(lengths: &[u8]) -> Result<Self, ImageError> {
        let mut count = [0u32; MAX_CODE_LENGTH + 1];
        let mut used = 0usize;
        let mut only = 0usize;
        for (symbol, &len) in lengths.iter().enumerate() {
            if len as usize > MAX_CODE_LENGTH {
                return Err(ImageError::InvalidHeader("VP8L code length too long"));
            }
            if len > 0 {
                count[len as usize] += 1;
                used += 1;
                only = symbol;
            }
        }
        if used == 0 {
            return Err(ImageError::InvalidHeader("VP8L prefix code has no symbols"));
        }
        if used == 1 {
            return Ok(PrefixCode {
                table: vec![only as u32],
                root_bits: 0,
            });
        }
        // Kraft sum must be exactly 1 for a complete code.
        let mut space: u32 = 0;
        for (len, &n) in count.iter().enumerate().skip(1) {
            space += n << (MAX_CODE_LENGTH - len);
        }
        if space != 1 << MAX_CODE_LENGTH {
            return Err(ImageError::InvalidHeader(
                "VP8L prefix code is not complete",
            ));
        }
        let max_len = (1..=MAX_CODE_LENGTH)
            .rev()
            .find(|&l| count[l] > 0)
            .unwrap_or(1) as u32;
        let root_bits = max_len.min(ROOT_BITS);

        // First canonical code of each length.
        let mut next_code = [0u32; MAX_CODE_LENGTH + 2];
        let mut code = 0u32;
        for len in 1..=MAX_CODE_LENGTH {
            code = (code + count[len - 1]) << 1;
            next_code[len] = code;
        }
        // count[0] holds no codes; the loop above treats it as zero.
        let mut codes = vec![0u32; lengths.len()];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len > 0 {
                codes[symbol] = reverse_bits(next_code[len as usize], len as u32);
                next_code[len as usize] += 1;
            }
        }

        // Size the second-level tables: one per root slot that long codes share.
        let root_size = 1usize << root_bits;
        let mut sub_bits = vec![0u32; root_size];
        for (symbol, &len) in lengths.iter().enumerate() {
            let len = u32::from(len);
            if len > root_bits {
                let slot = (codes[symbol] & ((1 << root_bits) - 1)) as usize;
                sub_bits[slot] = sub_bits[slot].max(len - root_bits);
            }
        }
        let mut total = root_size;
        let mut table = vec![0u32; root_size];
        for (slot, &bits) in sub_bits.iter().enumerate() {
            if bits > 0 {
                table[slot] = LINK | (bits << 16) | total as u32;
                total += 1 << bits;
            }
        }
        table.resize(total, 0);

        for (symbol, &len) in lengths.iter().enumerate() {
            let len = u32::from(len);
            if len == 0 {
                continue;
            }
            let code = codes[symbol];
            if len <= root_bits {
                let mut index = code as usize;
                while index < root_size {
                    table[index] = symbol as u32 | (len << 16);
                    index += 1 << len;
                }
            } else {
                let slot = (code & ((1 << root_bits) - 1)) as usize;
                let link = table[slot];
                let bits = (link >> 16) & 0xFF;
                let base = (link & 0xFFFF) as usize;
                let rest_len = len - root_bits;
                let mut index = (code >> root_bits) as usize;
                while index < 1 << bits {
                    table[base + index] = symbol as u32 | (rest_len << 16);
                    index += 1 << rest_len;
                }
            }
        }
        Ok(PrefixCode { table, root_bits })
    }

    fn entries(&self) -> usize {
        self.table.len()
    }

    /// Decode one symbol.
    fn read_symbol(&self, br: &mut BitReader<'_>) -> Result<u16, ImageError> {
        if self.root_bits == 0 {
            return Ok(self.table[0] as u16);
        }
        let bits = br.peek(MAX_CODE_LENGTH as u32);
        let entry = self.table[(bits & ((1 << self.root_bits) - 1)) as usize];
        if entry & LINK == 0 {
            br.consume(entry >> 16)?;
            return Ok(entry as u16);
        }
        let sub = (entry >> 16) & 0xFF;
        let base = (entry & 0xFFFF) as usize;
        let index = ((bits >> self.root_bits) & ((1 << sub) - 1)) as usize;
        let leaf = self.table[base + index];
        br.consume(self.root_bits + (leaf >> 16))?;
        Ok(leaf as u16)
    }
}

/// Reverse the low `len` bits of `code`.
fn reverse_bits(code: u32, len: u32) -> u32 {
    if len == 0 {
        0
    } else {
        code.reverse_bits() >> (32 - len)
    }
}

/// Read one prefix code for an alphabet of `alphabet` symbols.
fn read_prefix_code(br: &mut BitReader<'_>, alphabet: usize) -> Result<PrefixCode, ImageError> {
    let mut lengths = vec![0u8; alphabet];
    if br.read_flag()? {
        // Simple code: one or two symbols, each of length 1 (or 0 if alone).
        let two = br.read_flag()?;
        let first_bits = if br.read_flag()? { 8 } else { 1 };
        let first = br.read(first_bits)? as usize;
        if first >= alphabet {
            return Err(ImageError::InvalidHeader(
                "VP8L simple code symbol out of range",
            ));
        }
        lengths[first] = 1;
        if two {
            let second = br.read(8)? as usize;
            if second >= alphabet {
                return Err(ImageError::InvalidHeader(
                    "VP8L simple code symbol out of range",
                ));
            }
            lengths[second] = 1;
        }
        return PrefixCode::from_lengths(&lengths);
    }

    let mut length_code_lengths = [0u8; 19];
    let count = 4 + br.read(4)? as usize;
    for &slot in CODE_LENGTH_ORDER.iter().take(count) {
        length_code_lengths[slot] = br.read(3)? as u8;
    }
    let length_code = PrefixCode::from_lengths(&length_code_lengths)?;

    let mut max_tokens = if br.read_flag()? {
        let nbits = 2 + 2 * br.read(3)?;
        let tokens = 2 + br.read(nbits)? as usize;
        if tokens > alphabet {
            return Err(ImageError::InvalidHeader(
                "VP8L max_symbol exceeds alphabet",
            ));
        }
        tokens
    } else {
        alphabet
    };

    let mut symbol = 0usize;
    let mut previous = 8u8;
    while symbol < alphabet {
        if max_tokens == 0 {
            break;
        }
        max_tokens -= 1;
        let token = length_code.read_symbol(br)?;
        match token {
            0..=15 => {
                lengths[symbol] = token as u8;
                symbol += 1;
                if token != 0 {
                    previous = token as u8;
                }
            }
            16..=18 => {
                let (extra, base, value) = match token {
                    16 => (2, 3, previous),
                    17 => (3, 3, 0),
                    _ => (7, 11, 0),
                };
                let repeat = base + br.read(extra)? as usize;
                if symbol + repeat > alphabet {
                    return Err(ImageError::InvalidHeader(
                        "VP8L code length repeat overflows",
                    ));
                }
                lengths[symbol..symbol + repeat].fill(value);
                symbol += repeat;
            }
            _ => return Err(ImageError::InvalidHeader("VP8L bad code length symbol")),
        }
    }
    PrefixCode::from_lengths(&lengths)
}

/// The five prefix codes used for one block of pixels.
struct Group {
    codes: [PrefixCode; CODES_PER_GROUP],
}

/// Read a value coded as a prefix symbol plus extra bits (lengths and distances).
fn prefix_value(br: &mut BitReader<'_>, symbol: u16) -> Result<usize, ImageError> {
    let symbol = u32::from(symbol);
    if symbol < 4 {
        return Ok(symbol as usize + 1);
    }
    let extra = (symbol - 2) >> 1;
    let offset = (2 + (symbol & 1)) << extra;
    Ok((offset + br.read(extra)?) as usize + 1)
}

/// Turn a distance code into a scan-line distance for an image `width` wide.
fn plane_distance(width: usize, code: usize) -> usize {
    if code > DISTANCE_MAP.len() {
        return code - DISTANCE_MAP.len();
    }
    let (dx, dy) = DISTANCE_MAP[code - 1];
    let dist = i64::from(dx) + i64::from(dy) * width as i64;
    if dist < 1 { 1 } else { dist as usize }
}

/// Number of blocks of `1 << bits` pixels needed to cover `size` pixels.
fn div_round_up(size: usize, bits: u32) -> usize {
    (size + (1 << bits) - 1) >> bits
}

/// Fixed-size color cache indexed by a multiplicative hash.
struct ColorCache {
    colors: Vec<u32>,
    shift: u32,
}

impl ColorCache {
    fn new(bits: u32) -> Self {
        ColorCache {
            colors: vec![0; 1 << bits],
            shift: 32 - bits,
        }
    }

    fn insert(&mut self, argb: u32) {
        let index = (argb.wrapping_mul(0x1E35_A7BD) >> self.shift) as usize;
        self.colors[index] = argb;
    }
}

/// Shared decode state: the reader plus the running prefix table budget.
struct Decoder<'a> {
    br: BitReader<'a>,
    table_entries: usize,
}

/// One inverse transform, recorded in stream order.
enum Transform {
    Predictor {
        bits: u32,
        data: Vec<u32>,
    },
    CrossColor {
        bits: u32,
        data: Vec<u32>,
    },
    SubtractGreen,
    ColorIndexing {
        bits: u32,
        palette: Vec<u32>,
        width: usize,
    },
}

impl Decoder<'_> {
    /// Decode an entropy-coded image of `width` by `height` pixels.
    ///
    /// `top_level` is true for the main ARGB image, the only role that may use
    /// meta prefix codes.
    fn decode_image_data(
        &mut self,
        width: usize,
        height: usize,
        top_level: bool,
    ) -> Result<Vec<u32>, ImageError> {
        let total = width
            .checked_mul(height)
            .ok_or(ImageError::InvalidHeader("VP8L image too large"))?;

        let mut cache = if self.br.read_flag()? {
            let bits = self.br.read(4)?;
            if !(1..=11).contains(&bits) {
                return Err(ImageError::InvalidHeader(
                    "VP8L color cache size out of range",
                ));
            }
            Some(ColorCache::new(bits))
        } else {
            None
        };
        let cache_size = cache.as_ref().map_or(0, |c| c.colors.len());

        // Meta prefix codes: an entropy image picks a group per block.
        let mut meta_bits = 0u32;
        let mut meta_width = 0usize;
        let mut meta: Vec<u32> = Vec::new();
        let mut group_count = 1usize;
        if top_level && self.br.read_flag()? {
            meta_bits = self.br.read(3)? + 2;
            meta_width = div_round_up(width, meta_bits);
            let meta_height = div_round_up(height, meta_bits);
            meta = self.decode_image_data(meta_width, meta_height, false)?;
            for value in &mut meta {
                *value = (*value >> 8) & 0xFFFF;
                group_count = group_count.max(*value as usize + 1);
            }
        }

        // Only build tables for groups the entropy image refers to. The
        // others are still read so the bit position stays right.
        let mut used = vec![meta.is_empty(); group_count];
        for &value in &meta {
            used[value as usize] = true;
        }
        let alphabets = [
            LITERAL_ALPHABET + LENGTH_PREFIXES + cache_size,
            LITERAL_ALPHABET,
            LITERAL_ALPHABET,
            LITERAL_ALPHABET,
            DISTANCE_ALPHABET,
        ];
        let mut groups: Vec<Option<Group>> = Vec::with_capacity(group_count.min(4096));
        for &is_used in &used {
            let mut codes = Vec::with_capacity(CODES_PER_GROUP);
            for &alphabet in &alphabets {
                let code = read_prefix_code(&mut self.br, alphabet)?;
                if is_used {
                    self.table_entries += code.entries();
                    if self.table_entries > MAX_TABLE_ENTRIES {
                        return Err(ImageError::UnsupportedFormat(
                            "VP8L prefix tables exceed the size limit",
                        ));
                    }
                    codes.push(code);
                }
            }
            groups.push(if is_used {
                let mut it = codes.into_iter();
                match (it.next(), it.next(), it.next(), it.next(), it.next()) {
                    (Some(g), Some(r), Some(b), Some(a), Some(d)) => Some(Group {
                        codes: [g, r, b, a, d],
                    }),
                    _ => return Err(ImageError::InvalidHeader("VP8L prefix group incomplete")),
                }
            } else {
                None
            });
        }

        let mut out = vec![0u32; total];
        let mut pos = 0usize;
        let mut x = 0usize;
        let mut y = 0usize;
        let mask = if meta.is_empty() {
            usize::MAX
        } else {
            (1usize << meta_bits) - 1
        };
        let mut group_index = 0usize;
        let mut cached = 0usize; // pixels already pushed into the color cache
        while pos < total {
            if !meta.is_empty() && (x & mask) == 0 {
                group_index = meta[(y >> meta_bits) * meta_width + (x >> meta_bits)] as usize;
            }
            let group = groups[group_index]
                .as_ref()
                .ok_or(ImageError::InvalidHeader("VP8L missing prefix group"))?;
            let symbol = group.codes[0].read_symbol(&mut self.br)? as usize;
            if symbol < LITERAL_ALPHABET {
                let red = u32::from(group.codes[1].read_symbol(&mut self.br)?);
                let blue = u32::from(group.codes[2].read_symbol(&mut self.br)?);
                let alpha = u32::from(group.codes[3].read_symbol(&mut self.br)?);
                out[pos] = (alpha << 24) | (red << 16) | ((symbol as u32) << 8) | blue;
                pos += 1;
                x += 1;
                if x == width {
                    x = 0;
                    y += 1;
                }
            } else if symbol < LITERAL_ALPHABET + LENGTH_PREFIXES {
                let length = prefix_value(&mut self.br, (symbol - LITERAL_ALPHABET) as u16)?;
                let dist_symbol = group.codes[4].read_symbol(&mut self.br)?;
                let dist_code = prefix_value(&mut self.br, dist_symbol)?;
                let dist = plane_distance(width, dist_code);
                if dist > pos || length > total - pos {
                    return Err(ImageError::InvalidHeader(
                        "VP8L backward reference out of range",
                    ));
                }
                for i in pos..pos + length {
                    out[i] = out[i - dist];
                }
                pos += length;
                x += length;
                while x >= width {
                    x -= width;
                    y += 1;
                }
                // The group may change inside a copy; recompute it next round.
                if !meta.is_empty() && pos < total {
                    group_index = meta[(y >> meta_bits) * meta_width + (x >> meta_bits)] as usize;
                }
            } else {
                let index = symbol - LITERAL_ALPHABET - LENGTH_PREFIXES;
                let cache = cache.as_mut().ok_or(ImageError::InvalidHeader(
                    "VP8L cache symbol without a cache",
                ))?;
                // Bring the cache up to date before looking a color up.
                while cached < pos {
                    cache.insert(out[cached]);
                    cached += 1;
                }
                let color = *cache
                    .colors
                    .get(index)
                    .ok_or(ImageError::InvalidHeader("VP8L cache index out of range"))?;
                out[pos] = color;
                pos += 1;
                x += 1;
                if x == width {
                    x = 0;
                    y += 1;
                }
            }
            if let Some(cache) = cache.as_mut() {
                while cached < pos {
                    cache.insert(out[cached]);
                    cached += 1;
                }
            }
        }
        Ok(out)
    }

    /// Read the transform list, then the main image, and undo the transforms.
    fn decode_image_stream(&mut self, width: usize, height: usize) -> Result<Vec<u32>, ImageError> {
        let mut transforms: Vec<Transform> = Vec::new();
        let mut seen = [false; 4];
        let mut coded_width = width;
        while self.br.read_flag()? {
            let kind = self.br.read(2)? as usize;
            if seen[kind] {
                return Err(ImageError::InvalidHeader("VP8L transform repeated"));
            }
            seen[kind] = true;
            let transform = match kind {
                0 | 1 => {
                    let bits = self.br.read(3)? + 2;
                    let data = self.decode_image_data(
                        div_round_up(coded_width, bits),
                        div_round_up(height, bits),
                        false,
                    )?;
                    if kind == 0 {
                        Transform::Predictor { bits, data }
                    } else {
                        Transform::CrossColor { bits, data }
                    }
                }
                2 => Transform::SubtractGreen,
                _ => {
                    let size = self.br.read(8)? as usize + 1;
                    let bits = match size {
                        0..=2 => 3,
                        3..=4 => 2,
                        5..=16 => 1,
                        _ => 0,
                    };
                    let mut palette = self.decode_image_data(size, 1, false)?;
                    for i in 1..palette.len() {
                        palette[i] = add_pixels(palette[i], palette[i - 1]);
                    }
                    // Indexes past the table read as transparent black.
                    palette.resize(256, 0);
                    let original = coded_width;
                    coded_width = div_round_up(coded_width, bits);
                    Transform::ColorIndexing {
                        bits,
                        palette,
                        width: original,
                    }
                }
            };
            transforms.push(transform);
        }

        let mut pixels = self.decode_image_data(coded_width, height, true)?;
        let mut current_width = coded_width;
        for transform in transforms.iter().rev() {
            match transform {
                Transform::Predictor { bits, data } => {
                    inverse_predictor(&mut pixels, current_width, height, *bits, data);
                }
                Transform::CrossColor { bits, data } => {
                    inverse_cross_color(&mut pixels, current_width, *bits, data);
                }
                Transform::SubtractGreen => {
                    for p in &mut pixels {
                        let green = (*p >> 8) & 0xFF;
                        let red = ((*p >> 16) + green) & 0xFF;
                        let blue = ((*p & 0xFF) + green) & 0xFF;
                        *p = (*p & 0xFF00_FF00) | (red << 16) | blue;
                    }
                }
                Transform::ColorIndexing {
                    bits,
                    palette,
                    width,
                } => {
                    pixels = inverse_color_indexing(
                        &pixels,
                        current_width,
                        *width,
                        height,
                        *bits,
                        palette,
                    );
                    current_width = *width;
                }
            }
        }
        if current_width != width || pixels.len() != width * height {
            return Err(ImageError::InvalidHeader("VP8L transform size mismatch"));
        }
        Ok(pixels)
    }
}

/// Add two ARGB pixels channel by channel, modulo 256.
fn add_pixels(a: u32, b: u32) -> u32 {
    let alpha_green = (a & 0xFF00_FF00).wrapping_add(b & 0xFF00_FF00) & 0xFF00_FF00;
    let red_blue = (a & 0x00FF_00FF).wrapping_add(b & 0x00FF_00FF) & 0x00FF_00FF;
    alpha_green | red_blue
}

/// Channel-wise average of two pixels, rounding down.
fn average2(a: u32, b: u32) -> u32 {
    (((a ^ b) & 0xFEFE_FEFE) >> 1) + (a & b)
}

fn channel(p: u32, shift: u32) -> i32 {
    ((p >> shift) & 0xFF) as i32
}

/// Pick `left` or `top`, whichever is closer to the gradient estimate.
fn select(left: u32, top: u32, top_left: u32) -> u32 {
    let mut to_left = 0;
    let mut to_top = 0;
    for shift in [24, 16, 8, 0] {
        let l = channel(left, shift);
        let t = channel(top, shift);
        let tl = channel(top_left, shift);
        // Estimate is l + t - tl; its distance to l is |t - tl| and to t is |l - tl|.
        to_left += (t - tl).abs();
        to_top += (l - tl).abs();
    }
    if to_left < to_top { left } else { top }
}

fn clamp_byte(v: i32) -> u32 {
    v.clamp(0, 255) as u32
}

fn clamp_add_subtract_full(a: u32, b: u32, c: u32) -> u32 {
    let mut out = 0;
    for shift in [24, 16, 8, 0] {
        out |= clamp_byte(channel(a, shift) + channel(b, shift) - channel(c, shift)) << shift;
    }
    out
}

fn clamp_add_subtract_half(a: u32, b: u32) -> u32 {
    let mut out = 0;
    for shift in [24, 16, 8, 0] {
        let ca = channel(a, shift);
        let cb = channel(b, shift);
        out |= clamp_byte(ca + (ca - cb) / 2) << shift;
    }
    out
}

/// Prediction for mode `mode` from the left, top, top-left, and top-right pixels.
fn predict(mode: u32, left: u32, top: u32, top_left: u32, top_right: u32) -> u32 {
    match mode {
        0 => 0xFF00_0000,
        1 => left,
        2 => top,
        3 => top_right,
        4 => top_left,
        5 => average2(average2(left, top_right), top),
        6 => average2(left, top_left),
        7 => average2(left, top),
        8 => average2(top_left, top),
        9 => average2(top, top_right),
        10 => average2(average2(left, top_left), average2(top, top_right)),
        11 => select(left, top, top_left),
        12 => clamp_add_subtract_full(left, top, top_left),
        13 => clamp_add_subtract_half(average2(left, top), top_left),
        // Modes 14 and 15 are not defined by the spec. They predict opaque
        // black, like mode 0, so a stray value cannot index out of range.
        _ => 0xFF00_0000,
    }
}

fn inverse_predictor(pixels: &mut [u32], width: usize, height: usize, bits: u32, data: &[u32]) {
    if pixels.is_empty() {
        return;
    }
    let blocks_wide = div_round_up(width, bits);
    // Top-left pixel: black prediction. Rest of the top row: left.
    pixels[0] = add_pixels(pixels[0], 0xFF00_0000);
    for x in 1..width {
        pixels[x] = add_pixels(pixels[x], pixels[x - 1]);
    }
    for y in 1..height {
        let row = y * width;
        // First column: top.
        pixels[row] = add_pixels(pixels[row], pixels[row - width]);
        let block_row = (y >> bits) * blocks_wide;
        for x in 1..width {
            let mode = (data[block_row + (x >> bits)] >> 8) & 0xF;
            let i = row + x;
            let left = pixels[i - 1];
            let top = pixels[i - width];
            let top_left = pixels[i - width - 1];
            // At the right edge the top-right neighbor is the first pixel of
            // the current row, which is what the next index holds.
            let top_right = pixels[i - width + 1];
            let pred = predict(mode, left, top, top_left, top_right);
            pixels[i] = add_pixels(pixels[i], pred);
        }
    }
}

/// Signed product used by the cross-color transform.
fn color_delta(t: u8, c: u8) -> i32 {
    (i32::from(t as i8) * i32::from(c as i8)) >> 5
}

fn inverse_cross_color(pixels: &mut [u32], width: usize, bits: u32, data: &[u32]) {
    let blocks_wide = div_round_up(width, bits);
    for (i, p) in pixels.iter_mut().enumerate() {
        let x = i % width;
        let y = i / width;
        let element = data[(y >> bits) * blocks_wide + (x >> bits)];
        let green_to_red = element as u8;
        let green_to_blue = (element >> 8) as u8;
        let red_to_blue = (element >> 16) as u8;
        let green = (*p >> 8) as u8;
        let mut red = ((*p >> 16) & 0xFF) as i32;
        let mut blue = (*p & 0xFF) as i32;
        red += color_delta(green_to_red, green);
        let red_byte = (red & 0xFF) as u8;
        blue += color_delta(green_to_blue, green);
        blue += color_delta(red_to_blue, red_byte);
        *p = (*p & 0xFF00_FF00) | (u32::from(red_byte) << 16) | ((blue & 0xFF) as u32);
    }
}

fn inverse_color_indexing(
    packed: &[u32],
    packed_width: usize,
    width: usize,
    height: usize,
    bits: u32,
    palette: &[u32],
) -> Vec<u32> {
    let mut out = vec![0u32; width * height];
    let per_byte_bits = 8 >> bits;
    let index_mask = (1u32 << per_byte_bits) - 1;
    let x_mask = (1usize << bits) - 1;
    for y in 0..height {
        for x in 0..width {
            let source = packed[y * packed_width + (x >> bits)];
            let green = (source >> 8) & 0xFF;
            let index = (green >> ((x & x_mask) as u32 * per_byte_bits)) & index_mask;
            out[y * width + x] = palette[index as usize];
        }
    }
    out
}

/// Parse the five-byte `VP8L` header: signature, 14-bit width and height
/// minus one, the alpha hint, and a 3-bit version that must be 0.
pub(crate) fn read_header(data: &[u8]) -> Result<(u32, u32, bool), ImageError> {
    if data.len() < 5 {
        return Err(ImageError::UnexpectedEof);
    }
    if data[0] != VP8L_SIGNATURE {
        return Err(ImageError::InvalidHeader("invalid VP8L signature"));
    }
    let bits = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
    let width = (bits & 0x3FFF) + 1;
    let height = ((bits >> 14) & 0x3FFF) + 1;
    let alpha_hint = (bits >> 28) & 1 == 1;
    if bits >> 29 != 0 {
        return Err(ImageError::UnsupportedFormat("VP8L version is not 0"));
    }
    Ok((width, height, alpha_hint))
}

/// Decode a complete `VP8L` chunk payload into ARGB pixels.
pub(crate) fn decode(data: &[u8]) -> Result<(u32, u32, Vec<u32>), ImageError> {
    let (width, height, _) = read_header(data)?;
    crate::validate_dimensions(width, height)?;
    let mut decoder = Decoder {
        br: BitReader::new(&data[5..]),
        table_entries: 0,
    };
    let pixels = decoder.decode_image_stream(width as usize, height as usize)?;
    Ok((width, height, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LSB-first bit writer, the mirror of [`BitReader`].
    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        acc: u64,
        nbits: u32,
    }

    impl BitWriter {
        fn put(&mut self, value: u32, n: u32) {
            self.acc |= u64::from(value) << self.nbits;
            self.nbits += n;
            while self.nbits >= 8 {
                self.bytes.push(self.acc as u8);
                self.acc >>= 8;
                self.nbits -= 8;
            }
        }

        fn finish(mut self) -> Vec<u8> {
            if self.nbits > 0 {
                self.bytes.push(self.acc as u8);
            }
            self.bytes
        }
    }

    /// Write a prefix code where every literal 0..256 has length 8 and the
    /// rest of the alphabet is unused. The code-length code has two
    /// one-bit symbols: 0 (code `0`) and 8 (code `1`).
    fn put_flat_literal_code(w: &mut BitWriter, alphabet: usize) {
        w.put(0, 1); // normal code
        // 12 code-length code lengths in storage order 17, 18, 0, 1, ..., 7, 8.
        w.put(12 - 4, 4);
        for &symbol in CODE_LENGTH_ORDER.iter().take(12) {
            let len = if symbol == 0 || symbol == 8 { 1 } else { 0 };
            w.put(len, 3);
        }
        w.put(0, 1); // read lengths for the whole alphabet
        for symbol in 0..alphabet {
            w.put(u32::from(symbol < 256), 1);
        }
    }

    /// A tiny lossless encoder: no transforms (or subtract green only), no
    /// cache, no meta codes, every pixel a literal with flat 8-bit codes.
    fn encode_literals(width: u32, height: u32, argb: &[u32], subtract_green: bool) -> Vec<u8> {
        let mut w = BitWriter::default();
        w.put(u32::from(VP8L_SIGNATURE), 8);
        w.put(width - 1, 14);
        w.put(height - 1, 14);
        w.put(1, 1); // alpha hint
        w.put(0, 3); // version
        if subtract_green {
            w.put(1, 1);
            w.put(2, 2);
        }
        w.put(0, 1); // no more transforms
        w.put(0, 1); // no color cache
        w.put(0, 1); // no meta prefix codes
        put_flat_literal_code(&mut w, 256 + 24);
        for _ in 0..3 {
            put_flat_literal_code(&mut w, 256);
        }
        // Distance code: simple, one symbol (0), zero bits per use.
        w.put(1, 1);
        w.put(0, 1);
        w.put(0, 1);
        w.put(0, 1);
        for &p in argb {
            let (mut r, g, mut b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
            if subtract_green {
                r = (r + 256 - g) & 0xFF;
                b = (b + 256 - g) & 0xFF;
            }
            // A flat code assigns codes in symbol order, so the code for
            // symbol s is s itself, written most significant bit first.
            for value in [g, r, b, p >> 24] {
                w.put(reverse_bits(value, 8), 8);
            }
        }
        w.finish()
    }

    #[test]
    fn bit_reader_reads_lsb_first_and_reports_the_end() {
        let mut br = BitReader::new(&[0b1010_1100, 0xFF]);
        assert_eq!(br.read(2).unwrap(), 0b00);
        assert_eq!(br.read(3).unwrap(), 0b011);
        assert_eq!(br.read(3).unwrap(), 0b101);
        assert_eq!(br.read(8).unwrap(), 0xFF);
        assert_eq!(br.read(1), Err(ImageError::UnexpectedEof));
        let mut wide = BitReader::new(&[0x78, 0x56, 0x34, 0x12, 0x01]);
        assert_eq!(wide.read(32).unwrap(), 0x1234_5678);
        assert_eq!(wide.read(0).unwrap(), 0);
    }

    #[test]
    fn prefix_codes_must_be_complete() {
        // Lengths 1, 2, 3, 3: complete.
        let code = PrefixCode::from_lengths(&[1, 2, 3, 3]).unwrap();
        // Canonical codes: 0 -> 0, 1 -> 10, 2 -> 110, 3 -> 111 (first bit first).
        let mut br = BitReader::new(&[0b1111_1010, 0b0000_0000]);
        // Bits in read order: 0 | 1 0 | 1 1 1 | 1 1 0
        assert_eq!(code.read_symbol(&mut br).unwrap(), 0);
        assert_eq!(code.read_symbol(&mut br).unwrap(), 1);
        assert_eq!(code.read_symbol(&mut br).unwrap(), 3);
        assert_eq!(code.read_symbol(&mut br).unwrap(), 2);
        assert!(PrefixCode::from_lengths(&[1, 2, 3]).is_err(), "under-full");
        assert!(PrefixCode::from_lengths(&[1, 1, 1]).is_err(), "over-full");
        assert!(PrefixCode::from_lengths(&[0, 0, 0]).is_err(), "empty");
        assert!(PrefixCode::from_lengths(&[16, 1]).is_err(), "too long");
        // One used symbol reads no bits.
        let single = PrefixCode::from_lengths(&[0, 0, 5, 0]).unwrap();
        let mut empty = BitReader::new(&[]);
        assert_eq!(single.read_symbol(&mut empty).unwrap(), 2);
    }

    #[test]
    fn long_codes_use_the_second_table_level() {
        // Two symbols of length 1 would be trivial; build a skewed code with
        // lengths 1, 2, ..., 14, 15, 15 so the longest codes need 15 bits.
        let mut lengths: Vec<u8> = (1..=15).collect();
        lengths.push(15);
        let code = PrefixCode::from_lengths(&lengths).unwrap();
        // Symbol 15 (the second length-15 code) is fifteen 1 bits.
        let mut br = BitReader::new(&[0xFF, 0x7F]);
        assert_eq!(code.read_symbol(&mut br).unwrap(), 15);
        // Symbol 9 is nine 1 bits then a 0.
        let mut br = BitReader::new(&[0xFF, 0x01]);
        assert_eq!(code.read_symbol(&mut br).unwrap(), 9);
        // Running out of data mid-code is an error.
        let mut short = BitReader::new(&[0xFF]);
        assert_eq!(code.read_symbol(&mut short), Err(ImageError::UnexpectedEof));
    }

    #[test]
    fn prefix_values_and_distances_follow_the_spec() {
        let mut br = BitReader::new(&[0b0000_0011, 0]);
        assert_eq!(prefix_value(&mut br, 0).unwrap(), 1);
        assert_eq!(prefix_value(&mut br, 3).unwrap(), 4);
        // Symbol 4: one extra bit (1) on offset 4, plus one.
        assert_eq!(prefix_value(&mut br, 4).unwrap(), 6);
        // Symbol 5: one extra bit (1) on offset 6, plus one.
        assert_eq!(prefix_value(&mut br, 5).unwrap(), 8);
        assert_eq!(plane_distance(10, 1), 10); // straight up
        assert_eq!(plane_distance(10, 2), 1); // left
        assert_eq!(plane_distance(10, 3), 11); // up and left
        assert_eq!(plane_distance(10, 4), 9); // up and right
        assert_eq!(plane_distance(1, 4), 1); // clamped to 1
        assert_eq!(plane_distance(10, 121), 1);
        assert_eq!(plane_distance(10, 500), 380);
    }

    #[test]
    fn pixel_arithmetic_works_per_channel() {
        assert_eq!(add_pixels(0xFF80_0102, 0x0180_FFFF), 0x0000_0001);
        assert_eq!(average2(0xFF00_0A0B, 0x0102_0C0E), 0x8001_0B0C);
        assert_eq!(select(0x0000_0010, 0x0000_0020, 0x0000_0011), 0x0000_0020);
        assert_eq!(select(0x0000_0010, 0x0000_0020, 0x0000_0021), 0x0000_0010);
        assert_eq!(
            clamp_add_subtract_full(0xFF10_F000, 0xFF10_2000, 0x0020_1000),
            0xFF00_FF00
        );
        assert_eq!(
            clamp_add_subtract_half(0x0000_0010, 0x0000_0013),
            0x0000_000F
        );
        assert_eq!(
            clamp_add_subtract_half(0x0000_00F0, 0x0000_0010),
            0x0000_00FF
        );
        assert_eq!(predict(0, 1, 2, 3, 4), 0xFF00_0000);
        assert_eq!(predict(3, 1, 2, 3, 4), 4);
        assert_eq!(predict(14, 1, 2, 3, 4), 0xFF00_0000);
        assert_eq!(color_delta(0x10, 0x20), 16);
        assert_eq!(color_delta(0xF0, 0x20), -16);
    }

    #[test]
    fn inverse_transforms_undo_simple_cases() {
        // Predictor mode 1 (left) everywhere: residuals of 1 accumulate along rows.
        let mut pixels = vec![0x0000_0001u32; 6];
        inverse_predictor(&mut pixels, 3, 2, 2, &[0x0000_0100]);
        // Row 0: black + 1, then left. Row 1: first pixel from above, then left.
        assert_eq!(
            pixels,
            vec![
                0xFF00_0001,
                0xFF00_0002,
                0xFF00_0003,
                0xFF00_0002,
                0xFF00_0003,
                0xFF00_0004,
            ]
        );

        // Cross color with green_to_red = 0x20, which adds all of green to red.
        let mut px = vec![0xFF10_2030u32];
        inverse_cross_color(&mut px, 1, 2, &[0xFF00_0020]);
        // red += (0x20 * 0x20) >> 5 = 0x20.
        assert_eq!(px[0], 0xFF30_2030);

        // Color indexing with two colors: eight indexes per green byte.
        let palette = {
            let mut p = vec![0xFF00_0000u32, 0xFFFF_FFFF];
            p.resize(256, 0);
            p
        };
        let packed = vec![0x0000_A500u32]; // green = 1010_0101
        let out = inverse_color_indexing(&packed, 1, 8, 1, 3, &palette);
        assert_eq!(
            out,
            vec![
                0xFFFF_FFFF,
                0xFF00_0000,
                0xFFFF_FFFF,
                0xFF00_0000,
                0xFF00_0000,
                0xFFFF_FFFF,
                0xFF00_0000,
                0xFFFF_FFFF,
            ]
        );
    }

    #[test]
    fn header_fields_are_checked() {
        assert_eq!(read_header(&[0x2F, 0, 0, 0, 0]).unwrap(), (1, 1, false));
        assert_eq!(
            read_header(&[0x2F, 0xFF, 0xFF, 0xFF, 0x1F]).unwrap(),
            (16384, 16384, true)
        );
        assert!(matches!(
            read_header(&[0x2E, 0, 0, 0, 0]),
            Err(ImageError::InvalidHeader(_))
        ));
        assert!(matches!(
            read_header(&[0x2F, 0, 0, 0, 0x20]),
            Err(ImageError::UnsupportedFormat(_))
        ));
        assert_eq!(read_header(&[0x2F, 0, 0]), Err(ImageError::UnexpectedEof));
        // 16384 x 16384 passes the header but not the size cap.
        assert!(matches!(
            decode(&[0x2F, 0xFF, 0xFF, 0xFF, 0x0F]),
            Err(ImageError::DimensionsTooLarge { .. })
        ));
    }

    #[test]
    fn self_made_literal_streams_round_trip() {
        let width = 5;
        let height = 3;
        let argb: Vec<u32> = (0..width * height)
            .map(|i| 0x8000_0000 | (i * 0x0102_0304) ^ 0x00A0_5010)
            .collect();
        for subtract_green in [false, true] {
            let stream = encode_literals(width, height, &argb, subtract_green);
            let (w, h, decoded) = decode(&stream).unwrap();
            assert_eq!((w, h), (width, height));
            assert_eq!(decoded, argb, "subtract_green = {subtract_green}");
        }
    }

    #[test]
    fn truncated_self_made_stream_is_an_error() {
        let argb = vec![0xFF12_3456u32; 16];
        let stream = encode_literals(4, 4, &argb, false);
        for keep in [5, stream.len() / 2, stream.len() - 1] {
            assert!(decode(&stream[..keep]).is_err(), "kept {keep} bytes");
        }
    }

    #[test]
    fn backward_reference_before_the_start_is_rejected() {
        // Green code: one symbol, 256 (a length-1 copy). Distance code: one
        // symbol, 0 (distance code 1, the pixel above). At pixel 0 there is
        // nothing above, so the copy must fail.
        let mut w = BitWriter::default();
        w.put(u32::from(VP8L_SIGNATURE), 8);
        w.put(1, 14);
        w.put(1, 14);
        w.put(0, 4);
        w.put(0, 1); // no transforms
        w.put(0, 1); // no cache
        w.put(0, 1); // no meta codes
        // Green: a normal code with only symbol 256 in use.
        w.put(0, 1);
        w.put(12 - 4, 4);
        for &symbol in CODE_LENGTH_ORDER.iter().take(12) {
            // Symbols 0 and 1 of the code-length alphabet, one bit each.
            w.put(u32::from(symbol == 0 || symbol == 1), 3);
        }
        w.put(0, 1);
        for symbol in 0..280 {
            w.put(u32::from(symbol == 256), 1);
        }
        for _ in 0..4 {
            // Red, blue, alpha, distance: simple single-symbol codes.
            w.put(1, 1);
            w.put(0, 1);
            w.put(0, 1);
            w.put(0, 1);
        }
        let stream = w.finish();
        assert!(matches!(decode(&stream), Err(ImageError::InvalidHeader(_))));
    }

    #[test]
    fn repeated_transform_is_rejected() {
        let mut w = BitWriter::default();
        w.put(u32::from(VP8L_SIGNATURE), 8);
        w.put(0, 32);
        for _ in 0..2 {
            w.put(1, 1);
            w.put(2, 2); // subtract green, twice
        }
        let stream = w.finish();
        assert!(matches!(decode(&stream), Err(ImageError::InvalidHeader(_))));
    }

    #[test]
    fn bad_color_cache_size_is_rejected() {
        let mut w = BitWriter::default();
        w.put(u32::from(VP8L_SIGNATURE), 8);
        w.put(0, 32);
        w.put(0, 1); // no transforms
        w.put(1, 1); // color cache present
        w.put(12, 4); // 12 bits is past the limit of 11
        let stream = w.finish();
        assert!(matches!(decode(&stream), Err(ImageError::InvalidHeader(_))));
    }
}
