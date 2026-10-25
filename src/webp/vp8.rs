// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2026 Buntato
//! Lossy `VP8` key frame decoder.
//!
//! A WebP lossy image is one VP8 key frame. Decoding goes: boolean entropy
//! decoder, frame header (segments, loop filter, quantizers, token
//! probability updates), per-macroblock prediction modes from the first
//! partition, DCT/WHT coefficient tokens from the token partitions,
//! dequantization, inverse transforms, intra prediction (16x16 luma, 8x8
//! chroma, and the ten 4x4 subblock modes), and finally the loop filter over
//! the whole frame. The result is a set of Y, U, and V planes at 4:2:0, which
//! [`Frame::to_rgba`] turns into RGB with bilinear chroma upsampling.
//!
//! Interframes are refused. Reads past the end of a partition return zero
//! bits; a partition that runs more than a couple of bytes past its end is
//! reported as truncated. Every loop is bounded by the macroblock count.

use crate::ImageError;

/// Bytes a bool decoder may read past its data before the stream counts as truncated.
///
/// The decoder keeps two bytes of lookahead, so a complete stream can touch
/// up to two bytes beyond its last real byte.
const MAX_OVERREAD: usize = 2;

/// Boolean entropy decoder over one partition.
struct BoolDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    value: u32,
    range: u32,
    bit_count: u32,
    overread: usize,
}

impl<'a> BoolDecoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        let mut d = BoolDecoder {
            data,
            pos: 0,
            value: 0,
            range: 255,
            bit_count: 0,
            overread: 0,
        };
        d.value = (u32::from(d.next_byte()) << 8) | u32::from(d.next_byte());
        d
    }

    fn next_byte(&mut self) -> u8 {
        if let Some(&b) = self.data.get(self.pos) {
            self.pos += 1;
            b
        } else {
            self.overread += 1;
            0
        }
    }

    /// Decode one bool whose probability of being 0 is `prob / 256`.
    fn read_bool(&mut self, prob: u8) -> bool {
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        let big_split = split << 8;
        let bit = if self.value >= big_split {
            self.range -= split;
            self.value -= big_split;
            true
        } else {
            self.range = split;
            false
        };
        while self.range < 128 {
            self.value <<= 1;
            self.range <<= 1;
            self.bit_count += 1;
            if self.bit_count == 8 {
                self.bit_count = 0;
                self.value |= u32::from(self.next_byte());
            }
        }
        bit
    }

    /// Read an `n`-bit unsigned literal, most significant bit first.
    fn read_literal(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | u32::from(self.read_bool(128));
        }
        v
    }

    fn read_flag(&mut self) -> bool {
        self.read_bool(128)
    }

    /// Read an `n`-bit magnitude followed by a sign bit.
    fn read_signed(&mut self, n: u32) -> i32 {
        let v = self.read_literal(n) as i32;
        if self.read_flag() { -v } else { v }
    }

    /// Read a flag; when set, read a signed `n`-bit value, otherwise 0.
    fn read_optional_signed(&mut self, n: u32) -> i32 {
        if self.read_flag() {
            self.read_signed(n)
        } else {
            0
        }
    }

    /// Walk a tree whose leaves are stored as `-symbol` (0 is a leaf too).
    fn read_tree(&mut self, tree: &[i8], probs: &[u8]) -> usize {
        let mut i = 0usize;
        loop {
            let next = tree[i + usize::from(self.read_bool(probs[i >> 1]))];
            if next <= 0 {
                return (-next) as usize;
            }
            i = next as usize;
        }
    }

    fn check(&self) -> Result<(), ImageError> {
        if self.overread > MAX_OVERREAD {
            Err(ImageError::UnexpectedEof)
        } else {
            Ok(())
        }
    }
}

// Luma (16x16) and chroma prediction modes.
const DC_PRED: usize = 0;
const V_PRED: usize = 1;
const H_PRED: usize = 2;
const TM_PRED: usize = 3;
const B_PRED: usize = 4;

// Subblock (4x4) prediction modes, in the order the spec numbers them.
const B_DC_PRED: u8 = 0;
const B_TM_PRED: u8 = 1;
const B_VE_PRED: u8 = 2;
const B_HE_PRED: u8 = 3;
const B_LD_PRED: u8 = 4;
const B_RD_PRED: u8 = 5;
const B_VR_PRED: u8 = 6;
const B_VL_PRED: u8 = 7;
const B_HD_PRED: u8 = 8;
const B_HU_PRED: u8 = 9;

/// Key frame luma mode tree: B_PRED, then DC, V, H, TM.
const KF_YMODE_TREE: [i8; 8] = [
    -(B_PRED as i8),
    2,
    4,
    6,
    -(DC_PRED as i8),
    -(V_PRED as i8),
    -(H_PRED as i8),
    -(TM_PRED as i8),
];
const KF_YMODE_PROBS: [u8; 4] = [145, 156, 163, 128];

const UV_MODE_TREE: [i8; 6] = [
    -(DC_PRED as i8),
    2,
    -(V_PRED as i8),
    4,
    -(H_PRED as i8),
    -(TM_PRED as i8),
];
const KF_UV_MODE_PROBS: [u8; 3] = [142, 114, 183];

const BMODE_TREE: [i8; 18] = [
    -(B_DC_PRED as i8),
    2,
    -(B_TM_PRED as i8),
    4,
    -(B_VE_PRED as i8),
    6,
    8,
    12,
    -(B_HE_PRED as i8),
    10,
    -(B_RD_PRED as i8),
    -(B_VR_PRED as i8),
    -(B_LD_PRED as i8),
    14,
    -(B_VL_PRED as i8),
    16,
    -(B_HD_PRED as i8),
    -(B_HU_PRED as i8),
];

const SEGMENT_TREE: [i8; 6] = [2, 4, 0, -1, -2, -3];

/// Coefficient position to probability band.
const COEFF_BANDS: [usize; 16] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7];

/// Coefficient position (in decode order) to raster position in the 4x4 block.
const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// Extra-bit probabilities for token categories 3 through 6.
const CAT3_PROBS: [u8; 3] = [173, 148, 140];
const CAT4_PROBS: [u8; 4] = [176, 155, 140, 135];
const CAT5_PROBS: [u8; 5] = [180, 157, 141, 134, 130];
const CAT6_PROBS: [u8; 11] = [254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129];

// Block types used to index the token probabilities.
const TYPE_Y_AFTER_Y2: usize = 0;
const TYPE_Y2: usize = 1;
const TYPE_CHROMA: usize = 2;
const TYPE_Y_WITH_DC: usize = 3;

/// Fixed subblock mode probabilities for key frames, indexed by the mode
/// of the subblock above, then the one to the left.
const KF_BMODE_PROBS: [[[u8; 9]; 10]; 10] = [
    [
        [231, 120, 48, 89, 115, 113, 120, 152, 112],
        [152, 179, 64, 126, 170, 118, 46, 70, 95],
        [175, 69, 143, 80, 85, 82, 72, 155, 103],
        [56, 58, 10, 171, 218, 189, 17, 13, 152],
        [144, 71, 10, 38, 171, 213, 144, 34, 26],
        [114, 26, 17, 163, 44, 195, 21, 10, 173],
        [121, 24, 80, 195, 26, 62, 44, 64, 85],
        [170, 46, 55, 19, 136, 160, 33, 206, 71],
        [63, 20, 8, 114, 114, 208, 12, 9, 226],
        [81, 40, 11, 96, 182, 84, 29, 16, 36],
    ],
    [
        [134, 183, 89, 137, 98, 101, 106, 165, 148],
        [72, 187, 100, 130, 157, 111, 32, 75, 80],
        [66, 102, 167, 99, 74, 62, 40, 234, 128],
        [41, 53, 9, 178, 241, 141, 26, 8, 107],
        [104, 79, 12, 27, 217, 255, 87, 17, 7],
        [74, 43, 26, 146, 73, 166, 49, 23, 157],
        [65, 38, 105, 160, 51, 52, 31, 115, 128],
        [87, 68, 71, 44, 114, 51, 15, 186, 23],
        [47, 41, 14, 110, 182, 183, 21, 17, 194],
        [66, 45, 25, 102, 197, 189, 23, 18, 22],
    ],
    [
        [88, 88, 147, 150, 42, 46, 45, 196, 205],
        [43, 97, 183, 117, 85, 38, 35, 179, 61],
        [39, 53, 200, 87, 26, 21, 43, 232, 171],
        [56, 34, 51, 104, 114, 102, 29, 93, 77],
        [107, 54, 32, 26, 51, 1, 81, 43, 31],
        [39, 28, 85, 171, 58, 165, 90, 98, 64],
        [34, 22, 116, 206, 23, 34, 43, 166, 73],
        [68, 25, 106, 22, 64, 171, 36, 225, 114],
        [34, 19, 21, 102, 132, 188, 16, 76, 124],
        [62, 18, 78, 95, 85, 57, 50, 48, 51],
    ],
    [
        [193, 101, 35, 159, 215, 111, 89, 46, 111],
        [60, 148, 31, 172, 219, 228, 21, 18, 111],
        [112, 113, 77, 85, 179, 255, 38, 120, 114],
        [40, 42, 1, 196, 245, 209, 10, 25, 109],
        [100, 80, 8, 43, 154, 1, 51, 26, 71],
        [88, 43, 29, 140, 166, 213, 37, 43, 154],
        [61, 63, 30, 155, 67, 45, 68, 1, 209],
        [142, 78, 78, 16, 255, 128, 34, 197, 171],
        [41, 40, 5, 102, 211, 183, 4, 1, 221],
        [51, 50, 17, 168, 209, 192, 23, 25, 82],
    ],
    [
        [125, 98, 42, 88, 104, 85, 117, 175, 82],
        [95, 84, 53, 89, 128, 100, 113, 101, 45],
        [75, 79, 123, 47, 51, 128, 81, 171, 1],
        [57, 17, 5, 71, 102, 57, 53, 41, 49],
        [115, 21, 2, 10, 102, 255, 166, 23, 6],
        [38, 33, 13, 121, 57, 73, 26, 1, 85],
        [41, 10, 67, 138, 77, 110, 90, 47, 114],
        [101, 29, 16, 10, 85, 128, 101, 196, 26],
        [57, 18, 10, 102, 102, 213, 34, 20, 43],
        [117, 20, 15, 36, 163, 128, 68, 1, 26],
    ],
    [
        [138, 31, 36, 171, 27, 166, 38, 44, 229],
        [67, 87, 58, 169, 82, 115, 26, 59, 179],
        [63, 59, 90, 180, 59, 166, 93, 73, 154],
        [40, 40, 21, 116, 143, 209, 34, 39, 175],
        [57, 46, 22, 24, 128, 1, 54, 17, 37],
        [47, 15, 16, 183, 34, 223, 49, 45, 183],
        [46, 17, 33, 183, 6, 98, 15, 32, 183],
        [65, 32, 73, 115, 28, 128, 23, 128, 205],
        [40, 3, 9, 115, 51, 192, 18, 6, 223],
        [87, 37, 9, 115, 59, 77, 64, 21, 47],
    ],
    [
        [104, 55, 44, 218, 9, 54, 53, 130, 226],
        [64, 90, 70, 205, 40, 41, 23, 26, 57],
        [54, 57, 112, 184, 5, 41, 38, 166, 213],
        [30, 34, 26, 133, 152, 116, 10, 32, 134],
        [75, 32, 12, 51, 192, 255, 160, 43, 51],
        [39, 19, 53, 221, 26, 114, 32, 73, 255],
        [31, 9, 65, 234, 2, 15, 1, 118, 73],
        [88, 31, 35, 67, 102, 85, 55, 186, 85],
        [56, 21, 23, 111, 59, 205, 45, 37, 192],
        [55, 38, 70, 124, 73, 102, 1, 34, 98],
    ],
    [
        [102, 61, 71, 37, 34, 53, 31, 243, 192],
        [69, 60, 71, 38, 73, 119, 28, 222, 37],
        [68, 45, 128, 34, 1, 47, 11, 245, 171],
        [62, 17, 19, 70, 146, 85, 55, 62, 70],
        [75, 15, 9, 9, 64, 255, 184, 119, 16],
        [37, 43, 37, 154, 100, 163, 85, 160, 1],
        [63, 9, 92, 136, 28, 64, 32, 201, 85],
        [86, 6, 28, 5, 64, 255, 25, 248, 1],
        [56, 8, 17, 132, 137, 255, 55, 116, 128],
        [58, 15, 20, 82, 135, 57, 26, 121, 40],
    ],
    [
        [164, 50, 31, 137, 154, 133, 25, 35, 218],
        [51, 103, 44, 131, 131, 123, 31, 6, 158],
        [86, 40, 64, 135, 148, 224, 45, 183, 128],
        [22, 26, 17, 131, 240, 154, 14, 1, 209],
        [83, 12, 13, 54, 192, 255, 68, 47, 28],
        [45, 16, 21, 91, 64, 222, 7, 1, 197],
        [56, 21, 39, 155, 60, 138, 23, 102, 213],
        [85, 26, 85, 85, 128, 128, 32, 146, 171],
        [18, 11, 7, 63, 144, 171, 4, 4, 246],
        [35, 27, 10, 146, 174, 171, 12, 26, 128],
    ],
    [
        [190, 80, 35, 99, 180, 80, 126, 54, 45],
        [85, 126, 47, 87, 176, 51, 41, 20, 32],
        [101, 75, 128, 139, 118, 146, 116, 128, 85],
        [56, 41, 15, 176, 236, 85, 37, 9, 62],
        [146, 36, 19, 30, 171, 255, 97, 27, 20],
        [71, 30, 17, 119, 118, 255, 17, 18, 138],
        [101, 38, 60, 138, 55, 70, 43, 26, 142],
        [138, 45, 61, 62, 219, 1, 81, 188, 64],
        [32, 41, 20, 117, 151, 142, 20, 21, 163],
        [112, 19, 12, 61, 195, 128, 48, 4, 24],
    ],
];

/// Probability that each token probability is replaced in the frame header.
const COEFF_UPDATE_PROBS: [[[[u8; 11]; 3]; 8]; 4] = [
    [
        [
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [176, 246, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [223, 241, 252, 255, 255, 255, 255, 255, 255, 255, 255],
            [249, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 244, 252, 255, 255, 255, 255, 255, 255, 255, 255],
            [234, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 246, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [239, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [251, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [251, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 254, 253, 255, 254, 255, 255, 255, 255, 255, 255],
            [250, 255, 254, 255, 254, 255, 255, 255, 255, 255, 255],
            [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
    ],
    [
        [
            [217, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [225, 252, 241, 253, 255, 255, 254, 255, 255, 255, 255],
            [234, 250, 241, 250, 253, 255, 253, 254, 255, 255, 255],
        ],
        [
            [255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [223, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [238, 253, 254, 254, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [249, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [247, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [252, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255],
            [250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
    ],
    [
        [
            [186, 251, 250, 255, 255, 255, 255, 255, 255, 255, 255],
            [234, 251, 244, 254, 255, 255, 255, 255, 255, 255, 255],
            [251, 251, 243, 253, 254, 255, 254, 255, 255, 255, 255],
        ],
        [
            [255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [236, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [251, 253, 253, 254, 254, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [254, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
    ],
    [
        [
            [248, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [250, 254, 252, 254, 255, 255, 255, 255, 255, 255, 255],
            [248, 254, 249, 253, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255],
            [246, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255],
            [252, 254, 251, 254, 254, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 254, 252, 255, 255, 255, 255, 255, 255, 255, 255],
            [248, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255],
            [253, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [245, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [253, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 251, 253, 255, 255, 255, 255, 255, 255, 255, 255],
            [252, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 252, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [249, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255],
            [250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
        [
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        ],
    ],
];

/// Token probabilities in force at the start of every key frame.
const DEFAULT_COEFF_PROBS: [[[[u8; 11]; 3]; 8]; 4] = [
    [
        [
            [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
            [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
            [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
        ],
        [
            [253, 136, 254, 255, 228, 219, 128, 128, 128, 128, 128],
            [189, 129, 242, 255, 227, 213, 255, 219, 128, 128, 128],
            [106, 126, 227, 252, 214, 209, 255, 255, 128, 128, 128],
        ],
        [
            [1, 98, 248, 255, 236, 226, 255, 255, 128, 128, 128],
            [181, 133, 238, 254, 221, 234, 255, 154, 128, 128, 128],
            [78, 134, 202, 247, 198, 180, 255, 219, 128, 128, 128],
        ],
        [
            [1, 185, 249, 255, 243, 255, 128, 128, 128, 128, 128],
            [184, 150, 247, 255, 236, 224, 128, 128, 128, 128, 128],
            [77, 110, 216, 255, 236, 230, 128, 128, 128, 128, 128],
        ],
        [
            [1, 101, 251, 255, 241, 255, 128, 128, 128, 128, 128],
            [170, 139, 241, 252, 236, 209, 255, 255, 128, 128, 128],
            [37, 116, 196, 243, 228, 255, 255, 255, 128, 128, 128],
        ],
        [
            [1, 204, 254, 255, 245, 255, 128, 128, 128, 128, 128],
            [207, 160, 250, 255, 238, 128, 128, 128, 128, 128, 128],
            [102, 103, 231, 255, 211, 171, 128, 128, 128, 128, 128],
        ],
        [
            [1, 152, 252, 255, 240, 255, 128, 128, 128, 128, 128],
            [177, 135, 243, 255, 234, 225, 128, 128, 128, 128, 128],
            [80, 129, 211, 255, 194, 224, 128, 128, 128, 128, 128],
        ],
        [
            [1, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
            [246, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
            [255, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
        ],
    ],
    [
        [
            [198, 35, 237, 223, 193, 187, 162, 160, 145, 155, 62],
            [131, 45, 198, 221, 172, 176, 220, 157, 252, 221, 1],
            [68, 47, 146, 208, 149, 167, 221, 162, 255, 223, 128],
        ],
        [
            [1, 149, 241, 255, 221, 224, 255, 255, 128, 128, 128],
            [184, 141, 234, 253, 222, 220, 255, 199, 128, 128, 128],
            [81, 99, 181, 242, 176, 190, 249, 202, 255, 255, 128],
        ],
        [
            [1, 129, 232, 253, 214, 197, 242, 196, 255, 255, 128],
            [99, 121, 210, 250, 201, 198, 255, 202, 128, 128, 128],
            [23, 91, 163, 242, 170, 187, 247, 210, 255, 255, 128],
        ],
        [
            [1, 200, 246, 255, 234, 255, 128, 128, 128, 128, 128],
            [109, 178, 241, 255, 231, 245, 255, 255, 128, 128, 128],
            [44, 130, 201, 253, 205, 192, 255, 255, 128, 128, 128],
        ],
        [
            [1, 132, 239, 251, 219, 209, 255, 165, 128, 128, 128],
            [94, 136, 225, 251, 218, 190, 255, 255, 128, 128, 128],
            [22, 100, 174, 245, 186, 161, 255, 199, 128, 128, 128],
        ],
        [
            [1, 182, 249, 255, 232, 235, 128, 128, 128, 128, 128],
            [124, 143, 241, 255, 227, 234, 128, 128, 128, 128, 128],
            [35, 77, 181, 251, 193, 211, 255, 205, 128, 128, 128],
        ],
        [
            [1, 157, 247, 255, 236, 231, 255, 255, 128, 128, 128],
            [121, 141, 235, 255, 225, 227, 255, 255, 128, 128, 128],
            [45, 99, 188, 251, 195, 217, 255, 224, 128, 128, 128],
        ],
        [
            [1, 1, 251, 255, 213, 255, 128, 128, 128, 128, 128],
            [203, 1, 248, 255, 255, 128, 128, 128, 128, 128, 128],
            [137, 1, 177, 255, 224, 255, 128, 128, 128, 128, 128],
        ],
    ],
    [
        [
            [253, 9, 248, 251, 207, 208, 255, 192, 128, 128, 128],
            [175, 13, 224, 243, 193, 185, 249, 198, 255, 255, 128],
            [73, 17, 171, 221, 161, 179, 236, 167, 255, 234, 128],
        ],
        [
            [1, 95, 247, 253, 212, 183, 255, 255, 128, 128, 128],
            [239, 90, 244, 250, 211, 209, 255, 255, 128, 128, 128],
            [155, 77, 195, 248, 188, 195, 255, 255, 128, 128, 128],
        ],
        [
            [1, 24, 239, 251, 218, 219, 255, 205, 128, 128, 128],
            [201, 51, 219, 255, 196, 186, 128, 128, 128, 128, 128],
            [69, 46, 190, 239, 201, 218, 255, 228, 128, 128, 128],
        ],
        [
            [1, 191, 251, 255, 255, 128, 128, 128, 128, 128, 128],
            [223, 165, 249, 255, 213, 255, 128, 128, 128, 128, 128],
            [141, 124, 248, 255, 255, 128, 128, 128, 128, 128, 128],
        ],
        [
            [1, 16, 248, 255, 255, 128, 128, 128, 128, 128, 128],
            [190, 36, 230, 255, 236, 255, 128, 128, 128, 128, 128],
            [149, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
        ],
        [
            [1, 226, 255, 128, 128, 128, 128, 128, 128, 128, 128],
            [247, 192, 255, 128, 128, 128, 128, 128, 128, 128, 128],
            [240, 128, 255, 128, 128, 128, 128, 128, 128, 128, 128],
        ],
        [
            [1, 134, 252, 255, 255, 128, 128, 128, 128, 128, 128],
            [213, 62, 250, 255, 255, 128, 128, 128, 128, 128, 128],
            [55, 93, 255, 128, 128, 128, 128, 128, 128, 128, 128],
        ],
        [
            [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
            [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
            [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
        ],
    ],
    [
        [
            [202, 24, 213, 235, 186, 191, 220, 160, 240, 175, 255],
            [126, 38, 182, 232, 169, 184, 228, 174, 255, 187, 128],
            [61, 46, 138, 219, 151, 178, 240, 170, 255, 216, 128],
        ],
        [
            [1, 112, 230, 250, 199, 191, 247, 159, 255, 255, 128],
            [166, 109, 228, 252, 211, 215, 255, 174, 128, 128, 128],
            [39, 77, 162, 232, 172, 180, 245, 178, 255, 255, 128],
        ],
        [
            [1, 52, 220, 246, 198, 199, 249, 220, 255, 255, 128],
            [124, 74, 191, 243, 183, 193, 250, 221, 255, 255, 128],
            [24, 71, 130, 219, 154, 170, 243, 182, 255, 255, 128],
        ],
        [
            [1, 182, 225, 249, 219, 240, 255, 224, 128, 128, 128],
            [149, 150, 226, 252, 216, 205, 255, 171, 128, 128, 128],
            [28, 108, 170, 242, 183, 194, 254, 223, 255, 255, 128],
        ],
        [
            [1, 81, 230, 252, 204, 203, 255, 192, 128, 128, 128],
            [123, 102, 209, 247, 188, 196, 255, 233, 128, 128, 128],
            [20, 95, 153, 243, 164, 173, 255, 203, 128, 128, 128],
        ],
        [
            [1, 222, 248, 255, 216, 213, 128, 128, 128, 128, 128],
            [168, 175, 246, 252, 235, 205, 255, 255, 128, 128, 128],
            [47, 116, 215, 255, 211, 212, 255, 255, 128, 128, 128],
        ],
        [
            [1, 121, 236, 253, 212, 214, 255, 255, 128, 128, 128],
            [141, 84, 213, 252, 201, 202, 255, 219, 128, 128, 128],
            [42, 80, 160, 240, 162, 185, 255, 205, 128, 128, 128],
        ],
        [
            [1, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
            [244, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
            [238, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
        ],
    ],
];

/// Dequantization factor for DC coefficients, by quantizer index.
const DC_QUANT: [i32; 128] = [
    4, 5, 6, 7, 8, 9, 10, 10, 11, 12, 13, 14, 15, 16, 17, 17, 18, 19, 20, 20, 21, 21, 22, 22, 23,
    23, 24, 25, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 37, 38, 39, 40, 41, 42, 43, 44,
    45, 46, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67,
    68, 69, 70, 71, 72, 73, 74, 75, 76, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 91,
    93, 95, 96, 98, 100, 101, 102, 104, 106, 108, 110, 112, 114, 116, 118, 122, 124, 126, 128, 130,
    132, 134, 136, 138, 140, 143, 145, 148, 151, 154, 157,
];

/// Dequantization factor for AC coefficients, by quantizer index.
const AC_QUANT: [i32; 128] = [
    4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
    29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52,
    53, 54, 55, 56, 57, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88, 90, 92, 94,
    96, 98, 100, 102, 104, 106, 108, 110, 112, 114, 116, 119, 122, 125, 128, 131, 134, 137, 140,
    143, 146, 149, 152, 155, 158, 161, 164, 167, 170, 173, 177, 181, 185, 189, 193, 197, 201, 205,
    209, 213, 217, 221, 225, 229, 234, 239, 245, 249, 254, 259, 264, 269, 274, 279, 284,
];
/// Dequantization factors for one segment.
#[derive(Clone, Copy, Default, Debug)]
struct Quant {
    y_dc: i32,
    y_ac: i32,
    y2_dc: i32,
    y2_ac: i32,
    uv_dc: i32,
    uv_ac: i32,
}

/// Everything the frame header sets that the macroblock loop needs.
struct Header {
    update_map: bool,
    segment_probs: [u8; 3],
    quant: [Quant; 4],
    filter_levels: [i32; 4],
    simple_filter: bool,
    sharpness: i32,
    frame_filter_level: i32,
    lf_delta: bool,
    ref_delta0: i32,
    mode_delta0: i32,
    coeff_probs: [[[[u8; 11]; 3]; 8]; 4],
    skip_prob: Option<u8>,
}

/// Look up a quantizer table entry with the index clamped to `0..=127`.
fn quant_lookup(table: &[i32; 128], index: i32) -> i32 {
    table[index.clamp(0, 127) as usize]
}

fn read_header(d: &mut BoolDecoder<'_>) -> (Header, usize) {
    let _color_space = d.read_flag();
    let _clamping = d.read_flag();

    let segmentation = d.read_flag();
    let mut update_map = false;
    let mut absolute = false;
    let mut seg_quant = [0i32; 4];
    let mut seg_lf = [0i32; 4];
    let mut segment_probs = [255u8; 3];
    if segmentation {
        update_map = d.read_flag();
        let update_data = d.read_flag();
        if update_data {
            absolute = d.read_flag();
            for q in &mut seg_quant {
                *q = d.read_optional_signed(7);
            }
            for lf in &mut seg_lf {
                *lf = d.read_optional_signed(6);
            }
        }
        if update_map {
            for p in &mut segment_probs {
                *p = if d.read_flag() {
                    d.read_literal(8) as u8
                } else {
                    255
                };
            }
        }
    }

    let simple_filter = d.read_flag();
    let frame_filter_level = d.read_literal(6) as i32;
    let sharpness = d.read_literal(3) as i32;
    let lf_delta = d.read_flag();
    let mut ref_delta0 = 0;
    let mut mode_delta0 = 0;
    if lf_delta && d.read_flag() {
        // Four reference-frame deltas, then four mode deltas. A key frame
        // only uses the first of each (intra frame, and B_PRED).
        for i in 0..4 {
            let v = d.read_optional_signed(6);
            if i == 0 {
                ref_delta0 = v;
            }
        }
        for i in 0..4 {
            let v = d.read_optional_signed(6);
            if i == 0 {
                mode_delta0 = v;
            }
        }
    }

    let partitions = 1usize << d.read_literal(2);

    let base_q = d.read_literal(7) as i32;
    let y_dc_delta = d.read_optional_signed(4);
    let y2_dc_delta = d.read_optional_signed(4);
    let y2_ac_delta = d.read_optional_signed(4);
    let uv_dc_delta = d.read_optional_signed(4);
    let uv_ac_delta = d.read_optional_signed(4);

    let mut quant = [Quant::default(); 4];
    let mut filter_levels = [frame_filter_level; 4];
    for s in 0..4 {
        let q = if !segmentation {
            base_q
        } else if absolute {
            seg_quant[s]
        } else {
            base_q + seg_quant[s]
        };
        quant[s] = Quant {
            y_dc: quant_lookup(&DC_QUANT, q + y_dc_delta),
            y_ac: quant_lookup(&AC_QUANT, q),
            y2_dc: quant_lookup(&DC_QUANT, q + y2_dc_delta) * 2,
            y2_ac: (quant_lookup(&AC_QUANT, q + y2_ac_delta) * 155 / 100).max(8),
            uv_dc: quant_lookup(&DC_QUANT, q + uv_dc_delta).min(132),
            uv_ac: quant_lookup(&AC_QUANT, q + uv_ac_delta),
        };
        if segmentation {
            filter_levels[s] = if absolute {
                seg_lf[s]
            } else {
                frame_filter_level + seg_lf[s]
            };
        }
    }

    // Key frames carry refresh_entropy_probs; the value does not matter for
    // a single frame.
    let _refresh_probs = d.read_flag();

    let mut coeff_probs = DEFAULT_COEFF_PROBS;
    for (i, plane) in coeff_probs.iter_mut().enumerate() {
        for (j, band) in plane.iter_mut().enumerate() {
            for (k, ctx) in band.iter_mut().enumerate() {
                for (l, prob) in ctx.iter_mut().enumerate() {
                    if d.read_bool(COEFF_UPDATE_PROBS[i][j][k][l]) {
                        *prob = d.read_literal(8) as u8;
                    }
                }
            }
        }
    }

    let skip_prob = if d.read_flag() {
        Some(d.read_literal(8) as u8)
    } else {
        None
    };

    (
        Header {
            update_map,
            segment_probs,
            quant,
            filter_levels,
            simple_filter,
            sharpness,
            frame_filter_level,
            lf_delta,
            ref_delta0,
            mode_delta0,
            coeff_probs,
            skip_prob,
        },
        partitions,
    )
}

/// Split the bytes after the first partition into `count` token partitions.
fn split_partitions(data: &[u8], count: usize) -> Result<Vec<&[u8]>, ImageError> {
    let table = 3 * (count - 1);
    if data.len() < table {
        return Err(ImageError::UnexpectedEof);
    }
    let mut rest = &data[table..];
    let mut parts = Vec::with_capacity(count);
    for i in 0..count - 1 {
        let size = usize::from(data[3 * i])
            | (usize::from(data[3 * i + 1]) << 8)
            | (usize::from(data[3 * i + 2]) << 16);
        if size > rest.len() {
            return Err(ImageError::UnexpectedEof);
        }
        parts.push(&rest[..size]);
        rest = &rest[size..];
    }
    parts.push(rest);
    Ok(parts)
}

/// "Has non-zero data" flags for the blocks along one macroblock edge.
#[derive(Clone, Copy, Default)]
struct NonZero {
    y: [bool; 4],
    u: [bool; 2],
    v: [bool; 2],
    y2: bool,
}

/// Read a token value of 2 or more, given the probabilities at the current position.
fn read_large_value(d: &mut BoolDecoder<'_>, p: &[u8; 11]) -> i32 {
    if !d.read_bool(p[3]) {
        if !d.read_bool(p[4]) {
            2
        } else {
            3 + i32::from(d.read_bool(p[5]))
        }
    } else if !d.read_bool(p[6]) {
        if !d.read_bool(p[7]) {
            5 + i32::from(d.read_bool(159))
        } else {
            7 + 2 * i32::from(d.read_bool(165)) + i32::from(d.read_bool(145))
        }
    } else {
        let high = usize::from(d.read_bool(p[8]));
        let low = usize::from(d.read_bool(p[9 + high]));
        let (probs, base): (&[u8], i32) = match 2 * high + low {
            0 => (&CAT3_PROBS, 11),
            1 => (&CAT4_PROBS, 19),
            2 => (&CAT5_PROBS, 35),
            _ => (&CAT6_PROBS, 67),
        };
        let mut extra = 0;
        for &prob in probs {
            extra = 2 * extra + i32::from(d.read_bool(prob));
        }
        base + extra
    }
}

/// Decode the tokens of one 4x4 block into `out` (raster order, dequantized).
///
/// Returns the position after the last token read. The caller treats a
/// value above `first` as "this block has data" for the next context.
fn read_block(
    d: &mut BoolDecoder<'_>,
    probs: &[[[u8; 11]; 3]; 8],
    ctx: usize,
    first: usize,
    dc_q: i32,
    ac_q: i32,
    out: &mut [i32; 16],
) -> usize {
    let mut n = first;
    let mut p = &probs[COEFF_BANDS[n]][ctx];
    if !d.read_bool(p[0]) {
        return n;
    }
    loop {
        while !d.read_bool(p[1]) {
            n += 1;
            if n == 16 {
                return 16;
            }
            p = &probs[COEFF_BANDS[n]][0];
        }
        let (value, next_ctx) = if !d.read_bool(p[2]) {
            (1, 1)
        } else {
            (read_large_value(d, p), 2)
        };
        let value = if d.read_flag() { -value } else { value };
        out[ZIGZAG[n]] = value * if n > 0 { ac_q } else { dc_q };
        n += 1;
        if n == 16 {
            return 16;
        }
        p = &probs[COEFF_BANDS[n]][next_ctx];
        if !d.read_bool(p[0]) {
            return n;
        }
    }
}

/// Inverse Walsh-Hadamard transform of the Y2 block into 16 luma DC values.
fn inverse_wht(input: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let a = input[i] + input[12 + i];
        let b = input[4 + i] + input[8 + i];
        let c = input[4 + i] - input[8 + i];
        let d = input[i] - input[12 + i];
        tmp[i] = a + b;
        tmp[4 + i] = c + d;
        tmp[8 + i] = a - b;
        tmp[12 + i] = d - c;
    }
    let mut out = [0i32; 16];
    for r in 0..4 {
        let row = &tmp[4 * r..4 * r + 4];
        let a = row[0] + row[3];
        let b = row[1] + row[2];
        let c = row[1] - row[2];
        let d = row[0] - row[3];
        out[4 * r] = (a + b + 3) >> 3;
        out[4 * r + 1] = (c + d + 3) >> 3;
        out[4 * r + 2] = (a - b + 3) >> 3;
        out[4 * r + 3] = (d - c + 3) >> 3;
    }
    out
}

/// `x * sqrt(2) * cos(pi / 8)` in the spec's 16-bit fixed point.
fn mul_cos(x: i32) -> i32 {
    x + ((i64::from(x) * 20091) >> 16) as i32
}

/// `x * sqrt(2) * sin(pi / 8)` in the spec's 16-bit fixed point.
fn mul_sin(x: i32) -> i32 {
    ((i64::from(x) * 35468) >> 16) as i32
}

/// Inverse 4x4 DCT of `coeffs`, added to the predicted pixels at `pos`.
fn idct_add(coeffs: &[i32; 16], plane: &mut [u8], pos: usize, stride: usize) {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let a = coeffs[i] + coeffs[8 + i];
        let b = coeffs[i] - coeffs[8 + i];
        let c = mul_sin(coeffs[4 + i]) - mul_cos(coeffs[12 + i]);
        let d = mul_cos(coeffs[4 + i]) + mul_sin(coeffs[12 + i]);
        tmp[i] = a + d;
        tmp[4 + i] = b + c;
        tmp[8 + i] = b - c;
        tmp[12 + i] = a - d;
    }
    for r in 0..4 {
        let row = &tmp[4 * r..4 * r + 4];
        let a = row[0] + row[2];
        let b = row[0] - row[2];
        let c = mul_sin(row[1]) - mul_cos(row[3]);
        let d = mul_cos(row[1]) + mul_sin(row[3]);
        let residual = [
            (a + d + 4) >> 3,
            (b + c + 4) >> 3,
            (b - c + 4) >> 3,
            (a - d + 4) >> 3,
        ];
        let at = pos + r * stride;
        for (c, &res) in residual.iter().enumerate() {
            plane[at + c] = clamp255(i32::from(plane[at + c]) + res);
        }
    }
}

fn clamp255(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn avg2(a: u8, b: u8) -> u8 {
    ((u16::from(a) + u16::from(b) + 1) >> 1) as u8
}

fn avg3(a: u8, b: u8, c: u8) -> u8 {
    ((u16::from(a) + 2 * u16::from(b) + u16::from(c) + 2) >> 2) as u8
}

/// Fill a `size` x `size` block with one of the whole-block modes.
///
/// `top[0]` is the corner pixel and `top[1..=size]` the row above; `left`
/// is the column to the left. `has_top` / `has_left` say whether those
/// edges lie inside the frame, which only DC prediction cares about.
#[allow(clippy::too_many_arguments)]
fn predict_block(
    plane: &mut [u8],
    pos: usize,
    stride: usize,
    size: usize,
    mode: usize,
    top: &[u8],
    left: &[u8],
    has_top: bool,
    has_left: bool,
) {
    let shift = size.trailing_zeros();
    match mode {
        V_PRED => {
            for r in 0..size {
                plane[pos + r * stride..pos + r * stride + size].copy_from_slice(&top[1..=size]);
            }
        }
        H_PRED => {
            for r in 0..size {
                plane[pos + r * stride..pos + r * stride + size].fill(left[r]);
            }
        }
        TM_PRED => {
            let corner = i32::from(top[0]);
            for r in 0..size {
                let l = i32::from(left[r]) - corner;
                for c in 0..size {
                    plane[pos + r * stride + c] = clamp255(l + i32::from(top[1 + c]));
                }
            }
        }
        _ => {
            let sum_top: u32 = top[1..=size].iter().map(|&v| u32::from(v)).sum();
            let sum_left: u32 = left[..size].iter().map(|&v| u32::from(v)).sum();
            let value = match (has_top, has_left) {
                (true, true) => (sum_top + sum_left + size as u32) >> (shift + 1),
                (true, false) => (sum_top + (size as u32 >> 1)) >> shift,
                (false, true) => (sum_left + (size as u32 >> 1)) >> shift,
                (false, false) => 128,
            };
            for r in 0..size {
                plane[pos + r * stride..pos + r * stride + size].fill(value as u8);
            }
        }
    }
}

/// Predict one 4x4 luma subblock.
///
/// `above[0]` is the corner pixel P, `above[1..9]` the eight pixels above
/// and above-right; `left` is the column to the left.
fn predict_subblock(out: &mut [[u8; 4]; 4], mode: u8, above: &[u8; 9], left: &[u8; 4]) {
    let p = above[0];
    let a = &above[1..9];
    let l = left;
    // Edge pixels from bottom-left, up through the corner, then across the top.
    let e = [l[3], l[2], l[1], l[0], p, a[0], a[1], a[2], a[3]];
    match mode {
        B_DC_PRED => {
            let sum: u32 = a[..4].iter().chain(l.iter()).map(|&v| u32::from(v)).sum();
            let v = ((sum + 4) >> 3) as u8;
            *out = [[v; 4]; 4];
        }
        B_TM_PRED => {
            for (row, &left) in out.iter_mut().zip(l.iter()) {
                for (px, &above) in row.iter_mut().zip(a.iter()) {
                    *px = clamp255(i32::from(left) + i32::from(above) - i32::from(p));
                }
            }
        }
        B_VE_PRED => {
            for c in 0..4 {
                let prev = if c == 0 { p } else { a[c - 1] };
                let v = avg3(prev, a[c], a[c + 1]);
                for row in out.iter_mut() {
                    row[c] = v;
                }
            }
        }
        B_HE_PRED => {
            let rows = [
                avg3(p, l[0], l[1]),
                avg3(l[0], l[1], l[2]),
                avg3(l[1], l[2], l[3]),
                avg3(l[2], l[3], l[3]),
            ];
            for (row, &v) in out.iter_mut().zip(rows.iter()) {
                *row = [v; 4];
            }
        }
        B_LD_PRED => {
            for (r, row) in out.iter_mut().enumerate() {
                for (c, px) in row.iter_mut().enumerate() {
                    let i = r + c;
                    *px = if i == 6 {
                        avg3(a[6], a[7], a[7])
                    } else {
                        avg3(a[i], a[i + 1], a[i + 2])
                    };
                }
            }
        }
        B_RD_PRED => {
            for (r, row) in out.iter_mut().enumerate() {
                for (c, px) in row.iter_mut().enumerate() {
                    let i = 4 + c - r;
                    *px = avg3(e[i - 1], e[i], e[i + 1]);
                }
            }
        }
        B_VR_PRED => {
            out[3][0] = avg3(e[1], e[2], e[3]);
            out[2][0] = avg3(e[2], e[3], e[4]);
            out[3][1] = avg3(e[3], e[4], e[5]);
            out[1][0] = out[3][1];
            out[2][1] = avg2(e[4], e[5]);
            out[0][0] = out[2][1];
            out[3][2] = avg3(e[4], e[5], e[6]);
            out[1][1] = out[3][2];
            out[2][2] = avg2(e[5], e[6]);
            out[0][1] = out[2][2];
            out[3][3] = avg3(e[5], e[6], e[7]);
            out[1][2] = out[3][3];
            out[2][3] = avg2(e[6], e[7]);
            out[0][2] = out[2][3];
            out[1][3] = avg3(e[6], e[7], e[8]);
            out[0][3] = avg2(e[7], e[8]);
        }
        B_VL_PRED => {
            out[0][0] = avg2(a[0], a[1]);
            out[1][0] = avg3(a[0], a[1], a[2]);
            out[2][0] = avg2(a[1], a[2]);
            out[0][1] = out[2][0];
            out[1][1] = avg3(a[1], a[2], a[3]);
            out[3][0] = out[1][1];
            out[2][1] = avg2(a[2], a[3]);
            out[0][2] = out[2][1];
            out[3][1] = avg3(a[2], a[3], a[4]);
            out[1][2] = out[3][1];
            out[2][2] = avg2(a[3], a[4]);
            out[0][3] = out[2][2];
            out[3][2] = avg3(a[3], a[4], a[5]);
            out[1][3] = out[3][2];
            out[2][3] = avg3(a[4], a[5], a[6]);
            out[3][3] = avg3(a[5], a[6], a[7]);
        }
        B_HD_PRED => {
            out[3][0] = avg2(e[0], e[1]);
            out[3][1] = avg3(e[0], e[1], e[2]);
            out[2][0] = avg2(e[1], e[2]);
            out[3][2] = out[2][0];
            out[2][1] = avg3(e[1], e[2], e[3]);
            out[3][3] = out[2][1];
            out[2][2] = avg2(e[2], e[3]);
            out[1][0] = out[2][2];
            out[2][3] = avg3(e[2], e[3], e[4]);
            out[1][1] = out[2][3];
            out[1][2] = avg2(e[3], e[4]);
            out[0][0] = out[1][2];
            out[1][3] = avg3(e[3], e[4], e[5]);
            out[0][1] = out[1][3];
            out[0][2] = avg3(e[4], e[5], e[6]);
            out[0][3] = avg3(e[5], e[6], e[7]);
        }
        _ => {
            // B_HU_PRED
            out[0][0] = avg2(l[0], l[1]);
            out[0][1] = avg3(l[0], l[1], l[2]);
            out[0][2] = avg2(l[1], l[2]);
            out[1][0] = out[0][2];
            out[0][3] = avg3(l[1], l[2], l[3]);
            out[1][1] = out[0][3];
            out[1][2] = avg2(l[2], l[3]);
            out[2][0] = out[1][2];
            out[1][3] = avg3(l[2], l[3], l[3]);
            out[2][1] = out[1][3];
            out[2][2] = l[3];
            out[2][3] = l[3];
            out[3] = [l[3]; 4];
        }
    }
}

/// Decoded key frame: Y at full size, U and V at half size, all padded to
/// whole macroblocks.
pub(crate) struct Frame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) y: Vec<u8>,
    pub(crate) u: Vec<u8>,
    pub(crate) v: Vec<u8>,
    pub(crate) y_stride: usize,
    pub(crate) uv_stride: usize,
}

/// Loop filter settings for one macroblock.
#[derive(Clone, Copy, Default)]
struct MbFilter {
    level: u8,
    inner: bool,
}

/// Read the size from the ten-byte key frame header without decoding.
pub(crate) fn read_dimensions(data: &[u8]) -> Result<(u32, u32), ImageError> {
    if data.len() < 10 {
        return Err(ImageError::UnexpectedEof);
    }
    let tag = u32::from(data[0]) | (u32::from(data[1]) << 8) | (u32::from(data[2]) << 16);
    if tag & 1 != 0 {
        return Err(ImageError::UnsupportedFormat(
            "VP8 interframes are not supported",
        ));
    }
    if (tag >> 1) & 7 > 3 {
        return Err(ImageError::UnsupportedFormat("unknown VP8 version"));
    }
    if data[3..6] != [0x9D, 0x01, 0x2A] {
        return Err(ImageError::InvalidHeader("missing VP8 start code"));
    }
    // The top two bits of each size are an upscaling hint, which is ignored.
    let width = u32::from(u16::from_le_bytes([data[6], data[7]]) & 0x3FFF);
    let height = u32::from(u16::from_le_bytes([data[8], data[9]]) & 0x3FFF);
    crate::validate_dimensions(width, height)?;
    Ok((width, height))
}

/// Decode a `VP8 ` chunk payload (one key frame) into YUV planes.
pub(crate) fn decode(data: &[u8]) -> Result<Frame, ImageError> {
    let (width, height) = read_dimensions(data)?;
    let tag = u32::from(data[0]) | (u32::from(data[1]) << 8) | (u32::from(data[2]) << 16);
    let first_size = (tag >> 5) as usize;
    let body = &data[10..];
    if first_size > body.len() {
        return Err(ImageError::UnexpectedEof);
    }
    let mut d = BoolDecoder::new(&body[..first_size]);
    let (header, partition_count) = read_header(&mut d);
    d.check()?;
    let parts = split_partitions(&body[first_size..], partition_count)?;
    let mut tokens: Vec<BoolDecoder<'_>> = parts.into_iter().map(BoolDecoder::new).collect();

    let mb_w = (width as usize).div_ceil(16);
    let mb_h = (height as usize).div_ceil(16);
    let y_stride = mb_w * 16;
    let uv_stride = mb_w * 8;
    let mut frame = Frame {
        width,
        height,
        y: vec![0; y_stride * mb_h * 16],
        u: vec![0; uv_stride * mb_h * 8],
        v: vec![0; uv_stride * mb_h * 8],
        y_stride,
        uv_stride,
    };

    let mut above_modes = vec![[B_DC_PRED; 4]; mb_w];
    let mut above_nz = vec![NonZero::default(); mb_w];
    let mut filters = vec![MbFilter::default(); mb_w * mb_h];

    for mb_y in 0..mb_h {
        let mut left_modes = [B_DC_PRED; 4];
        let mut left_nz = NonZero::default();
        let token_index = mb_y & (partition_count - 1);
        for mb_x in 0..mb_w {
            // Macroblock header, from the first partition.
            let segment = if header.update_map {
                d.read_tree(&SEGMENT_TREE, &header.segment_probs)
            } else {
                0
            };
            let skip = match header.skip_prob {
                Some(prob) => d.read_bool(prob),
                None => false,
            };
            let y_mode = d.read_tree(&KF_YMODE_TREE, &KF_YMODE_PROBS);
            let mut sub_modes = [B_DC_PRED; 16];
            if y_mode == B_PRED {
                for i in 0..16 {
                    let above = if i < 4 {
                        above_modes[mb_x][i]
                    } else {
                        sub_modes[i - 4]
                    };
                    let left = if i % 4 == 0 {
                        left_modes[i / 4]
                    } else {
                        sub_modes[i - 1]
                    };
                    sub_modes[i] = d.read_tree(
                        &BMODE_TREE,
                        &KF_BMODE_PROBS[usize::from(above)][usize::from(left)],
                    ) as u8;
                }
            } else {
                let implied = match y_mode {
                    V_PRED => B_VE_PRED,
                    H_PRED => B_HE_PRED,
                    TM_PRED => B_TM_PRED,
                    _ => B_DC_PRED,
                };
                sub_modes = [implied; 16];
            }
            above_modes[mb_x] = [sub_modes[12], sub_modes[13], sub_modes[14], sub_modes[15]];
            left_modes = [sub_modes[3], sub_modes[7], sub_modes[11], sub_modes[15]];
            let uv_mode = d.read_tree(&UV_MODE_TREE, &KF_UV_MODE_PROBS);

            // Residual tokens, from this row's token partition.
            let q = header.quant[segment];
            let has_y2 = y_mode != B_PRED;
            let mut coeffs = [[0i32; 16]; 24];
            let mut has_data = false;
            let top = &mut above_nz[mb_x];
            if skip {
                top.y = [false; 4];
                top.u = [false; 2];
                top.v = [false; 2];
                left_nz.y = [false; 4];
                left_nz.u = [false; 2];
                left_nz.v = [false; 2];
                if has_y2 {
                    top.y2 = false;
                    left_nz.y2 = false;
                }
            } else {
                let t = &mut tokens[token_index];
                let probs = &header.coeff_probs;
                let (y_type, first) = if has_y2 {
                    let mut y2 = [0i32; 16];
                    let ctx = usize::from(top.y2) + usize::from(left_nz.y2);
                    let n = read_block(t, &probs[TYPE_Y2], ctx, 0, q.y2_dc, q.y2_ac, &mut y2);
                    let nz = n > 0;
                    top.y2 = nz;
                    left_nz.y2 = nz;
                    has_data |= nz;
                    let dc = inverse_wht(&y2);
                    for (block, &value) in coeffs.iter_mut().zip(dc.iter()) {
                        block[0] = value;
                    }
                    (TYPE_Y_AFTER_Y2, 1)
                } else {
                    (TYPE_Y_WITH_DC, 0)
                };
                for by in 0..4 {
                    for bx in 0..4 {
                        let ctx = usize::from(top.y[bx]) + usize::from(left_nz.y[by]);
                        let block = &mut coeffs[by * 4 + bx];
                        let n = read_block(t, &probs[y_type], ctx, first, q.y_dc, q.y_ac, block);
                        let nz = n > first;
                        top.y[bx] = nz;
                        left_nz.y[by] = nz;
                        has_data |= nz;
                    }
                }
                for (plane, base) in [(0usize, 16usize), (1, 20)] {
                    for by in 0..2 {
                        for bx in 0..2 {
                            let (above_flag, left_flag) = if plane == 0 {
                                (top.u[bx], left_nz.u[by])
                            } else {
                                (top.v[bx], left_nz.v[by])
                            };
                            let ctx = usize::from(above_flag) + usize::from(left_flag);
                            let block = &mut coeffs[base + by * 2 + bx];
                            let n =
                                read_block(t, &probs[TYPE_CHROMA], ctx, 0, q.uv_dc, q.uv_ac, block);
                            let nz = n > 0;
                            if plane == 0 {
                                top.u[bx] = nz;
                                left_nz.u[by] = nz;
                            } else {
                                top.v[bx] = nz;
                                left_nz.v[by] = nz;
                            }
                            has_data |= nz;
                        }
                    }
                }
            }

            reconstruct(
                &mut frame, mb_x, mb_y, mb_w, y_mode, &sub_modes, uv_mode, &coeffs,
            );

            let mut level = header.filter_levels[segment];
            if header.lf_delta {
                level += header.ref_delta0;
                if y_mode == B_PRED {
                    level += header.mode_delta0;
                }
            }
            filters[mb_y * mb_w + mb_x] = MbFilter {
                level: level.clamp(0, 63) as u8,
                inner: y_mode == B_PRED || has_data,
            };
        }
        d.check()?;
        tokens[token_index].check()?;
    }

    if header.frame_filter_level > 0 {
        loop_filter(&mut frame, &header, &filters, mb_w, mb_h);
    }
    Ok(frame)
}

/// Predict one macroblock and add its residual, in place in `frame`.
#[allow(clippy::too_many_arguments)]
fn reconstruct(
    frame: &mut Frame,
    mb_x: usize,
    mb_y: usize,
    mb_w: usize,
    y_mode: usize,
    sub_modes: &[u8; 16],
    uv_mode: usize,
    coeffs: &[[i32; 16]; 24],
) {
    let has_top = mb_y > 0;
    let has_left = mb_x > 0;

    // Luma edges: corner, 16 above, 4 above-right; 16 left.
    let stride = frame.y_stride;
    let x0 = mb_x * 16;
    let y0 = mb_y * 16;
    let mut top = [127u8; 21];
    let mut left = [129u8; 16];
    if has_top {
        let row = (y0 - 1) * stride;
        top[0] = if has_left { frame.y[row + x0 - 1] } else { 129 };
        top[1..17].copy_from_slice(&frame.y[row + x0..row + x0 + 16]);
        if mb_x + 1 < mb_w {
            top[17..21].copy_from_slice(&frame.y[row + x0 + 16..row + x0 + 20]);
        } else {
            top[17..21].fill(frame.y[row + x0 + 15]);
        }
    }
    if has_left {
        for (r, l) in left.iter_mut().enumerate() {
            *l = frame.y[(y0 + r) * stride + x0 - 1];
        }
    }

    let base = y0 * stride + x0;
    if y_mode == B_PRED {
        for i in 0..16 {
            let (sr, sc) = (i / 4, i % 4);
            let mut above = [0u8; 9];
            let mut side = [0u8; 4];
            if sr == 0 {
                above.copy_from_slice(&top[4 * sc..4 * sc + 9]);
            } else {
                let row = base + (4 * sr - 1) * stride;
                above[0] = if sc == 0 {
                    left[4 * sr - 1]
                } else {
                    frame.y[row + 4 * sc - 1]
                };
                above[1..5].copy_from_slice(&frame.y[row + 4 * sc..row + 4 * sc + 4]);
                if sc == 3 {
                    above[5..9].copy_from_slice(&top[17..21]);
                } else {
                    above[5..9].copy_from_slice(&frame.y[row + 4 * sc + 4..row + 4 * sc + 8]);
                }
            }
            for (r, s) in side.iter_mut().enumerate() {
                *s = if sc == 0 {
                    left[4 * sr + r]
                } else {
                    frame.y[base + (4 * sr + r) * stride + 4 * sc - 1]
                };
            }
            let mut block = [[0u8; 4]; 4];
            predict_subblock(&mut block, sub_modes[i], &above, &side);
            let pos = base + 4 * sr * stride + 4 * sc;
            for (r, row) in block.iter().enumerate() {
                frame.y[pos + r * stride..pos + r * stride + 4].copy_from_slice(row);
            }
            if coeffs[i].iter().any(|&c| c != 0) {
                idct_add(&coeffs[i], &mut frame.y, pos, stride);
            }
        }
    } else {
        predict_block(
            &mut frame.y,
            base,
            stride,
            16,
            y_mode,
            &top[..17],
            &left,
            has_top,
            has_left,
        );
        for (i, block) in coeffs.iter().enumerate().take(16) {
            if block.iter().any(|&c| c != 0) {
                let pos = base + 4 * (i / 4) * stride + 4 * (i % 4);
                idct_add(block, &mut frame.y, pos, stride);
            }
        }
    }

    // Chroma: the same edge rules at 8x8.
    let stride = frame.uv_stride;
    let x0 = mb_x * 8;
    let y0 = mb_y * 8;
    let base = y0 * stride + x0;
    for (plane, first_block) in [(&mut frame.u, 16usize), (&mut frame.v, 20usize)] {
        let mut top = [127u8; 9];
        let mut left = [129u8; 8];
        if has_top {
            let row = (y0 - 1) * stride;
            top[0] = if has_left { plane[row + x0 - 1] } else { 129 };
            top[1..9].copy_from_slice(&plane[row + x0..row + x0 + 8]);
        }
        if has_left {
            for (r, l) in left.iter_mut().enumerate() {
                *l = plane[(y0 + r) * stride + x0 - 1];
            }
        }
        predict_block(
            plane, base, stride, 8, uv_mode, &top, &left, has_top, has_left,
        );
        for j in 0..4 {
            let block = &coeffs[first_block + j];
            if block.iter().any(|&c| c != 0) {
                let pos = base + 4 * (j / 2) * stride + 4 * (j % 2);
                idct_add(block, plane, pos, stride);
            }
        }
    }
}

/// Filter strengths derived from a macroblock's level.
struct EdgeLimits {
    mb_edge: i32,
    sub_edge: i32,
    interior: i32,
    hev: i32,
}

fn edge_limits(level: i32, sharpness: i32) -> EdgeLimits {
    let mut interior = level;
    if sharpness > 0 {
        interior >>= if sharpness > 4 { 2 } else { 1 };
        interior = interior.min(9 - sharpness);
    }
    let interior = interior.max(1);
    let hev = if level >= 40 {
        2
    } else if level >= 15 {
        1
    } else {
        0
    };
    EdgeLimits {
        mb_edge: (level + 2) * 2 + interior,
        sub_edge: level * 2 + interior,
        interior,
        hev,
    }
}

/// Clamp to the signed 8-bit range.
fn s8(v: i32) -> i32 {
    v.clamp(-128, 127)
}

/// Read the eight pixels across an edge as signed values `p3 p2 p1 p0 q0 q1 q2 q3`.
fn segment(buf: &[u8], pos: usize, step: usize) -> [i32; 8] {
    let mut s = [0i32; 8];
    for (k, v) in s.iter_mut().enumerate() {
        *v = i32::from(buf[pos + k * step - 4 * step]) - 128;
    }
    s
}

fn put(buf: &mut [u8], pos: usize, value: i32) {
    buf[pos] = (s8(value) + 128) as u8;
}

/// Edge test shared by the simple filter and the normal filters.
fn edge_ok(s: &[i32; 8], limit: i32) -> bool {
    (s[3] - s[4]).abs() * 2 + (s[2] - s[5]).abs() / 2 <= limit
}

fn interior_ok(s: &[i32; 8], interior: i32) -> bool {
    (s[0] - s[1]).abs() <= interior
        && (s[1] - s[2]).abs() <= interior
        && (s[2] - s[3]).abs() <= interior
        && (s[7] - s[6]).abs() <= interior
        && (s[6] - s[5]).abs() <= interior
        && (s[5] - s[4]).abs() <= interior
}

fn high_variance(s: &[i32; 8], threshold: i32) -> bool {
    (s[2] - s[3]).abs() > threshold || (s[5] - s[4]).abs() > threshold
}

/// Move `p0` and `q0` toward each other. Returns the amount taken from `q0`.
fn adjust_edge(buf: &mut [u8], pos: usize, step: usize, s: &[i32; 8], outer_taps: bool) -> i32 {
    let (p1, p0, q0, q1) = (s[2], s[3], s[4], s[5]);
    let base = if outer_taps { s8(p1 - q1) } else { 0 };
    let a = s8(base + 3 * (q0 - p0));
    let b = s8(a + 3) >> 3;
    let a = s8(a + 4) >> 3;
    put(buf, pos, q0 - a);
    put(buf, pos - step, p0 + b);
    a
}

fn simple_filter(buf: &mut [u8], pos: usize, step: usize, limit: i32) {
    // Only p1..q1 are needed, but `segment` reads p3..q3. Read the four
    // directly so a frame edge two pixels away is never touched.
    let p1 = i32::from(buf[pos - 2 * step]) - 128;
    let p0 = i32::from(buf[pos - step]) - 128;
    let q0 = i32::from(buf[pos]) - 128;
    let q1 = i32::from(buf[pos + step]) - 128;
    let s = [0, 0, p1, p0, q0, q1, 0, 0];
    if edge_ok(&s, limit) {
        adjust_edge(buf, pos, step, &s, true);
    }
}

fn subblock_filter(buf: &mut [u8], pos: usize, step: usize, lim: &EdgeLimits) {
    let s = segment(buf, pos, step);
    if !edge_ok(&s, lim.sub_edge) || !interior_ok(&s, lim.interior) {
        return;
    }
    let hev = high_variance(&s, lim.hev);
    let a = (adjust_edge(buf, pos, step, &s, hev) + 1) >> 1;
    if !hev {
        put(buf, pos + step, s[5] - a);
        put(buf, pos - 2 * step, s[2] + a);
    }
}

fn macroblock_filter(buf: &mut [u8], pos: usize, step: usize, lim: &EdgeLimits) {
    let s = segment(buf, pos, step);
    if !edge_ok(&s, lim.mb_edge) || !interior_ok(&s, lim.interior) {
        return;
    }
    if high_variance(&s, lim.hev) {
        adjust_edge(buf, pos, step, &s, true);
        return;
    }
    let (p2, p1, p0, q0, q1, q2) = (s[1], s[2], s[3], s[4], s[5], s[6]);
    let w = s8(s8(p1 - q1) + 3 * (q0 - p0));
    let a = s8((27 * w + 63) >> 7);
    put(buf, pos, q0 - a);
    put(buf, pos - step, p0 + a);
    let a = s8((18 * w + 63) >> 7);
    put(buf, pos + step, q1 - a);
    put(buf, pos - 2 * step, p1 + a);
    let a = s8((9 * w + 63) >> 7);
    put(buf, pos + 2 * step, q2 - a);
    put(buf, pos - 3 * step, p2 + a);
}

/// Which filter to run on each pixel of an edge.
#[derive(Clone, Copy)]
enum EdgeKind {
    Simple(i32),
    Macroblock,
    Subblock,
}

/// Filter `count` segments along one edge starting at `pos`.
///
/// `step` crosses the edge (1 for a vertical edge, the stride for a
/// horizontal one); `along` moves to the next segment.
fn filter_edge(
    buf: &mut [u8],
    pos: usize,
    step: usize,
    along: usize,
    count: usize,
    kind: EdgeKind,
    lim: &EdgeLimits,
) {
    for k in 0..count {
        let at = pos + k * along;
        match kind {
            EdgeKind::Simple(limit) => simple_filter(buf, at, step, limit),
            EdgeKind::Macroblock => macroblock_filter(buf, at, step, lim),
            EdgeKind::Subblock => subblock_filter(buf, at, step, lim),
        }
    }
}

/// Run the loop filter over the whole frame, one macroblock at a time in
/// raster order: left edge, inner vertical edges, top edge, inner horizontal edges.
fn loop_filter(frame: &mut Frame, header: &Header, filters: &[MbFilter], mb_w: usize, mb_h: usize) {
    let ys = frame.y_stride;
    let uvs = frame.uv_stride;
    for mb_y in 0..mb_h {
        for mb_x in 0..mb_w {
            let info = filters[mb_y * mb_w + mb_x];
            if info.level == 0 {
                continue;
            }
            let lim = edge_limits(i32::from(info.level), header.sharpness);
            let y_base = mb_y * 16 * ys + mb_x * 16;
            let uv_base = mb_y * 8 * uvs + mb_x * 8;
            if header.simple_filter {
                let y = &mut frame.y;
                if mb_x > 0 {
                    filter_edge(y, y_base, 1, ys, 16, EdgeKind::Simple(lim.mb_edge), &lim);
                }
                if info.inner {
                    for x in [4, 8, 12] {
                        filter_edge(
                            y,
                            y_base + x,
                            1,
                            ys,
                            16,
                            EdgeKind::Simple(lim.sub_edge),
                            &lim,
                        );
                    }
                }
                if mb_y > 0 {
                    filter_edge(y, y_base, ys, 1, 16, EdgeKind::Simple(lim.mb_edge), &lim);
                }
                if info.inner {
                    for r in [4, 8, 12] {
                        filter_edge(
                            y,
                            y_base + r * ys,
                            ys,
                            1,
                            16,
                            EdgeKind::Simple(lim.sub_edge),
                            &lim,
                        );
                    }
                }
                continue;
            }
            let planes: [(&mut Vec<u8>, usize, usize, usize); 3] = [
                (&mut frame.y, y_base, ys, 16),
                (&mut frame.u, uv_base, uvs, 8),
                (&mut frame.v, uv_base, uvs, 8),
            ];
            for (buf, base, stride, size) in planes {
                if mb_x > 0 {
                    filter_edge(buf, base, 1, stride, size, EdgeKind::Macroblock, &lim);
                }
                if info.inner {
                    for x in (4..size).step_by(4) {
                        filter_edge(buf, base + x, 1, stride, size, EdgeKind::Subblock, &lim);
                    }
                }
                if mb_y > 0 {
                    filter_edge(buf, base, stride, 1, size, EdgeKind::Macroblock, &lim);
                }
                if info.inner {
                    for r in (4..size).step_by(4) {
                        filter_edge(
                            buf,
                            base + r * stride,
                            stride,
                            1,
                            size,
                            EdgeKind::Subblock,
                            &lim,
                        );
                    }
                }
            }
        }
    }
}

impl Frame {
    /// Convert to RGBA with alpha 255, cropping to the visible size.
    ///
    /// Chroma is upsampled bilinearly: each output pixel mixes the four
    /// nearest chroma samples with weights 9, 3, 3, 1 (out of 16), which
    /// matches 4:2:0 samples sitting between pairs of luma rows and columns.
    /// The color conversion is BT.601 with studio-range luma (16..235).
    pub(crate) fn to_rgba(&self) -> Vec<u8> {
        let width = self.width as usize;
        let height = self.height as usize;
        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);
        let mut out = vec![0u8; width * height * 4];
        for y in 0..height {
            // Nearest chroma row and the next nearest one.
            let near_y = y / 2;
            let far_y = if y % 2 == 0 {
                near_y.saturating_sub(1)
            } else {
                (near_y + 1).min(ch - 1)
            };
            for x in 0..width {
                let near_x = x / 2;
                let far_x = if x % 2 == 0 {
                    near_x.saturating_sub(1)
                } else {
                    (near_x + 1).min(cw - 1)
                };
                let sample = |plane: &[u8]| -> i32 {
                    let at = |cx: usize, cy: usize| i32::from(plane[cy * self.uv_stride + cx]);
                    (9 * at(near_x, near_y)
                        + 3 * at(far_x, near_y)
                        + 3 * at(near_x, far_y)
                        + at(far_x, far_y)
                        + 8)
                        >> 4
                };
                let u = sample(&self.u);
                let v = sample(&self.v);
                let luma = i32::from(self.y[y * self.y_stride + x]);
                let rgb = yuv_to_rgb(luma, u, v);
                let o = (y * width + x) * 4;
                out[o..o + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        out
    }
}

/// BT.601 studio-range YUV to RGB in 16-bit fixed point.
pub(crate) fn yuv_to_rgb(y: i32, u: i32, v: i32) -> [u8; 3] {
    let c = 76309 * (y - 16);
    let d = u - 128;
    let e = v - 128;
    let round = 1 << 15;
    let r = (c + 104597 * e + round) >> 16;
    let g = (c - 25675 * d - 53279 * e + round) >> 16;
    let b = (c + 132201 * d + round) >> 16;
    [clamp255(r), clamp255(g), clamp255(b)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boolean entropy encoder, the mirror of [`BoolDecoder`].
    struct BoolEncoder {
        out: Vec<u8>,
        range: u32,
        bottom: u32,
        bit_count: u32,
    }

    impl BoolEncoder {
        fn new() -> Self {
            BoolEncoder {
                out: Vec::new(),
                range: 255,
                bottom: 0,
                bit_count: 24,
            }
        }

        fn carry(&mut self) {
            let mut i = self.out.len();
            while i > 0 {
                i -= 1;
                if self.out[i] == 255 {
                    self.out[i] = 0;
                } else {
                    self.out[i] += 1;
                    break;
                }
            }
        }

        fn put(&mut self, prob: u8, bit: bool) {
            let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
            if bit {
                self.bottom += split;
                self.range -= split;
            } else {
                self.range = split;
            }
            while self.range < 128 {
                self.range <<= 1;
                if self.bottom & (1 << 31) != 0 {
                    self.carry();
                }
                self.bottom <<= 1;
                self.bit_count -= 1;
                if self.bit_count == 0 {
                    self.out.push((self.bottom >> 24) as u8);
                    self.bottom &= (1 << 24) - 1;
                    self.bit_count = 8;
                }
            }
        }

        fn literal(&mut self, value: u32, n: u32) {
            for i in (0..n).rev() {
                self.put(128, (value >> i) & 1 == 1);
            }
        }

        fn signed(&mut self, value: i32, n: u32) {
            self.put(128, true);
            self.literal(value.unsigned_abs(), n);
            self.put(128, value < 0);
        }

        fn finish(mut self) -> Vec<u8> {
            // Pad with enough zero bits to push every pending byte out.
            for _ in 0..32 {
                self.put(128, false);
            }
            self.out
        }
    }

    /// Settings for [`build_frame`].
    struct Plan {
        width: u32,
        height: u32,
        partitions_log2: u32,
        /// Absolute quantizer index per segment, when segmentation is on.
        segment_quant: Option<[u8; 4]>,
        base_quant: u8,
        lf_deltas: bool,
        /// Per macroblock: segment, skip, and the Y2 DC token value.
        blocks: Vec<(usize, bool, i32)>,
    }

    /// Write one DC-only token (value `v` at position 0) followed by end of block.
    fn put_dc_token(e: &mut BoolEncoder, probs: &[[[u8; 11]; 3]; 8], ctx: usize, v: i32) {
        let p = &probs[COEFF_BANDS[0]][ctx];
        let mag = v.unsigned_abs() as i32;
        e.put(p[0], true); // not end of block
        e.put(p[1], true); // not a zero
        let next_ctx;
        if mag == 1 {
            e.put(p[2], false);
            next_ctx = 1;
        } else {
            e.put(p[2], true);
            next_ctx = 2;
            match mag {
                2..=4 => {
                    e.put(p[3], false);
                    if mag == 2 {
                        e.put(p[4], false);
                    } else {
                        e.put(p[4], true);
                        e.put(p[5], mag == 4);
                    }
                }
                5..=10 => {
                    e.put(p[3], true);
                    e.put(p[6], false);
                    if mag <= 6 {
                        e.put(p[7], false);
                        e.put(159, mag == 6);
                    } else {
                        e.put(p[7], true);
                        let extra = mag - 7;
                        e.put(165, extra & 2 != 0);
                        e.put(145, extra & 1 != 0);
                    }
                }
                _ => {
                    e.put(p[3], true);
                    e.put(p[6], true);
                    let (cat, base, extra_probs): (usize, i32, &[u8]) = match mag {
                        11..=18 => (0, 11, &CAT3_PROBS),
                        19..=34 => (1, 19, &CAT4_PROBS),
                        35..=66 => (2, 35, &CAT5_PROBS),
                        _ => (3, 67, &CAT6_PROBS),
                    };
                    let high = cat >= 2;
                    e.put(p[8], high);
                    e.put(p[9 + usize::from(high)], cat & 1 == 1);
                    let extra = (mag - base) as u32;
                    let n = extra_probs.len() as u32;
                    for (i, &prob) in extra_probs.iter().enumerate() {
                        e.put(prob, (extra >> (n - 1 - i as u32)) & 1 == 1);
                    }
                }
            }
        }
        e.put(128, v < 0);
        e.put(probs[COEFF_BANDS[1]][next_ctx][0], false); // end of block
    }

    /// Build a key frame where every macroblock uses DC prediction and
    /// carries at most a Y2 DC value, with the loop filter off.
    fn build_frame(plan: &Plan) -> Vec<u8> {
        let mb_w = (plan.width as usize).div_ceil(16);
        let mb_h = (plan.height as usize).div_ceil(16);
        assert_eq!(plan.blocks.len(), mb_w * mb_h);
        let mut e = BoolEncoder::new();
        e.put(128, false); // color space
        e.put(128, false); // clamping
        if let Some(quant) = plan.segment_quant {
            e.put(128, true); // segmentation on
            e.put(128, true); // update map
            e.put(128, true); // update data
            e.put(128, true); // absolute values
            for q in quant {
                e.signed(i32::from(q), 7);
            }
            for _ in 0..4 {
                e.put(128, false); // no loop filter value
            }
            for prob in [128u8, 100, 160] {
                e.put(128, true);
                e.literal(u32::from(prob), 8);
            }
        } else {
            e.put(128, false);
        }
        e.put(128, false); // normal filter type
        e.literal(0, 6); // filter level 0: no loop filter
        e.literal(0, 3); // sharpness
        if plan.lf_deltas {
            e.put(128, true);
            e.put(128, true);
            for v in [5, -3, 0, 7, -2, 1, 0, 4] {
                if v == 0 {
                    e.put(128, false);
                } else {
                    e.signed(v, 6);
                }
            }
        } else {
            e.put(128, false);
        }
        e.literal(plan.partitions_log2, 2);
        e.literal(u32::from(plan.base_quant), 7);
        for _ in 0..5 {
            e.put(128, false); // no quantizer deltas
        }
        e.put(128, false); // refresh entropy probs
        for plane in COEFF_UPDATE_PROBS.iter() {
            for band in plane.iter() {
                for ctx in band.iter() {
                    for &prob in ctx.iter() {
                        e.put(prob, false);
                    }
                }
            }
        }
        e.put(128, true); // skip flags present
        e.literal(200, 8);
        let skip_prob = 200;

        let count = 1usize << plan.partitions_log2;
        let mut tokens: Vec<BoolEncoder> = (0..count).map(|_| BoolEncoder::new()).collect();
        let probs = &DEFAULT_COEFF_PROBS;
        let mut above_y2 = vec![false; mb_w];
        for mb_y in 0..mb_h {
            let mut left_y2 = false;
            let t = &mut tokens[mb_y & (count - 1)];
            for (mb_x, above) in above_y2.iter_mut().enumerate() {
                let (segment, skip, value) = plan.blocks[mb_y * mb_w + mb_x];
                if plan.segment_quant.is_some() {
                    // Tree: bit with prob 128 picks {0,1} or {2,3}, then 100 or 160.
                    e.put(128, segment >= 2);
                    if segment >= 2 {
                        e.put(160, segment == 3);
                    } else {
                        e.put(100, segment == 1);
                    }
                }
                e.put(skip_prob, skip);
                // DC_PRED: 1, 0, 0 in the key frame luma tree; chroma DC_PRED: 0.
                e.put(145, true);
                e.put(156, false);
                e.put(163, false);
                e.put(142, false);
                if skip {
                    *above = false;
                    left_y2 = false;
                    continue;
                }
                let ctx = usize::from(*above) + usize::from(left_y2);
                if value == 0 {
                    t.put(probs[TYPE_Y2][0][ctx][0], false);
                } else {
                    put_dc_token(t, &probs[TYPE_Y2], ctx, value);
                }
                *above = value != 0;
                left_y2 = value != 0;
                // Sixteen Y blocks and eight chroma blocks, all empty. Every
                // neighbor is empty too, so each context is 0.
                for _ in 0..16 {
                    t.put(probs[TYPE_Y_AFTER_Y2][COEFF_BANDS[1]][0][0], false);
                }
                for _ in 0..8 {
                    t.put(probs[TYPE_CHROMA][0][0][0], false);
                }
            }
        }

        let first = e.finish();
        let parts: Vec<Vec<u8>> = tokens.into_iter().map(BoolEncoder::finish).collect();
        let mut out = Vec::new();
        let tag = ((first.len() as u32) << 5) | (1 << 4); // key frame, version 0, shown
        out.extend_from_slice(&tag.to_le_bytes()[..3]);
        out.extend_from_slice(&[0x9D, 0x01, 0x2A]);
        out.extend_from_slice(&(plan.width as u16).to_le_bytes());
        out.extend_from_slice(&(plan.height as u16).to_le_bytes());
        out.extend_from_slice(&first);
        for part in &parts[..parts.len() - 1] {
            out.extend_from_slice(&(part.len() as u32).to_le_bytes()[..3]);
        }
        for part in &parts {
            out.extend_from_slice(part);
        }
        out
    }

    /// What [`build_frame`] should decode to: one flat luma value per macroblock.
    fn expected_luma(plan: &Plan) -> Vec<u8> {
        let mb_w = (plan.width as usize).div_ceil(16);
        let mb_h = (plan.height as usize).div_ceil(16);
        let mut values = vec![0u8; mb_w * mb_h];
        for mb_y in 0..mb_h {
            for mb_x in 0..mb_w {
                let above = (mb_y > 0).then(|| u32::from(values[(mb_y - 1) * mb_w + mb_x]));
                let left = (mb_x > 0).then(|| u32::from(values[mb_y * mb_w + mb_x - 1]));
                let pred = match (above, left) {
                    (Some(a), Some(l)) => (16 * a + 16 * l + 16) >> 5,
                    (Some(a), None) => a,
                    (None, Some(l)) => l,
                    (None, None) => 128,
                } as i32;
                let (segment, skip, value) = plan.blocks[mb_y * mb_w + mb_x];
                let q = match plan.segment_quant {
                    Some(quant) => quant[segment],
                    None => plan.base_quant,
                };
                let dc = if skip {
                    0
                } else {
                    let y2_dc = DC_QUANT[usize::from(q)] * 2;
                    // Inverse WHT of a lone DC, then inverse DCT of a lone DC.
                    (((value * y2_dc + 3) >> 3) + 4) >> 3
                };
                values[mb_y * mb_w + mb_x] = (pred + dc).clamp(0, 255) as u8;
            }
        }
        values
    }

    fn check_plan(plan: &Plan) {
        let data = build_frame(plan);
        let frame = decode(&data).expect("self-made frame decodes");
        assert_eq!((frame.width, frame.height), (plan.width, plan.height));
        let expected = expected_luma(plan);
        let mb_w = (plan.width as usize).div_ceil(16);
        for y in 0..plan.height as usize {
            for x in 0..plan.width as usize {
                let want = expected[(y / 16) * mb_w + x / 16];
                assert_eq!(frame.y[y * frame.y_stride + x], want, "luma at {x},{y}");
            }
        }
        for y in 0..(plan.height as usize).div_ceil(2) {
            for x in 0..(plan.width as usize).div_ceil(2) {
                assert_eq!(frame.u[y * frame.uv_stride + x], 128);
                assert_eq!(frame.v[y * frame.uv_stride + x], 128);
            }
        }
    }

    #[test]
    fn bool_decoder_reads_what_the_encoder_wrote() {
        let mut e = BoolEncoder::new();
        let bits: Vec<(u8, bool)> = (0..500u32)
            .map(|i| (((i * 37) % 255 + 1) as u8, (i * 7919) % 3 == 0))
            .collect();
        for &(prob, bit) in &bits {
            e.put(prob, bit);
        }
        e.literal(0x2A5, 10);
        e.signed(-17, 6);
        let data = e.finish();
        let mut d = BoolDecoder::new(&data);
        for &(prob, bit) in &bits {
            assert_eq!(d.read_bool(prob), bit);
        }
        assert_eq!(d.read_literal(10), 0x2A5);
        assert_eq!(d.read_optional_signed(6), -17);
        assert!(d.check().is_ok());
    }

    #[test]
    fn bool_decoder_flags_reads_far_past_the_end() {
        let mut d = BoolDecoder::new(&[0x55]);
        for _ in 0..64 {
            d.read_bool(128);
        }
        assert_eq!(d.check(), Err(ImageError::UnexpectedEof));
    }

    #[test]
    fn trees_decode_every_leaf() {
        for mode in 0..10u8 {
            // Find the path to `mode` by search, encode it, and decode it back.
            fn path(tree: &[i8], node: usize, target: i8, bits: &mut Vec<(usize, bool)>) -> bool {
                for branch in 0..2 {
                    let next = tree[node + branch];
                    bits.push((node >> 1, branch == 1));
                    if (next <= 0 && -next == target)
                        || (next > 0 && path(tree, next as usize, target, bits))
                    {
                        return true;
                    }
                    bits.pop();
                }
                false
            }
            let probs = KF_BMODE_PROBS[3][7];
            let mut bits = Vec::new();
            assert!(path(&BMODE_TREE, 0, mode as i8, &mut bits));
            let mut e = BoolEncoder::new();
            for &(i, bit) in &bits {
                e.put(probs[i], bit);
            }
            let data = e.finish();
            let mut d = BoolDecoder::new(&data);
            assert_eq!(d.read_tree(&BMODE_TREE, &probs), usize::from(mode));
        }
    }

    #[test]
    fn inverse_transforms_match_hand_results() {
        // A lone Y2 DC spreads evenly.
        let mut y2 = [0i32; 16];
        y2[0] = 80;
        assert_eq!(inverse_wht(&y2), [10; 16]);
        // Two inputs: the sign pattern of the second basis function.
        y2[1] = 16;
        let out = inverse_wht(&y2);
        assert_eq!(&out[..4], &[12, 12, 8, 8]);
        // A lone DCT DC adds (dc + 4) >> 3 everywhere.
        let mut plane = vec![100u8; 16];
        let mut coeffs = [0i32; 16];
        coeffs[0] = 36;
        idct_add(&coeffs, &mut plane, 0, 4);
        assert!(plane.iter().all(|&p| p == 105));
        // Results are clamped to 0..=255.
        let mut plane = vec![250u8; 16];
        coeffs[0] = 2000;
        idct_add(&coeffs, &mut plane, 0, 4);
        assert!(plane.iter().all(|&p| p == 255));
        // A first AC coefficient makes a left-to-right ramp.
        let mut plane = vec![128u8; 16];
        let mut coeffs = [0i32; 16];
        coeffs[1] = 100;
        idct_add(&coeffs, &mut plane, 0, 4);
        assert!(plane[0] > plane[1] && plane[1] > plane[2] && plane[2] > plane[3]);
        assert_eq!(plane[0], plane[12]);
        // Huge values from a corrupt stream must not overflow.
        let coeffs = [i32::from(i16::MAX) * 300; 16];
        let mut plane = vec![0u8; 16];
        idct_add(&coeffs, &mut plane, 0, 4);
    }

    #[test]
    fn subblock_modes_follow_the_spec_formulas() {
        let above = [50u8, 10, 20, 30, 40, 60, 70, 80, 90];
        let left = [100u8, 110, 120, 130];
        let mut b = [[0u8; 4]; 4];
        predict_subblock(&mut b, B_DC_PRED, &above, &left);
        assert_eq!(b, [[70; 4]; 4]); // (10+20+30+40+100+110+120+130+4) >> 3
        predict_subblock(&mut b, B_TM_PRED, &above, &left);
        assert_eq!(b[0], [60, 70, 80, 90]);
        assert_eq!(b[3][3], 120);
        predict_subblock(&mut b, B_VE_PRED, &above, &left);
        assert_eq!(
            b[0],
            [
                avg3(50, 10, 20),
                avg3(10, 20, 30),
                avg3(20, 30, 40),
                avg3(30, 40, 60)
            ]
        );
        assert_eq!(b[0], b[3]);
        predict_subblock(&mut b, B_HE_PRED, &above, &left);
        assert_eq!(b[0], [avg3(50, 100, 110); 4]);
        assert_eq!(b[3], [avg3(120, 130, 130); 4]);
        predict_subblock(&mut b, B_LD_PRED, &above, &left);
        assert_eq!(b[0][0], avg3(10, 20, 30));
        assert_eq!(b[3][3], avg3(80, 90, 90));
        assert_eq!(b[1][2], b[2][1]);
        predict_subblock(&mut b, B_RD_PRED, &above, &left);
        assert_eq!(b[0][0], avg3(100, 50, 10));
        assert_eq!(b[3][0], avg3(130, 120, 110));
        assert_eq!(b[0][3], avg3(20, 30, 40));
        predict_subblock(&mut b, B_VR_PRED, &above, &left);
        assert_eq!(b[0][0], avg2(50, 10));
        assert_eq!(b[3][0], avg3(120, 110, 100));
        predict_subblock(&mut b, B_VL_PRED, &above, &left);
        assert_eq!(b[0][0], avg2(10, 20));
        assert_eq!(b[3][3], avg3(70, 80, 90));
        predict_subblock(&mut b, B_HD_PRED, &above, &left);
        assert_eq!(b[3][0], avg2(130, 120));
        assert_eq!(b[0][3], avg3(10, 20, 30));
        predict_subblock(&mut b, B_HU_PRED, &above, &left);
        assert_eq!(b[0][0], avg2(100, 110));
        assert_eq!(b[3], [130; 4]);
    }

    #[test]
    fn whole_block_dc_handles_frame_edges() {
        let top = [0u8, 10, 10, 10, 10, 10, 10, 10, 10];
        let left = [30u8; 8];
        let mut plane = vec![0u8; 64];
        predict_block(&mut plane, 0, 8, 8, DC_PRED, &top, &left, true, true);
        assert_eq!(plane[0], 20);
        predict_block(&mut plane, 0, 8, 8, DC_PRED, &top, &left, true, false);
        assert_eq!(plane[63], 10);
        predict_block(&mut plane, 0, 8, 8, DC_PRED, &top, &left, false, true);
        assert_eq!(plane[63], 30);
        predict_block(&mut plane, 0, 8, 8, DC_PRED, &top, &left, false, false);
        assert_eq!(plane[63], 128);
        predict_block(&mut plane, 0, 8, 8, TM_PRED, &top, &left, true, true);
        assert_eq!(plane[63], 40);
    }

    #[test]
    fn loop_filter_limits_follow_level_and_sharpness() {
        let lim = edge_limits(32, 0);
        assert_eq!(
            (lim.interior, lim.mb_edge, lim.sub_edge, lim.hev),
            (32, 100, 96, 1)
        );
        let lim = edge_limits(32, 5);
        assert_eq!(lim.interior, 4);
        let lim = edge_limits(1, 3);
        assert_eq!(lim.interior, 1);
        assert_eq!(edge_limits(40, 0).hev, 2);
        assert_eq!(edge_limits(14, 0).hev, 0);
    }

    #[test]
    fn loop_filter_smooths_a_small_step_and_keeps_a_big_one() {
        let lim = edge_limits(20, 0);
        let mut row = [60u8, 60, 60, 60, 64, 64, 64, 64];
        macroblock_filter(&mut row, 4, 1, &lim);
        assert!(
            row[3] > 60 && row[4] < 64,
            "small step is softened: {row:?}"
        );
        let mut row = [10u8, 10, 10, 10, 200, 200, 200, 200];
        macroblock_filter(&mut row, 4, 1, &lim);
        assert_eq!(row, [10, 10, 10, 10, 200, 200, 200, 200]);
        let mut row = [60u8, 60, 60, 60, 64, 64, 64, 64];
        subblock_filter(&mut row, 4, 1, &lim);
        assert!(row[3] > 60 && row[4] < 64);
        let mut row = [0u8, 0, 60, 60, 64, 64, 0, 0];
        simple_filter(&mut row, 4, 1, lim.mb_edge);
        assert_eq!((row[3], row[4]), (61, 63));
    }

    #[test]
    fn color_conversion_hits_known_points() {
        assert_eq!(yuv_to_rgb(16, 128, 128), [0, 0, 0]);
        assert_eq!(yuv_to_rgb(235, 128, 128), [255, 255, 255]);
        assert_eq!(yuv_to_rgb(126, 128, 128), [128, 128, 128]);
        let red = yuv_to_rgb(81, 90, 240);
        assert!(red[0] > 250 && red[1] < 5 && red[2] < 5, "{red:?}");
    }

    #[test]
    fn header_checks_reject_bad_frames() {
        let plan = Plan {
            width: 20,
            height: 20,
            partitions_log2: 0,
            segment_quant: None,
            base_quant: 0,
            lf_deltas: false,
            blocks: vec![(0, false, 8); 4],
        };
        let good = build_frame(&plan);
        assert!(decode(&good).is_ok());
        let mut inter = good.clone();
        inter[0] |= 1;
        assert!(matches!(
            decode(&inter),
            Err(ImageError::UnsupportedFormat(_))
        ));
        let mut bad_code = good.clone();
        bad_code[4] = 0;
        assert!(matches!(
            decode(&bad_code),
            Err(ImageError::InvalidHeader(_))
        ));
        let mut zero = good.clone();
        zero[6] = 0;
        zero[7] = 0;
        assert!(decode(&zero).is_err());
        let mut huge = good.clone();
        huge[6..8].copy_from_slice(&16383u16.to_le_bytes());
        huge[8..10].copy_from_slice(&16383u16.to_le_bytes());
        assert!(matches!(
            decode(&huge),
            Err(ImageError::DimensionsTooLarge { .. })
        ));
        assert_eq!(decode(&good[..9]).err(), Some(ImageError::UnexpectedEof));
        assert_eq!(decode(&good[..12]).err(), Some(ImageError::UnexpectedEof));
        // Keep the first partition but only one byte of tokens: the token
        // decoder runs dry and the frame is reported as truncated.
        let busy = build_frame(&Plan {
            width: 64,
            height: 64,
            blocks: vec![(0, false, -900); 16],
            ..plan
        });
        let first_size = (u32::from_le_bytes([busy[0], busy[1], busy[2], 0]) >> 5) as usize;
        assert!(decode(&busy).is_ok());
        assert_eq!(
            decode(&busy[..10 + first_size + 1]).err(),
            Some(ImageError::UnexpectedEof)
        );
    }

    #[test]
    fn self_made_flat_frames_decode_exactly() {
        let blocks = |n: usize| -> Vec<(usize, bool, i32)> {
            (0..n)
                .map(|i| (i % 4, i % 5 == 3, [8, -40, 120, 0, 3, -2, 900, 1][i % 8]))
                .collect()
        };
        // One partition, no segments, one macroblock.
        check_plan(&Plan {
            width: 16,
            height: 16,
            partitions_log2: 0,
            segment_quant: None,
            base_quant: 0,
            lf_deltas: false,
            blocks: vec![(0, false, 40)],
        });
        // Odd size, several rows, skipped macroblocks, all token sizes.
        check_plan(&Plan {
            width: 50,
            height: 35,
            partitions_log2: 0,
            segment_quant: None,
            base_quant: 3,
            lf_deltas: false,
            blocks: blocks(12),
        });
        // Four token partitions, segment map with absolute quantizers, and
        // loop filter deltas in the header (unused with the filter off).
        for parts in 1..=3 {
            check_plan(&Plan {
                width: 70,
                height: 90,
                partitions_log2: parts,
                segment_quant: Some([0, 10, 40, 90]),
                base_quant: 5,
                lf_deltas: true,
                blocks: blocks(30),
            });
        }
    }

    #[test]
    fn rgba_output_is_cropped_and_opaque() {
        let plan = Plan {
            width: 3,
            height: 5,
            partitions_log2: 0,
            segment_quant: None,
            base_quant: 0,
            lf_deltas: false,
            blocks: vec![(0, false, 0)],
        };
        let frame = decode(&build_frame(&plan)).unwrap();
        let rgba = frame.to_rgba();
        assert_eq!(rgba.len(), 3 * 5 * 4);
        let gray = yuv_to_rgb(128, 128, 128);
        for px in rgba.chunks(4) {
            assert_eq!(px, &[gray[0], gray[1], gray[2], 255]);
        }
    }
}
